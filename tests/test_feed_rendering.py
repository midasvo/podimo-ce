"""Structural snapshot tests for `podcastsToRss`.

Lock down the rendered RSS shape so future feedgen bumps (or local code
changes) can't silently drop required elements. Assertions are deliberately
substring-based to avoid coupling to volatile bits feedgen controls:
``lastBuildDate``, generator version string, exact whitespace/indentation,
attribute ordering.
"""

from unittest.mock import AsyncMock

import main


def _fixed_payload() -> dict:
    """Deterministic two-episode payload.

    Uses ISO-8601 strings for ``publishDatetime`` so feedgen renders them
    consistently and the test isn't sensitive to local-time conversion.
    """
    return {
        "podcast": {
            "title": "Test Show",
            "description": "A deterministic test show",
            "webAddress": None,
            "authorName": "Author",
            "language": "nl",
            "images": {"coverImageUrl": "https://example.com/cover.jpg"},
        },
        "episodes": [
            {
                "id": "ep1",
                "title": "Episode 1",
                "description": "First episode body",
                "publishDatetime": "2024-01-01T12:00:00Z",
                "datetime": "2024-01-01T12:00:00Z",
                "imageUrl": "https://example.com/ep1.jpg",
                "audio": {"url": "https://example.com/ep1.mp3", "duration": 1234},
                "streamMedia": None,
                "artist": "Author",
                "podcastName": "Test Show",
            },
            {
                "id": "ep2",
                "title": "Episode 2",
                "description": "Second episode body",
                "publishDatetime": "2024-01-02T12:00:00Z",
                "datetime": "2024-01-02T12:00:00Z",
                "imageUrl": "https://example.com/ep2.jpg",
                "audio": {"url": "https://example.com/ep2.mp3", "duration": 5678},
                "streamMedia": None,
                "artist": "Author",
                "podcastName": "Test Show",
            },
        ],
    }


async def test_podcastsToRss_renders_expected_structure(monkeypatch):
    # urlHeadInfo would otherwise hit the network for each episode. Stubbing
    # it keeps the test offline and deterministic.
    monkeypatch.setattr(
        main, "urlHeadInfo", AsyncMock(return_value=("12345", "audio/mpeg"))
    )

    rss_bytes = await main.podcastsToRss("podcast-uuid", _fixed_payload(), "nl-NL")
    rss = rss_bytes.decode("utf-8")

    # Top-level RSS skeleton.
    assert "<rss" in rss
    assert "<channel>" in rss
    assert "</channel>" in rss
    assert "</rss>" in rss

    # Channel metadata.
    assert "<title>Test Show</title>" in rss
    assert "<description>A deterministic test show</description>" in rss
    assert "<language>nl</language>" in rss
    assert "<itunes:author>Author</itunes:author>" in rss
    assert "https://podimo.com/shows/podcast-uuid" in rss

    # Two items. feedgen sorts <item> entries by pubDate (newest first), not
    # by insertion order — so we assert presence + count rather than position.
    assert rss.count("<item>") == 2
    assert rss.count("</item>") == 2
    assert "<title>Episode 1</title>" in rss
    assert "<title>Episode 2</title>" in rss

    # GUIDs come through unchanged.
    assert "<guid" in rss and "ep1" in rss and "ep2" in rss

    # itunes:duration carried through for each episode.
    assert "<itunes:duration>1234</itunes:duration>" in rss
    assert "<itunes:duration>5678</itunes:duration>" in rss

    # Enclosure rendered for both episodes with the head-info metadata.
    assert rss.count('<enclosure url="https://example.com/ep1.mp3"') == 1
    assert rss.count('<enclosure url="https://example.com/ep2.mp3"') == 1
    assert 'type="audio/mpeg"' in rss
    assert 'length="12345"' in rss


async def test_podcastsToRss_appends_jpg_fragment_to_extensionless_image_urls(monkeypatch):
    """Regression test for the fork-local `#.jpg` patch.

    Podimo serves cover/episode images as signed query-string URLs without a
    file extension. feedgen (and Apple's spec) require an image URL to end in
    ``.jpg`` or ``.png``, so this fork appends a ``#.jpg`` fragment as a
    workaround — clients strip URL fragments before the HTTP GET, so the
    actual image fetch is unaffected.

    Applied in both ``podcastsToRss`` (channel-level image) and
    ``addFeedEntry`` (per-episode ``itunes:image``).
    """
    monkeypatch.setattr(
        main, "urlHeadInfo", AsyncMock(return_value=("0", "audio/mpeg"))
    )

    payload = _fixed_payload()
    # Extensionless URLs of the shape Podimo actually returns.
    payload["podcast"]["images"]["coverImageUrl"] = (
        "https://images.podimo.com/cover?sig=abcdef"
    )
    payload["episodes"][0]["imageUrl"] = (
        "https://images.podimo.com/ep1?sig=xyz"
    )
    payload["episodes"][1]["imageUrl"] = (
        "https://images.podimo.com/ep2?sig=qrs"
    )

    rss_bytes = await main.podcastsToRss("podcast-uuid", payload, "nl-NL")
    rss = rss_bytes.decode("utf-8")

    # Channel-level cover image: fragment appended.
    assert "https://images.podimo.com/cover?sig=abcdef#.jpg" in rss
    # Per-episode itunes:image: fragment appended for both.
    assert "https://images.podimo.com/ep1?sig=xyz#.jpg" in rss
    assert "https://images.podimo.com/ep2?sig=qrs#.jpg" in rss
    # Sanity: a URL that already ends in .jpg shouldn't get a second fragment.
    assert "ep1.jpg#.jpg" not in rss


async def test_podcastsToRss_preserves_existing_jpg_extension(monkeypatch):
    """Image URLs that already end in .jpg / .png must NOT be mutated."""
    monkeypatch.setattr(
        main, "urlHeadInfo", AsyncMock(return_value=("0", "audio/mpeg"))
    )
    payload = _fixed_payload()
    # All image URLs in _fixed_payload already end in .jpg.
    rss = (await main.podcastsToRss("podcast-uuid", payload, "nl-NL")).decode("utf-8")
    assert "https://example.com/cover.jpg" in rss
    assert "https://example.com/cover.jpg#.jpg" not in rss
    assert "https://example.com/ep1.jpg" in rss
    assert "https://example.com/ep1.jpg#.jpg" not in rss
