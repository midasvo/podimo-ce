//! Boots the service on an ephemeral port and hits /healthz over real HTTP.

use std::net::SocketAddr;
use std::time::Duration;

use podimo_rs::{app, config::Config, AppState};
use tokio::net::TcpListener;

#[tokio::test]
async fn healthz_returns_200_json() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("CACHE_DIR", tmp.path());

    let config = Config::from_env().expect("config");
    let state = AppState::new(config).await.expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("addr");

    let router = app(state).await.expect("build app");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let url = format!("http://{addr}/healthz");
    let resp = client.get(&url).send().await.expect("request");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    assert_eq!(
        resp.headers().get(reqwest::header::CACHE_CONTROL).unwrap(),
        "no-store"
    );

    let body = resp.text().await.expect("body");
    assert_eq!(body, r#"{"status":"ok"}"#);

    handle.abort();
}
