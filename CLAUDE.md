# CLAUDE.md

Guidance for Claude Code working on this repository.

## What this is

Self-hosted RSS proxy for Podimo. Rust service: axum on tokio, reqwest for
outbound, moka + bincode for caches, minijinja for the form templates, the
`rss` crate for output. Single binary `podimo-rs` listens on port 12104.

History: this was a Python service (Quart on Hypercorn), itself a fork of
`ThijsRay/podimo`. The Rust rewrite landed in PR #2 and replaced Python at the
root in PR #3. The Python source isn't in the tree anymore — `git log` from
before the cutover is the only reference. The HTTP contract was preserved,
including the fork-local patches (`amp;`-prefix query fallback, `#.jpg` image
fragment, `nl`/`nl-NL` region/locale defaults).

## Running locally

```sh
cp .env.example .env       # then edit; DEBUG=true gives verbose request logs
cargo run --bin podimo-rs  # binds PODIMO_BIND_HOST (default 127.0.0.1:12104)
```

Tests + lints:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all --locked
```

Docker build matches CI: `docker build -t podimo-rs:test .` then
`docker run -p 12104:12104 podimo-rs:test`.

## Architecture

```
.
├── Cargo.toml                       # workspace
├── crates/
│   └── podimo-rs/
│       ├── Cargo.toml               # package
│       ├── src/
│       │   ├── main.rs              # entrypoint, signal handling, listener
│       │   ├── lib.rs               # `app()` factory + module re-exports
│       │   ├── config.rs            # env+dotenv loading; REGIONS/LOCALES
│       │   ├── error.rs             # AppError + IntoResponse
│       │   ├── state.rs             # AppState (config, caches, blocklist, scraper, templates)
│       │   ├── handlers/
│       │   │   ├── healthz.rs       # GET /healthz
│       │   │   ├── index.rs         # GET/POST /
│       │   │   ├── feed.rs          # GET /feed/<id>.xml
│       │   │   └── not_found.rs     # fallback
│       │   ├── middleware.rs        # after-request CORS + Cache-Control
│       │   ├── podimo/
│       │   │   ├── client.rs        # GraphQL login + getPodcasts
│       │   │   ├── head.rs          # episode HEAD probe with retries
│       │   │   └── rss.rs           # podcasts_to_rss
│       │   ├── cache.rs             # TtlCache + on-disk bincode persistence
│       │   ├── blocklist.rs
│       │   ├── templates.rs         # minijinja env (templates embedded with include_str!)
│       │   └── util.rs              # token_key, randomHexId, randomFlyerId, email validation, _arg
│       ├── templates/
│       │   ├── base.html
│       │   ├── index.html
│       │   └── feed_location.html
│       └── tests/
│           ├── integration_healthz.rs
│           ├── integration_feed.rs           # validation + middleware parity
│           ├── integration_feed_upstream.rs  # wiremock-mocked GraphQL
│           ├── integration_head.rs           # wiremock-mocked HEAD probe
│           └── integration_rss.rs            # structural RSS rendering
```

Feed request flow:

1. `handlers/feed.rs::serve` resolves credentials. Two modes selected by
   `LOCAL_CREDENTIALS`:
   - **off (multi-user)**: HTTP Basic. The username field is overloaded as
     `email,region,locale` (comma-separated, URL-encoded). See
     `util::split_username_region_locale`; defaults to `nl`/`nl-NL` if the
     comma fields are missing.
   - **on (single-user)**: creds from `PODIMO_EMAIL`/`PODIMO_PASSWORD`;
     region/locale come from the query string with the same defaults.
2. Validation: podcast id regex (`[0-9a-fA-F-]+`), region/locale enum
   membership, block-list substring match (`410` if hit).
3. `PodimoClient::login` does the three-step GraphQL dance:
   `tokenWithPreregisterUser` → `userOnboardingFlow` → `tokenWithCredentials`.
   The final token is cached under `sha256(username~password)` (see
   `util::token_key`).
4. `PodimoClient::get_podcasts` pages through `podcastEpisodes` 100 at a time
   and returns the raw GraphQL payload.
5. `podimo::rss::podcasts_to_rss` builds RSS via the `rss` crate's iTunes
   extensions and runs `url_head_info` HEAD probes for enclosure metadata in
   chunks of 5 concurrent requests.

### Caching (`crates/podimo-rs/src/cache.rs`)

Three caches under `CACHE_DIR` (default `./cache`):

- `tokens_cache` — login tokens, TTL `TOKEN_CACHE_TIME` (5 days). In-memory
  only if `STORE_TOKENS_ON_DISK=false`; otherwise shadowed to disk.
- `podcast_cache` — full GraphQL payload per podcast, TTL `PODCAST_CACHE_TIME`
  (6 h). Determines how often new episodes appear in the feed.
- `head_cache` — `HeadInfo { content_length, content_type }` per episode id,
  TTL `HEAD_CACHE_TIME` (7 days). Read via `get_no_expire` so the on-disk
  record survives expiry (we always re-probe HEAD on expiry, but the historical
  record is kept for inspection).

`TtlCache<V>` wraps `moka::future::Cache` for in-memory TTL eviction and
shadows each entry as a bincode blob under `<CACHE_DIR>/<name>/<key>.bin`. On
expiry, reads return `None` and a fresh fetch repopulates. **The on-disk format
is not compatible with the old Python `diskcache`**; wipe `CACHE_DIR` on
cutover.

### Anti-bot bypass (`crates/podimo-rs/src/podimo/client.rs::post_graphql`)

Requests to `podimo.com/graphql` go through `reqwest` with no special browser
fingerprinting. Three mutually-exclusive overrides, checked in this order:

1. `SCRAPER_API` — URL is rewritten through ScraperAPI's `api.scraperapi.com`
   proxy with `keep_headers=true`.
2. `ZENROWS_API` — URL is rewritten through ZenRows' `api.zenrows.com` proxy.
3. `HTTP_PROXY` — used as the `https` proxy on the `reqwest::Client`.

The base case (no override) is plain `reqwest` and is unreliable from
datacenter IPs — Cloudflare will challenge it. Production deployments on VPSes
should set one of the three. Possible follow-up: add `rquest` (TLS+HTTP2
fingerprint impersonation) for a better default, but not urgent.

API-key safety: error messages from `post_graphql` only include the target
**host**, never the formatted proxy URL — the SCRAPER_API/ZENROWS_API key never
leaks into logs.

## Non-obvious carry-overs from the Python service

Preserved verbatim:

- **`util::amp_arg`** accepts `amp;region` / `amp;locale` as fallbacks because
  some downstream tools (e.g. Audiobookshelf) consume the rendered HTML feed
  URL without decoding `&amp;`, so the param arrives prefixed with `amp;`.
- **`#.jpg` image fragment** — Podimo image URLs are signed query strings with
  no file extension, but the `rss` crate / Apple's spec demand `.jpg`/`.png`.
  We append `#.jpg`; clients strip fragments before GET. Applied in both
  channel-level and per-item images in `podimo::rss`.
- **Region/locale defaults** — `util::split_username_region_locale` and the
  `LOCAL_CREDENTIALS` query-string path both default to `nl`/`nl-NL` when the
  fields are missing.

## Block list

`.block-list` (path overridable via `BLOCK_LIST_FILE`) is loaded by
`AppState::new`. Format: one token per line; lines starting with `#` are
comments; only the first whitespace-separated token is used. If any entry is a
substring of the request URI (path+query), the feed returns `410 GONE`. See
`.block-list.example`.

## Logging

`tracing` + `tracing-subscriber` with a custom line formatter that mirrors the
Python service's `LEVEL | YYYY-MM-DDThh:mm:ssZ | message` shape — log shippers
don't need re-tuning. `RUST_LOG` controls level; `PODIMO_LOG_JSON=true` swaps
to structured JSON output.

## Container

Multi-stage Dockerfile: `rust:1.95-slim-bookworm` builder → `debian:bookworm-slim`
runtime, static-linked-ish via rustls (no OpenSSL dependency). Binary lives at
`/usr/local/bin/podimo-rs`, exposes port 12104, `HEALTHCHECK` hits `/healthz`.

CI publishes `ghcr.io/midasvo/podimo-rs` from `main` pushes, semver tags, or
manual dispatch. Multi-arch (linux/amd64, linux/arm64).

## Deferred items

(Kept short; full list in `MIGRATION_NOTES.md`.)

- **Cloudflare challenge solving without an API proxy** — see
  `client.rs::post_graphql`. Mitigated by ScraperAPI/ZenRows/HTTP_PROXY.
- **Per-user cookie jars** — `reqwest::Client` has a process-wide jar; Python
  kept one per `token_key`. Probably fine; verify only if Podimo's GraphQL
  endpoint actually relies on per-user cookies.
- **Cache hydration on startup** — `TtlCache` is lazily hydrated on first read;
  a cold start re-authenticates against Podimo on the very first request.
- **Side-by-side production test** — never validated against the live Podimo
  upstream; only against wiremock fakes. Worth a one-off comparison before
  treating any new behaviour as authoritative.
