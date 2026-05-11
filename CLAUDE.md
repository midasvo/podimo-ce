# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Fork context

This repo (`midasvo/podimo-ce`) is a community fork of `ThijsRay/podimo`. Upstream is merged nightly by `.github/workflows/sync-upstream.yaml`, which also mirrors upstream tags as `vX.Y.Z-midasvo.N`. Local-only patches live on `main`; if you add commits here, the next sync will merge upstream into them — avoid rewriting history on `main`. Docker images are published to `ghcr.io/midasvo/podimo-ce` by `docker-publish.yml`.

## Running locally

There is no test suite, lint config, or formatter. Iteration is run-the-server-and-hit-it.

```sh
python -m venv venv && source venv/bin/activate
pip install -r requirements.txt
cp .env.example .env       # then edit; DEBUG=true gives verbose request logs
python main.py             # binds PODIMO_BIND_HOST (default 127.0.0.1:12104)
```

`make install` / `make start` install and run a systemd unit pointing at `venv/bin/python main.py` — use this only on the target host, not for dev iteration. `make update` checks out the latest tag (so it'll move HEAD off `main`); don't run it on a dev clone you're editing.

Docker build matches CI: `docker build -t podimo .` then `docker run -e PODIMO_BIND_HOST=0.0.0.0:12104 -p 12104:12104 podimo`.

## Architecture

Single-process async webserver (Quart on Hypercorn) that proxies Podimo's GraphQL API and re-renders the result as RSS. The flow for a feed request:

1. `main.py:serve_basic_auth_feed` resolves credentials. Two modes, set by `LOCAL_CREDENTIALS`:
   - **off (multi-user)**: creds come from HTTP Basic auth. The username field is overloaded as `email,region,locale` (comma-separated, URL-encoded) — see `split_username_region_locale`. The HTML form in `templates/index.html` builds these URLs.
   - **on (single-user)**: creds come from `PODIMO_EMAIL`/`PODIMO_PASSWORD` env vars; region/locale come from query string (defaulting to `nl`/`nl-NL`).
2. `check_auth` → `PodimoClient.podimoLogin` (`podimo/client.py`) does a three-step GraphQL dance: `tokenWithPreregisterUser` → `userOnboardingFlow` → `tokenWithCredentials`. The final token is cached under `sha256(username~password)`.
3. `client.getPodcasts` pages through `podcastEpisodes` 100 at a time and returns the raw GraphQL payload.
4. `main.py:podcastsToRss` builds the RSS via `feedgen` and fires off `HEAD` requests in chunks of 5 to populate enclosure `Content-Length`/`Content-Type`.

### Caching (`podimo/cache.py`)

Four caches backed by `diskcache` under `CACHE_DIR` (default `./cache`):
- `tokens_cache` — login tokens, TTL `TOKEN_CACHE_TIME` (5 days). Falls back to in-memory dict if `STORE_TOKENS_ON_DISK=false`.
- `podcast_cache` — full episode list per podcast, TTL `PODCAST_CACHE_TIME` (6 h). This determines how often new episodes appear.
- `head_cache` — `(content_length, content_type)` per episode ID, TTL `HEAD_CACHE_TIME` (7 days). Not deleted on expiry (see `getHeadEntry`'s `delete=False`).
- `cookie_jars` — per-user `aiohttp.CookieJar`, in-memory only.

Cache values are tuples of `(expiry_timestamp, value)`; see `getCacheEntry`/`insertCacheEntry`. Don't bypass these helpers.

### Anti-bot bypass (`podimo/client.py:post`)

Requests to `podimo.com/graphql` go through `cloudscraper` by default. Three mutually-exclusive overrides, checked in this order:
1. `SCRAPER_API` — rewrites the URL to ScraperAPI's proxy endpoint with `keep_headers=true`.
2. `ZENROWS_API` — replaces the `scraper` object with `ZenRowsClient` for this call.
3. `HTTP_PROXY` — set as `proxies['https']` on the cloudscraper session in `main.py:main`.

If a request starts failing with Cloudflare challenges, one of these probably needs configuring; cloudscraper alone is unreliable from datacenter IPs.

## Non-obvious patches on this fork

These are the local-only fixes; preserve them when resolving upstream merge conflicts.

- **`_arg()` in `main.py`** — accepts `amp;region` / `amp;locale` as fallbacks because some downstream tools (Audiobookshelf) consume the rendered HTML feed URL without decoding `&amp;`, so the param literally arrives prefixed with `amp;`.
- **`#.jpg` image fragment** — Podimo image URLs have no file extension (signed-query-string URLs), but feedgen / Apple's spec demand `.jpg`/`.png`. We append a `#.jpg` URL fragment; clients strip fragments before GET, so the actual fetch is unaffected. Applied in both `addFeedEntry` and `podcastsToRss`.
- **Region/locale defaults** — `split_username_region_locale` and the local-credentials path both default to `nl`/`nl-NL` when missing, instead of erroring. Older feed URLs were generated without these fields.

## Block list

`.block-list` (path overridable via `BLOCK_LIST_FILE`) is loaded once at import in `podimo/config.py`. Format: one token per line; lines starting with `#` are comments; only the first whitespace-separated token is used. If any entry is a substring of `request.url`, the feed returns `410 GONE`. See `.block-list.example`.
