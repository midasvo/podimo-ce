"""Tests for the /healthz liveness endpoint and check_auth error differentiation.

- /healthz must return 200 application/json and never be cached.
"""

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
