//! Rust port of podimo-ce. See `MIGRATION_PLAN.md` at the repo root.

pub mod config;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod telemetry;

use std::sync::Arc;

use axum::Router;

pub use config::SharedConfig;

/// Build the Axum app wired with shared state and middleware.
pub async fn app(config: SharedConfig) -> anyhow::Result<Router> {
    let router = Router::new()
        .merge(handlers::healthz::router())
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&config),
            middleware::after_request,
        ))
        .with_state(config);

    Ok(router)
}
