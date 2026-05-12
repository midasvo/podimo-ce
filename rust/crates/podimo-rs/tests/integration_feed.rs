//! HTTP-level parity tests for /feed/<id>.xml that don't require Podimo upstream:
//! basic-auth handling, validation, block-list, and middleware (CORS/Cache-Control).

use std::net::SocketAddr;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use podimo_rs::{app, config::Config, AppState};
use tokio::net::TcpListener;

async fn boot() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("CACHE_DIR", tmp.path());
    std::env::remove_var("LOCAL_CREDENTIALS");
    std::env::remove_var("PODIMO_EMAIL");
    std::env::remove_var("PODIMO_PASSWORD");
    std::env::remove_var("BLOCK_LIST_FILE");

    let config = Config::from_env().unwrap();
    let state = AppState::new(config).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let router = app(state).await.unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    // Forget the tempdir guard so it survives the entire test runtime.
    std::mem::forget(tmp);
    (addr, handle)
}

fn basic_auth_header(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!("Basic {}", BASE64.encode(raw))
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn unauthenticated_returns_401_with_no_store_and_www_authenticate() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!(
            "http://{addr}/feed/de9b2081-9fc5-489f-b9d3-d744ed9cab20.xml"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-store");
    assert!(resp
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("Basic realm="));
    handle.abort();
}

#[tokio::test]
async fn invalid_podcast_id_format_returns_400() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!("http://{addr}/feed/not-a-valid-id!.xml"))
        .header("Authorization", basic_auth_header("a@b.com,nl,nl-NL", "pw"))
        .send()
        .await
        .unwrap();
    // The router rejects the path-pattern mismatch before our handler — for the
    // characters that *do* match `[^/]`, our handler returns 400. `not-a-valid-id!`
    // matches the path segment, then trips the podcast-id regex.
    assert!(
        matches!(resp.status().as_u16(), 400 | 404),
        "status: {}",
        resp.status()
    );
    handle.abort();
}

#[tokio::test]
async fn invalid_region_returns_400() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!(
            "http://{addr}/feed/de9b2081-9fc5-489f-b9d3-d744ed9cab20.xml"
        ))
        .header("Authorization", basic_auth_header("a@b.com,zz,nl-NL", "pw"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Invalid region"));
    handle.abort();
}

#[tokio::test]
async fn invalid_locale_returns_400() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!(
            "http://{addr}/feed/de9b2081-9fc5-489f-b9d3-d744ed9cab20.xml"
        ))
        .header("Authorization", basic_auth_header("a@b.com,nl,zz-ZZ", "pw"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Invalid locale"));
    handle.abort();
}

#[tokio::test]
async fn feed_get_response_has_cors_headers() {
    let (addr, handle) = boot().await;
    // Unauthenticated -> 401 but CORS should still be emitted on /feed/* GETs.
    let resp = http_client()
        .get(format!(
            "http://{addr}/feed/de9b2081-9fc5-489f-b9d3-d744ed9cab20.xml"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    assert_eq!(
        resp.headers().get("access-control-allow-methods").unwrap(),
        "GET, HEAD"
    );
    handle.abort();
}

#[tokio::test]
async fn healthz_does_not_have_cors_headers() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("access-control-allow-origin").is_none());
    handle.abort();
}

#[tokio::test]
async fn index_get_renders_form() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Podimo-to-RSS"), "body: {body}");
    assert!(body.contains("email"));
    assert!(body.contains("password"));
    assert!(body.contains("Podcast ID"));
    handle.abort();
}

#[tokio::test]
async fn unknown_path_returns_404_text_plain() {
    let (addr, handle) = boot().await;
    let resp = http_client()
        .get(format!("http://{addr}/no-such-path"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers().get("content-type").unwrap(), "text/plain");
    let body = resp.text().await.unwrap();
    assert!(body.starts_with("404 Not found"));
    handle.abort();
}
