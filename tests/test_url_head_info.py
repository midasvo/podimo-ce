from unittest.mock import AsyncMock, MagicMock

import main
from podimo import cache


def _mock_session(headers: dict) -> MagicMock:
    response = MagicMock()
    response.headers = headers
    ctx = MagicMock()
    ctx.__aenter__ = AsyncMock(return_value=response)
    ctx.__aexit__ = AsyncMock(return_value=None)
    session = MagicMock()
    session.head = MagicMock(return_value=ctx)
    return session


async def test_missing_content_length_returns_string_zero(monkeypatch):
    # Regression: when the upstream HEAD has no Content-Length header,
    # urlHeadInfo must still return a string. feedgen/lxml's fe.enclosure(...)
    # raises TypeError on an int, which previously crashed the whole feed
    # render via the broad except in serve_feed.
    monkeypatch.setattr(cache, "head_cache", {})
    session = _mock_session({})

    length, ctype = await main.urlHeadInfo(session, "ep-fresh", "https://example.com/x.mp3", "nl-NL")

    assert length == "0"
    assert isinstance(length, str)


async def test_content_length_from_header_is_passed_through(monkeypatch):
    monkeypatch.setattr(cache, "head_cache", {})
    session = _mock_session({"content-length": "9876"})

    length, _ = await main.urlHeadInfo(session, "ep-with-length", "https://example.com/x.mp3", "nl-NL")

    assert length == "9876"
