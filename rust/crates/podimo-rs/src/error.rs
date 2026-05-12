//! Application error types and `IntoResponse` implementations.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

/// Top-level application error.
#[derive(Debug, Error)]
pub enum AppError {
    /// Bad credentials (mirrors Python's `ValueError` in the auth path → 401).
    #[error("invalid credentials")]
    Unauthorized,

    /// Upstream transiently unavailable (network failure, Cloudflare block, GraphQL down) → 503.
    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),

    /// Podcast not found upstream → 404.
    #[error("podcast not found")]
    NotFound,

    /// Catch-all for unexpected errors → 500.
    #[error("internal error: {0}")]
    Internal(String),

    /// Bad request payload → 400 with a plain-text reason.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// On the block list → 410.
    #[error("podcast is gone")]
    Gone,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized => {
                // Called from contexts without AppState (no live hostname); use the
                // hostname-less variant of the example body.
                crate::handlers::feed::unauthorized_response("localhost:12104")
            }
            AppError::UpstreamUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Upstream temporarily unavailable, please retry",
            )
                .into_response(),
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "Podcast not found. Are you sure you have the correct ID?",
            )
                .into_response(),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            AppError::Gone => (StatusCode::GONE, "Podcast is gone").into_response(),
            AppError::Internal(msg) => {
                tracing::error!(target: "podimo", "internal error: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong while fetching the podcasts",
                )
                    .into_response()
            }
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}
