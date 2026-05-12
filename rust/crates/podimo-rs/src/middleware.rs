//! After-request CORS + Cache-Control middleware. Mirrors `allow_cors` in `main.py`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;

use crate::config::SharedConfig;

pub async fn after_request(
    State(_config): State<SharedConfig>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();

    let mut response = next.run(req).await;
    let is_success = response.status().is_success();
    let headers = response.headers_mut();

    // CORS: only safe read-only fetches of /feed/* are cross-origin.
    if matches!(method, Method::GET | Method::HEAD) && path.starts_with("/feed/") {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, HEAD"),
        );
    }

    // Cache-Control: never cache /healthz; cache 2xx for 15 minutes; everything else no-store.
    let cc = if path == "/healthz" {
        "no-store"
    } else if is_success {
        "max-age=900"
    } else {
        "no-store"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));

    response
}
