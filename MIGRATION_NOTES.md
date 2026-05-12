# Rust Migration Notes

Companion to [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md). Captures the non-obvious
decisions, deviations from Python behaviour, and known gaps.

## Decisions

### Workspace shape
- Single-crate workspace under `rust/` (`crates/podimo-rs/`). The brief proposed
  `api / domain / infra / migrations`, but the service is ~700 LoC of Python with
  no database and no obvious split. One crate keeps the dependency graph flat and
  build times short; the workspace skeleton is in place so we can split later if
  the surface grows.

### Web framework: axum 0.7
- Mature, Tokio-native, tower-compatible. Axum 0.8 introduces `{name}` path-param
  syntax (and other small API changes); we hit the `:name` vs `{name}` divergence
  during scaffolding — tests caught it before commit. Pinning to the 0.7 series
  for now; an axum 0.8 bump can be a follow-up.

### TLS: rustls-only
- `reqwest` is built with `rustls-tls` (no `default-features`). Avoids dragging
  OpenSSL into the Docker build, which keeps the runtime image small and the
  builder fast.

### Cache: moka + bincode-on-disk
- `diskcache` is a sharded, sqlite-backed disk cache; an exact format port is
  unnecessary. We use `moka` for in-memory TTL eviction and shadow each entry to
  `<CACHE_DIR>/<cache_name>/<key>.bin` as bincode `(expiry_unix_seconds, value)`.
  On startup, entries are lazily hydrated on first read.
- `getHeadEntry(delete=False)` is mapped to a `get_no_expire` method that
  returns stale-but-present entries, mirroring the Python behaviour where the
  HEAD cache survives upstream blips.
- **Deviation**: existing `./cache/` directories from the Python service are
  **not** readable by the Rust port. Operators need to wipe `CACHE_DIR` on
  cutover; the three caches repopulate within their TTL windows.

### Templates: minijinja with `include_str!`
- minijinja is Jinja2-compatible; the existing templates (`base.html`,
  `index.html`, `feed_location.html`) work unmodified after a straight copy
  into `rust/crates/podimo-rs/templates/`. Templates are embedded at compile
  time via `include_str!`, so the runtime container needs no external files
  (we still ship `templates/` in the image for inspection convenience).

### RSS: `rss` crate with iTunes extensions
- Native `ITunesChannelExtensionBuilder` / `ITunesItemExtensionBuilder` covers
  `itunes:author`, `itunes:duration`, `itunes:image`, `itunes:block`. Channel
  namespaces are set explicitly (`xmlns:itunes`) to mirror feedgen's output.
- **Deviation**: feedgen produces *pretty-printed* XML by default; the `rss`
  crate emits compact XML. Field ordering inside `<item>` may also differ. RSS
  parsers should not care, but byte-level diffs against the Python output will
  not be a parity assertion.

### Cloudflare bypass
- Python uses `cloudscraper` by default with optional `SCRAPER_API`, `ZENROWS_API`,
  or `HTTP_PROXY` overrides.
- Rust uses `reqwest` baseline + the same three override hooks; the default path
  does not do JS challenge solving.
- **Why this is acceptable**: the existing README/CLAUDE.md and `.env.example`
  already document cloudscraper as unreliable from datacenter IPs and steer users
  to ScraperAPI / ZenRows / a residential HTTP_PROXY. Production setups use one
  of those three; the cloudscraper layer is essentially a "best-effort dev mode."
- A follow-up could integrate `rquest` (TLS/HTTP-fingerprint impersonation) if
  the plain reqwest client demonstrably fails for users who didn't configure a
  proxy. Tracked under "Deferred".

### Tracing format
- Default formatter emits `LEVEL | YYYY-MM-DDThh:mm:ssZ | message` to match the
  Python `logging.basicConfig` format byte-for-byte, so log shippers don't need
  re-tuning. Set `PODIMO_LOG_JSON=true` for JSON output.

### Error → HTTP mapping
- Maintained verbatim from `main.py`:
  - Bad credentials (or `PodimoClient::new` validation failure) → 401 with
    `WWW-Authenticate: Basic realm='Podimo credentials'` and the same plain-text
    example body.
  - Upstream failure during auth → 503 `Upstream temporarily unavailable, please retry`.
  - GraphQL error containing "not found" (case-insensitive) → 404.
  - Other upstream errors during fetch → 500.
  - Block-list substring match → 410.
  - Invalid podcast-id / region / locale → 400 with the same plain-text reasons.

### Pydantic-style coercion
- Python uses `bool(str(v).lower() in ['true','1','t','y','yes'])`. The Rust
  config layer mirrors that loose coercion for `DEBUG`, `LOCAL_CREDENTIALS`,
  `STORE_TOKENS_ON_DISK`, `PUBLIC_FEEDS`. Anything else (typos, empty strings)
  falls back to the documented default.

### `/feed/:podcast_id.xml` route shape
- Axum 0.7's path matcher treats `.xml` as a literal-suffix problem. The route
  is registered as `/feed/:podcast_id` and the handler strips the `.xml` suffix
  in-handler; missing suffixes fall through to the 404 fallback.

### `.gitignore`
- Repo had no Rust ignores; `rust/target/`, `.pytest_cache/`, `.ruff_cache/`, and
  `.claude/` (Claude Code's local-only worktree state) are now ignored.

## Deviations from Python behaviour

- **Disk cache format**: not on-wire compatible with `diskcache`; wipe `CACHE_DIR`
  on cutover (see above).
- **RSS body bytes**: not byte-for-byte identical with feedgen's output; structure
  and semantics are preserved (RSS 2.0 + iTunes namespace + same channel/item
  fields). Test assertions should be structural (XPath / element presence), not
  exact-string.
- **Default scraper**: no JS challenge solving (see "Cloudflare bypass" above).

## Deferred

Grep the codebase for `TODO(migration):` to find these in-line.

- **Cloudflare challenge solving without an API proxy**: the default `reqwest`
  client doesn't impersonate a browser; users running from datacenter IPs without
  `SCRAPER_API` / `ZENROWS_API` / `HTTP_PROXY` may get blocked. Long-term: try
  `rquest` (Chrome/Firefox JA3+HTTP2 impersonation). Short-term: docs are already
  honest about this in `.env.example`.
- **Per-user cookie jars**: Python keeps an `aiohttp.CookieJar` per user keyed by
  `sha256(username~password)`. The Rust `reqwest::Client` is process-wide with a
  shared cookie store. Investigate whether Podimo's GraphQL endpoint actually
  relies on per-user cookies; if so, switch to a `cookie_store::CookieStoreMutex`
  keyed by `client.key` so requests for user A can't leak cookies into user B's
  session.
- **Token cache hydration on startup**: each `TtlCache<V>` is lazily hydrated on
  the first `get`; this is fine for correctness but means a cold start
  re-authenticates against Podimo on the very first request after a restart. A
  background hydration pass on `AppState::new` would warm the cache.
- **Full RSS byte-parity tests**: structural tests cover the routing and
  validation surface; a contract test that boots both services and diffs RSS
  bodies (with XML canonicalization) is good for Phase 4 but not yet wired up.

## Container publishing

The Rust image publishes as a *parallel* image to the Python one, not a
replacement:

| Service | Image                              | Workflow                                  |
| ------- | ---------------------------------- | ----------------------------------------- |
| Python  | `ghcr.io/midasvo/podimo-ce`        | `.github/workflows/docker-publish.yml`    |
| Rust    | `ghcr.io/midasvo/podimo-rs`        | `.github/workflows/docker-publish-rs.yml` |

The Rust workflow is gated on `paths: ['rust/**']`, so Python-only changes
don't fire it, and only triggers from `main` / `v*.*.*` tags / manual dispatch.
Nothing publishes from `rust-rewrite` automatically; the PR can merge into
`main` without surprising consumers of the existing Python image.

Binary in the image is `/usr/local/bin/podimo-rs`. Locally: `cargo run --bin podimo-rs`.

## Verification log

Run from `rust/`:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
docker build -t podimo-rs:test .
docker run --rm -p 12104:12104 podimo-rs:test
curl http://127.0.0.1:12104/healthz   # → 200 {"status":"ok"}
```

## Cutover checklist

When promoting the Rust port to production:

1. Land all Phase 3 endpoints; ensure `cargo test` is green on the rust-rewrite branch.
2. Wipe the existing `CACHE_DIR` on the target host (diskcache → bincode format
   change). Tokens repopulate on first login; podcasts within 6h; head within 7d.
3. Update `docker-publish.yml` (or add a parallel workflow) to publish from
   `rust/Dockerfile`. Tag scheme stays `vX.Y.Z-midasvo.N`.
4. Sanity-check `/healthz` in staging; then a side-by-side run of the Python and
   Rust services pointed at the same Podimo creds, comparing RSS bodies for a
   handful of real podcast ids.
5. Roll forward; keep `main.py` as the reference implementation. The Python tree
   stays in the repo for diffability and rollback.
