"""Integration tests for the /feed/<id>.xml flow.

These exist primarily as a safety net for the upcoming
werkzeug 2 -> 3 + quart 0.18 -> 0.20 bump: they exercise
`request.authorization` (HTTP Basic) and the comma-separated
`username,region,locale` shape that the fork overloads onto the
username field. Mocks are kept high-level (main.check_auth,
main.urlHeadInfo) so the assertions are about HTTP-level behaviour,
not GraphQL/aiohttp internals.
"""

import base64
import uuid
from unittest.mock import AsyncMock, MagicMock

import main


def _auth_header(username: str, password: str) -> dict:
    raw = f"{username}:{password}".encode("utf-8")
    return {"Authorization": "Basic " + base64.b64encode(raw).decode("ascii")}


def _fake_payload() -> dict:
    return {
        "podcast": {
            "title": "Test Show",
            "description": "Hello world",
            "webAddress": None,
            "authorName": "Author",
            "language": "nl",
            "images": {"coverImageUrl": "https://example.com/cover.jpg"},
        },
        "episodes": [
            {
                "id": "ep1",
                "title": "Episode 1",
                "description": "First episode",
                "publishDatetime": "2024-01-01T12:00:00Z",
                "datetime": "2024-01-01T12:00:00Z",
                "imageUrl": "https://example.com/ep1.jpg",
                "audio": {"url": "https://example.com/ep1.mp3", "duration": 1234},
                "streamMedia": None,
                "artist": "Author",
                "podcastName": "Test Show",
            }
        ],
    }


def _mock_client(payload=None) -> MagicMock:
    if payload is None:
        payload = _fake_payload()
    client = MagicMock()
    client.getPodcasts = AsyncMock(return_value=payload)
    client.getPodcastName = MagicMock(return_value="Test Show")
    return client


async def test_unauthenticated_returns_401_with_no_store():
    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as client:
        response = await client.get(f"/feed/{feed_id}.xml")
    assert response.status_code == 401
    assert response.headers.get("Cache-Control") == "no-store"


async def test_happy_path_returns_rss(monkeypatch):
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=_mock_client()))
    # feedgen requires content-length as a string for the enclosure attribute;
    # production reads it straight off response.headers['content-length'] (also
    # a string), so the brief's `12345` int is rendered as "12345" here.
    monkeypatch.setattr(main, "urlHeadInfo", AsyncMock(return_value=("12345", "audio/mpeg")))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as client:
        response = await client.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,nl,nl-NL", "pw"),
        )
    assert response.status_code == 200
    assert response.mimetype == "text/xml"
    body = (await response.get_data()).decode("utf-8")
    assert "<rss" in body
    assert "<title>" in body
    assert "<item>" in body


async def test_podcast_not_found_returns_404(monkeypatch):
    client_mock = MagicMock()
    client_mock.getPodcasts = AsyncMock(
        side_effect=RuntimeError("GraphQL error: Podcast not found")
    )
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=client_mock))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        response = await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,nl,nl-NL", "pw"),
        )
    assert response.status_code == 404


async def test_other_upstream_error_returns_500(monkeypatch):
    client_mock = MagicMock()
    client_mock.getPodcasts = AsyncMock(side_effect=RuntimeError("upstream down"))
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=client_mock))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        response = await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,nl,nl-NL", "pw"),
        )
    assert response.status_code == 500


async def test_invalid_podcast_id_format_returns_400(monkeypatch):
    # check_auth shouldn't be reached, but stub it anyway so a leak would
    # not also break this assertion silently.
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=_mock_client()))
    monkeypatch.setattr(main, "BLOCKED", set())

    async with main.app.test_client() as c:
        response = await c.get(
            "/feed/not-a-valid-id!.xml",
            headers=_auth_header("a@b.com,nl,nl-NL", "pw"),
        )
    assert response.status_code == 400
    body = (await response.get_data()).decode("utf-8")
    assert "Invalid podcast id format" in body


async def test_invalid_region_returns_400(monkeypatch):
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=_mock_client()))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        response = await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,zz,nl-NL", "pw"),
        )
    assert response.status_code == 400
    body = (await response.get_data()).decode("utf-8")
    assert "Invalid region" in body


async def test_invalid_locale_returns_400(monkeypatch):
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=_mock_client()))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        response = await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,nl,zz-ZZ", "pw"),
        )
    assert response.status_code == 400
    body = (await response.get_data()).decode("utf-8")
    assert "Invalid locale" in body


async def test_blocked_podcast_returns_410(monkeypatch):
    feed_id = str(uuid.uuid4())
    monkeypatch.setattr(main, "check_auth", AsyncMock(return_value=_mock_client()))
    monkeypatch.setattr(main, "BLOCKED", {feed_id})

    async with main.app.test_client() as c:
        response = await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,nl,nl-NL", "pw"),
        )
    assert response.status_code == 410


async def test_check_auth_receives_three_part_username(monkeypatch):
    mock_check_auth = AsyncMock(return_value=_mock_client())
    monkeypatch.setattr(main, "check_auth", mock_check_auth)
    monkeypatch.setattr(main, "urlHeadInfo", AsyncMock(return_value=(0, "audio/mpeg")))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com,de,de-DE", "secret"),
        )

    assert mock_check_auth.await_count == 1
    args, _ = mock_check_auth.call_args
    # check_auth signature: (username, password, region, locale, scraper)
    assert args[0] == "a@b.com"
    assert args[1] == "secret"
    assert args[2] == "de"
    assert args[3] == "de-DE"


async def test_check_auth_receives_default_region_locale_when_username_has_one_part(monkeypatch):
    mock_check_auth = AsyncMock(return_value=_mock_client())
    monkeypatch.setattr(main, "check_auth", mock_check_auth)
    monkeypatch.setattr(main, "urlHeadInfo", AsyncMock(return_value=(0, "audio/mpeg")))
    monkeypatch.setattr(main, "BLOCKED", set())

    feed_id = str(uuid.uuid4())
    async with main.app.test_client() as c:
        await c.get(
            f"/feed/{feed_id}.xml",
            headers=_auth_header("a@b.com", "secret"),
        )

    assert mock_check_auth.await_count == 1
    args, _ = mock_check_auth.call_args
    assert args[0] == "a@b.com"
    assert args[1] == "secret"
    assert args[2] == "nl"
    assert args[3] == "nl-NL"
