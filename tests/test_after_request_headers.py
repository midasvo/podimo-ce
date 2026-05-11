"""Tests for the @app.after_request CORS and Cache-Control policy.

These exercise the policy implemented in main.allow_cors:
- Access-Control-Allow-Origin/Methods only on GET|HEAD /feed/* responses.
- Cache-Control: max-age=900 on 2xx, no-store on everything else.

All assertions are on response headers so that failures of the upstream
GraphQL flow (which we deliberately don't mock here) don't break the tests.
"""

import uuid

from main import app


async def test_get_root_has_no_cors_origin():
    # Form endpoint is same-origin only; no CORS headers should be emitted.
    async with app.test_client() as client:
        response = await client.get("/")
    assert "Access-Control-Allow-Origin" not in response.headers
    assert "Access-Control-Allow-Methods" not in response.headers


async def test_post_root_has_no_cors_origin():
    # POSTs to the form must never be allowed cross-origin (would expose creds).
    async with app.test_client() as client:
        response = await client.post(
            "/",
            form={
                "email": "",
                "password": "",
                "podcast_id": "",
                "region": "",
                "locale": "",
            },
        )
    assert "Access-Control-Allow-Origin" not in response.headers
    assert "Access-Control-Allow-Methods" not in response.headers


async def test_get_feed_has_permissive_cors_without_post():
    # Browser-based players can fetch feed XML cross-origin. POST must not be
    # advertised as allowed. Status will be 401 (no auth) but CORS headers are
    # still set per the policy because it's a GET on /feed/.
    feed_id = str(uuid.uuid4())
    async with app.test_client() as client:
        response = await client.get(f"/feed/{feed_id}.xml")
    assert response.headers.get("Access-Control-Allow-Origin") == "*"
    allow_methods = response.headers.get("Access-Control-Allow-Methods", "")
    assert "POST" not in allow_methods
    assert "GET" in allow_methods


async def test_2xx_response_has_max_age_cache_control():
    # GET / renders the form template and returns 200.
    async with app.test_client() as client:
        response = await client.get("/")
    assert response.status_code == 200
    assert response.headers.get("Cache-Control") == "max-age=900"


async def test_401_response_has_no_store_cache_control():
    # Unauthenticated feed fetch returns 401; must not be cached by proxies.
    feed_id = str(uuid.uuid4())
    async with app.test_client() as client:
        response = await client.get(f"/feed/{feed_id}.xml")
    assert response.status_code == 401
    assert response.headers.get("Cache-Control") == "no-store"


async def test_404_response_has_no_store_cache_control():
    # Unknown routes hit the 404 handler; must not be cached.
    async with app.test_client() as client:
        response = await client.get("/this-route-does-not-exist")
    assert response.status_code == 404
    assert response.headers.get("Cache-Control") == "no-store"


# TODO: out of scope for WP7 — we don't assert anything about OPTIONS preflight
# behavior. Quart handles preflight separately and adding
# Access-Control-Allow-Headers / Access-Control-Max-Age is explicitly out of
# scope for this work package.
