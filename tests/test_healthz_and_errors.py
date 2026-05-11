"""Tests for the /healthz liveness endpoint and check_auth error differentiation.

- /healthz must return 200 application/json and never be cached.
- Upstream errors propagated from check_auth (RuntimeError, network errors)
  must surface as 503 to distinguish transient outages from bad credentials.
- ValueError from check_auth (bad credentials / malformed email) must still
  produce a 401 so clients are prompted to fix their auth.
"""

import base64
import uuid

import main
from main import app


async def test_healthz_returns_ok():
    async with app.test_client() as client:
        response = await client.get("/healthz")
    assert response.status_code == 200
    body = await response.get_data(as_text=True)
    assert '"ok"' in body
    assert response.headers.get("Content-Type") == "application/json"
    # Liveness probes must always reflect current state — never cache.
    assert response.headers.get("Cache-Control") == "no-store"


def _basic_auth_header(username: str = "user@example.com,nl,nl-NL", password: str = "secret") -> dict:
    token = base64.b64encode(f"{username}:{password}".encode()).decode()
    return {"Authorization": f"Basic {token}"}


async def test_check_auth_runtime_error_returns_503(monkeypatch):
    """Cloudflare blocks / upstream outages must surface as 503, not 401."""

    async def fake_check_auth(*args, **kwargs):
        raise RuntimeError("Cloudflare blocked")

    monkeypatch.setattr(main, "check_auth", fake_check_auth)
    # Also skip LOCAL_CREDENTIALS so the basic-auth branch is exercised.
    monkeypatch.setattr(main, "LOCAL_CREDENTIALS", False)

    feed_id = str(uuid.uuid4())
    async with app.test_client() as client:
        response = await client.get(f"/feed/{feed_id}.xml", headers=_basic_auth_header())

    assert response.status_code == 503
    body = await response.get_data(as_text=True)
    assert "Upstream" in body


async def test_check_auth_value_error_returns_401(monkeypatch):
    """Genuinely bad credentials (ValueError) must still return 401."""

    async def fake_check_auth(*args, **kwargs):
        raise ValueError("bad credentials")

    monkeypatch.setattr(main, "check_auth", fake_check_auth)
    monkeypatch.setattr(main, "LOCAL_CREDENTIALS", False)

    feed_id = str(uuid.uuid4())
    async with app.test_client() as client:
        response = await client.get(f"/feed/{feed_id}.xml", headers=_basic_auth_header())

    assert response.status_code == 401
