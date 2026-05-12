//! 404 fallback. Mirrors Quart's `@app.errorhandler(404)` body.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;

use crate::state::AppState;

pub async fn fallback(State(state): State<AppState>) -> impl IntoResponse {
    let body = format!(
        "404 Not found.\n\n{}",
        example_block(&state.config.hostname)
    );
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain")],
        body,
    )
}

pub fn example_block(hostname: &str) -> String {
    format!(
        "Example\n\
------------\n\
Username: example@example.com\n\
Password: this-is-my-password\n\
Podcast ID: 12345-abcdef\n\n\
The URL will be\n\
https://example%40example.com:this-is-my-password@{hostname}/feed/12345-abcdef.xml\n\n\
Note that the username and password should be URL encoded. This can be done with\n\
a tool like https://gchq.github.io/CyberChef/#recipe=URL_Encode(true)\n"
    )
}
