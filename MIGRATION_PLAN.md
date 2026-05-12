# Rust Migration Plan: podimo-ce

Status: Phase 1 (discovery). Target branch: `rust-rewrite`. Python source stays in place
as the reference implementation; Rust code lands in `rust/` so both can coexist during
the migration.

## 1. The Python service today

### Runtime
- **Language**: Python 3.10 (Dockerfile pins `python:3.10-alpine`).
- **Web framework**: [Quart](https://quart.palletsprojects.com/) `~=0.20.0` (async-first
  Flask-compatible API).
- **Server**: [Hypercorn](https://hypercorn.readthedocs.io/) `~=0.17.3` (ASGI). Bound to
  `PODIMO_BIND_HOST` (default `127.0.0.1:12104`); `read_timeout=60`,
  `graceful_timeout=5`, `backlog=1000`.
- **Process model**: single-process, async, no worker pool. CPU is dominated by
  outbound network waits.
- **Entry point**: `python3 main.py` → `asyncio.run(main())` → `spawn_web_server()` →
  `hypercorn.asyncio.serve`.

### Route inventory

| Path                         | Method     | Handler                       | Auth         | Body in                              | Body out                                              | Status codes                  |
| ---------------------------- | ---------- | ----------------------------- | ------------ | ------------------------------------ | ----------------------------------------------------- | ----------------------------- |
| `/`                          | GET, POST  | `index`                       | none         | form: email, password, podcast_id, region, locale | rendered HTML (Jinja2)                    | 200                           |
| `/healthz`                   | GET        | `healthz`                     | none         | —                                    | `{"status":"ok"}` (`application/json`)                | 200                           |
| `/feed/<podcast_id>.xml`     | GET (HEAD) | `serve_basic_auth_feed`       | Basic (multi-user mode) or env vars (`LOCAL_CREDENTIALS=true`) | query: `region`, `locale` (or `amp;region`/`amp;locale`) | RSS 2.0 XML (`text/xml`) | 200, 400, 401, 404, 410, 500, 503 |
| (any unmatched)              | any        | `not_found`                   | —            | —                                    | text/plain example body                               | 404                           |

Cross-cutting `@app.after_request` (`allow_cors`):
- Sets `Access-Control-Allow-Origin: *` and `Access-Control-Allow-Methods: GET, HEAD`
  only on `GET`/`HEAD` requests to paths starting with `/feed/`.
- `/healthz` → `Cache-Control: no-store`.
- Other 2xx → `Cache-Control: max-age=900`.
- Non-2xx → `Cache-Control: no-store`.

Behavioural notes that must be preserved:
- Username field in HTTP Basic is overloaded as `email,region,locale`
  (`split_username_region_locale`); when only one part is present, defaults to
  `("nl", "nl-NL")`. Four-plus parts also fall back to defaults.
- `_arg(args, name)` accepts both `name` and `amp;name` for downstream consumers
  that don't decode `&amp;`.
- Podcast ID must match `[0-9a-fA-F\-]+` (full string).
- Region must be a known code (`nl, de, dk, es, latam, en, mx, no, fi, uk`).
- Locale must be in the enum
  (`nl-NL, de-DE, da-DK, es-ES, en-US, es-MX, no-NO, fi-FI, en-GB`).
- Any substring match between block-list tokens and `request.url` → `410 GONE`.
- Bad credentials (`ValueError` from `PodimoClient.__init__` or `podimoLogin`) → `401`
  with `WWW-Authenticate: Basic realm='Podimo credentials'`.
- Transient upstream errors during auth → `503 Upstream temporarily unavailable, please retry`.
- GraphQL "not found" error → `404`. Other errors during episode fetch → `500`.
- 401 body is plain text with a usage example; 404 body is plain text.

### External integrations
- **Podimo GraphQL** (`https://podimo.com/graphql`): three-step login dance
  (`tokenWithPreregisterUser` → `userOnboardingFlow` → `tokenWithCredentials`) and
  `podcastEpisodes`/`podcastById` query. All POSTs.
- **Cloudflare bypass** (exclusive, checked in this order):
  1. `SCRAPER_API` → URL rewrite `https://api.scraperapi.com?api_key=…&url=…&keep_headers=true`.
  2. `ZENROWS_API` → swap scraper for `ZenRowsClient` per call.
  3. `HTTP_PROXY` → set as `proxies['https']` on the cloudscraper session.
  - Default: `cloudscraper.create_scraper()`. Unreliable from datacenter IPs.
- **Episode media HEAD probes**: `aiohttp` `HEAD` to the audio CDN URL, 10 s timeout,
  3 attempts with exponential backoff (1 s, 2 s), populates enclosure
  `Content-Length` / `Content-Type` (with `audio/mpeg` fallback).

No database. No background workers, no scheduled jobs, no websockets, no streaming.

### Configuration surface (env + `.env` via `python-dotenv`)

| Var                       | Default              | Purpose                                                                |
| ------------------------- | -------------------- | ---------------------------------------------------------------------- |
| `PODIMO_HOSTNAME`         | `localhost:12104`    | Hostname shown in generated feed URLs.                                  |
| `PODIMO_BIND_HOST`        | `127.0.0.1:12104`    | Listen address.                                                         |
| `PODIMO_PROTOCOL`         | `http`               | Scheme in generated URLs.                                               |
| `HTTP_PROXY`              | `None`               | https proxy passed to cloudscraper.                                     |
| `ZENROWS_API`             | `None`               | API key, swaps scraper.                                                 |
| `SCRAPER_API`             | `None`               | API key, rewrites URL.                                                  |
| `CACHE_DIR`               | `./cache` (absolute) | Disk cache root.                                                        |
| `BLOCK_LIST_FILE`         | `./.block-list`      | Path of block list.                                                     |
| `DEBUG`                   | `false`              | Verbose request logs.                                                   |
| `LOCAL_CREDENTIALS`       | `false`              | Single-user mode: pull creds from env, not Basic auth.                  |
| `PODIMO_EMAIL`            | `None`               | Used only if `LOCAL_CREDENTIALS=true`.                                  |
| `PODIMO_PASSWORD`         | `None`               | Used only if `LOCAL_CREDENTIALS=true`.                                  |
| `STORE_TOKENS_ON_DISK`    | `true`               | If false, tokens are in-memory only.                                    |
| `TOKEN_CACHE_TIME`        | `432000` (5 d)       | Token TTL.                                                              |
| `PODCAST_CACHE_TIME`      | `21600` (6 h)        | Episode list TTL. Determines how often new episodes appear.            |
| `HEAD_CACHE_TIME`         | `604800` (7 d)       | Per-episode `(content_length, content_type)` TTL. Not deleted on expiry. |
| `PUBLIC_FEEDS`            | `false`              | If false, sets `itunes:block` on channel.                              |

Boolean coercion in Python: `lower() in ['true','1','t','y','yes']` — Rust must match.

### Caching
Four caches under `CACHE_DIR`, backed by `diskcache.Cache`:
- `tokens_cache` — login tokens. Falls back to in-memory `dict` if
  `STORE_TOKENS_ON_DISK=false`. Keyed by `sha256(username~password)`.
- `podcast_cache` — full episode list per podcast id.
- `head_cache` — `(content_length, content_type)` per episode id. `getHeadEntry` reads
  with `delete=False`, so expired entries stay on disk until overwritten.
- `cookie_jars` — in-memory `aiohttp.CookieJar` per user key.

Entries are tuples of `(expiry_timestamp, value)`. Always go through
`getCacheEntry` / `insertCacheEntry`.

### Block list
`.block-list` (path overridable via `BLOCK_LIST_FILE`). One token per line; `#` comments;
only the first whitespace-separated token is used. Loaded once at import time. Match is
substring against `request.url`, not equality.

### Observability
- Logging: `logging.basicConfig` with format
  `%(levelname)s | %(asctime)s | %(message)s` (timestamp ISO-8601 with `Z`), level INFO
  (DEBUG when `DEBUG=true`).
- Metrics: none.
- Tracing: none.
- Health: `GET /healthz` → 200 JSON.

### Tests
- Framework: `pytest` + `pytest-asyncio` (auto-mode), `pytest~=8.3`, `pytest-asyncio~=0.24`.
- Files:
  - `test_main_helpers.py` — `_arg`, `split_username_region_locale`.
  - `test_blocklist.py` — block-list loading.
  - `test_healthz_and_errors.py` — health endpoint and error mapping.
  - `test_feed_flow.py` — `/feed/<id>.xml` integration via Quart test client.
  - `test_feed_rendering.py` — `podcastsToRss` structural assertions.
  - `test_after_request_headers.py` — CORS + cache-control invariants.
  - `test_cache.py` — TTL semantics.
  - `test_utils.py`, `test_url_head_info.py` — helpers.
- No coverage tool configured. Lint: `ruff` (continue-on-error in CI).

### Deployment
- `Dockerfile`: `python:3.10-alpine`, installs `libxml2-dev libxslt-dev gcc libc-dev`
  (for `lxml`/`feedgen`), `pip install -r requirements.txt`, `ENTRYPOINT ["python3", "main.py"]`.
- Port: **12104** (must remain).
- Healthcheck path: **`/healthz`** (must remain).
- Images published to `ghcr.io/midasvo/podimo-ce` by `.github/workflows/docker-publish.yml`
  (multi-arch `linux/amd64,linux/arm64`).
- Production env vars: same as configuration surface above.

### Local-only patches on this fork
Must survive the migration:
- `_arg()` accepts `amp;<name>` fallbacks (Audiobookshelf consumes raw HTML feed URLs).
- Image URLs without `.jpg`/`.png` get a `#.jpg` URL fragment appended (clients strip
  fragments; feedgen / Apple require the extension).
- Region/locale default to `nl` / `nl-NL` when missing (older feed URLs lack them).

---

## 2. Chosen Rust stack

| Concern                  | Choice                                       | Justification                                                                                                                          |
| ------------------------ | -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Web framework            | **axum** (`0.7.x`)                           | Tokio-native, mature, integrates cleanly with `tower` / `tower-http` for the cross-cutting CORS + cache-control behaviour.            |
| Async runtime            | **tokio** (multi-thread, default features)   | The de facto choice; required by axum, reqwest, hyper.                                                                                |
| Outbound HTTP            | **reqwest** with `rustls-tls`                | Mature, async, JSON helpers, no OpenSSL build deps on Alpine.                                                                          |
| TLS                      | **rustls** (via reqwest)                     | Avoids OpenSSL toolchain in the Docker image.                                                                                          |
| Cloudflare bypass        | reqwest baseline + per-call URL rewrite for `SCRAPER_API`, per-call client swap for `ZENROWS_API`, `HTTPS_PROXY` honoured. See "Decisions". | Python's `cloudscraper` has no first-class Rust equivalent. The README already documents cloudscraper as unreliable from datacenter IPs and steers users to the API proxies. We preserve those proxy paths verbatim; the default scraper degrades to a plain `reqwest` client with Podimo's mobile UA. Optional later upgrade: `rquest` (TLS/HTTP fingerprint impersonation) — deferred until the plain client demonstrably fails. |
| Serialization            | **serde** + **serde_json**                   | Standard.                                                                                                                              |
| Validation               | Manual in handlers (mirrors the Python checks); `validator` crate not needed for this surface. | The validation surface is small (podcast-id regex, enum membership, basic-auth shape). A crate would be ceremony. |
| Caching (in-memory)      | **moka** (`0.12.x`)                          | Async TTL cache with the same expire-on-read semantics as `diskcache`'s tuple-with-timestamp trick.                                    |
| Caching (disk-backed)    | Custom thin layer over **fs** + bincode      | `diskcache` is a sharded sqlite-ish on-disk dict; an exact port is not necessary. We persist each `(expiry, value)` as a bincode blob under `<CACHE_DIR>/<cache_name>/<key>.bin`. On startup we hydrate `moka`. Simpler than pulling in `sled`/`redb`/`fjall` for three caches with shallow access patterns. |
| Templates                | **minijinja** (`2.x`)                        | Drop-in compatible with the existing Jinja2 templates. Zero rewriting needed.                                                          |
| RSS generation           | **rss** (`2.x`) with the iTunes extension    | Native `ITunesChannelExtension` and `ITunesItemExtension` cover `itunes:author`, `itunes:duration`, `itunes:block`, `itunes:image`.    |
| Configuration            | **figment** with env + dotenv providers      | Layered config matching the Python `{**dotenv_values, **os.environ}` semantics.                                                        |
| Tracing/logging          | **tracing** + **tracing-subscriber**         | Custom formatter matches the existing `LEVEL \| TIMESTAMP \| MESSAGE` format so log shippers don't need re-tuning.                     |
| Errors (domain)          | **thiserror**                                | Typed error variants for the `Auth*` / `Upstream*` / `Cache*` boundaries.                                                              |
| Errors (edges)           | **anyhow**                                   | For the `main` shim and ad-hoc fallibility in tests.                                                                                   |
| CORS / cache-control     | Hand-rolled `axum::middleware` matching the path-conditional Python `after_request`. | `tower-http`'s `CorsLayer` is global and unconditional. The Python rule is path-conditional (`/feed/*` only), which is easier and clearer to express inline. |
| Tests                    | `cargo test` + `axum::serve` on `:0` + `reqwest` for end-to-end; `tower::ServiceExt::oneshot` for unit-ish handler tests. | Stays in-tree; no extra harness.                                                                                                       |

### Cargo workspace layout

```
rust/
├── Cargo.toml          # workspace
├── rustfmt.toml
├── clippy.toml
└── crates/
    └── podimo-rs/      # single library + binary
        ├── Cargo.toml
        ├── src/
        │   ├── main.rs        # entrypoint
        │   ├── lib.rs         # `app()` factory + module re-exports
        │   ├── config.rs      # env + .env loading, defaults, REGIONS/LOCALES
        │   ├── error.rs       # AppError + IntoResponse
        │   ├── handlers/
        │   │   ├── mod.rs
        │   │   ├── index.rs   # GET/POST /
        │   │   ├── healthz.rs
        │   │   └── feed.rs    # GET /feed/<id>.xml
        │   ├── middleware.rs  # cors + cache-control after-request
        │   ├── podimo/
        │   │   ├── mod.rs
        │   │   ├── client.rs  # PodimoClient (login dance + getPodcasts)
        │   │   └── rss.rs     # podcastsToRss
        │   ├── cache.rs       # moka + on-disk persistence
        │   ├── blocklist.rs
        │   ├── templates.rs   # minijinja env + helpers
        │   └── util.rs        # token_key, randomHexId, randomFlyerId, email validation
        ├── templates/         # copied from ../templates (or symlinked at build)
        └── tests/
            ├── integration_healthz.rs
            ├── integration_feed.rs
            └── integration_index.rs
```

Decision: **single-crate workspace**. The brief suggests `api / domain / infra / migrations`,
but at ~700 LoC of Python with no database, splitting up-front is over-engineering. The
workspace skeleton is in place so we can split later if needed.

---

## 3. Migration order (Phase 3 preview)

Endpoints are ported in deployable order:

1. `/healthz` — proves the binding, middleware, and CI integration loop.
2. `GET /` (form rendering) — exercises minijinja and the `REGIONS`/`LOCALES` data.
3. `POST /` — pure form validation + URL formatting; no upstream calls.
4. 404 fallback — exercises error format parity.
5. `/feed/<id>.xml` (basic-auth path) — the bulk of the work:
   1. Basic-auth parsing + `split_username_region_locale`.
   2. Block-list short-circuit.
   3. `PodimoClient::login` (three-step GraphQL).
   4. `PodimoClient::get_podcasts` (paginated).
   5. `podcasts_to_rss` (feedgen → `rss` crate).
   6. `url_head_info` (HEAD probes with 3-retry exponential backoff).
6. `/feed/<id>.xml` (`LOCAL_CREDENTIALS=true`) — adds env-var credential path.

No background workers or websockets, so no second binary needed.

---

## 4. Deferred / open questions

- **Cloudflare bypass parity**: cloudscraper's exact challenge-solving isn't matched.
  Acceptable because (a) the README itself flags it as unreliable from datacenter IPs
  and (b) the `SCRAPER_API` / `ZENROWS_API` proxy paths are the recommended production
  configuration. Tracked under "Deferred" in MIGRATION_NOTES.md.
- **diskcache compatibility**: existing `./cache` directories from the Python service
  are *not* readable by the Rust port. Operators need to wipe `CACHE_DIR` on cutover.
  All three caches will repopulate (tokens within 5 days, podcasts within 6 hours,
  head within 7 days of access).
- **Logging format**: matched verbatim by default; JSON output is reserved for a
  follow-up if needed.
- **ruff/lint baseline**: equivalent is `clippy -D warnings`. Stricter than the
  Python side (where ruff runs with `continue-on-error`).
