//! Rust port of podimo-ce. See `MIGRATION_PLAN.md` at the repo root.

pub mod blocklist;
pub mod cache;
pub mod config;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod podimo;
pub mod state;
pub mod telemetry;
pub mod templates;
pub mod util;

use axum::Router;

pub use state::AppState;

/// Build the Axum app wired with shared state and middleware.
pub async fn app(state: AppState) -> anyhow::Result<Router> {
    let router = Router::new()
        .merge(handlers::healthz::router())
        .merge(handlers::index::router())
        .merge(handlers::feed::router())
        .fallback(handlers::not_found::fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::after_request,
        ))
        .with_state(state);

    Ok(router)
}
