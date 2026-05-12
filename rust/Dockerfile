# syntax=docker/dockerfile:1.7
# Multi-stage build. Runtime image exposes port 12104 and the /healthz
# healthcheck path, matching the Python service it replaces.

FROM rust:1.95-slim-bookworm AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock rustfmt.toml clippy.toml ./
COPY crates ./crates

RUN cargo build --release --locked --bin podimo-rs

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/podimo-rs /usr/local/bin/podimo-rs
COPY crates/podimo-rs/templates ./templates

ENV PODIMO_BIND_HOST=0.0.0.0:12104
EXPOSE 12104

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -q -O - http://127.0.0.1:12104/healthz | grep -q '"ok"' || exit 1

ENTRYPOINT ["/usr/local/bin/podimo-rs"]
