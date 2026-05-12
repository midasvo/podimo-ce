//! Feed endpoint stubs. Full implementation lands in Phase 3.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

const UNAUTHORIZED_BODY: &str = "401 Unauthorized.\n\
You need to login with the correct credentials for Podimo.\n\n\
Example\n\
------------\n\
Username: example@example.com\n\
Password: this-is-my-password\n\
Podcast ID: 12345-abcdef\n\n\
The URL will be\n\
https://example%40example.com:this-is-my-password@<hostname>/feed/12345-abcdef.xml\n\n\
Note that the username and password should be URL encoded. This can be done with\n\
a tool like https://gchq.github.io/CyberChef/#recipe=URL_Encode(true)\n";

pub fn unauthorized_response() -> Response {
    let mut resp = (StatusCode::UNAUTHORIZED, UNAUTHORIZED_BODY).into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm='Podimo credentials'"),
    );
    resp
}
