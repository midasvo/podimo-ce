//! Tests for `podimo::head::url_head_info`. Mirrors `tests/test_url_head_info.py`.

use std::time::Duration;

use podimo_rs::cache::{HeadInfo, TtlCache};
use podimo_rs::podimo::head::url_head_info;
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn empty_head_cache() -> TtlCache<HeadInfo> {
    TtlCache::new("head_test", None, Duration::from_secs(60)).await
}

#[tokio::test]
async fn missing_content_length_returns_string_zero() {
    // Python: test_missing_content_length_returns_string_zero.
    // A HEAD response with no Content-Length must surface as the string "0" so
    // the RSS enclosure builder doesn't crash on a non-string length.
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/episode.mp3"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let cache = empty_head_cache().await;
    let client = Client::new();
    let info = url_head_info(
        &client,
        &cache,
        "ep-fresh",
        &format!("{}/episode.mp3", server.uri()),
        "nl-NL",
    )
    .await
    .expect("HEAD");
    assert_eq!(info.content_length, "0");
}

#[tokio::test]
async fn content_length_from_header_is_passed_through() {
    // Python: test_content_length_from_header_is_passed_through.
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/x.mp3"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "9876")
                .insert_header("Content-Type", "audio/mpeg"),
        )
        .mount(&server)
        .await;

    let cache = empty_head_cache().await;
    let client = Client::new();
    let info = url_head_info(
        &client,
        &cache,
        "ep-with-length",
        &format!("{}/x.mp3", server.uri()),
        "nl-NL",
    )
    .await
    .expect("HEAD");
    assert_eq!(info.content_length, "9876");
    // mime_guess from path returns "audio/mpeg" for .mp3, which wins over the
    // response header. Either way, it must be audio/mpeg.
    assert_eq!(info.content_type, "audio/mpeg");
}

#[tokio::test]
async fn cached_head_info_short_circuits_network() {
    // If the cache already has a valid entry, no HTTP request is issued. Verify
    // by pointing at a URL that would otherwise fail (no server listening) and
    // confirming we still get the cached value.
    let cache = empty_head_cache().await;
    cache
        .insert(
            "ep-cached".to_string(),
            HeadInfo {
                content_length: "42".into(),
                content_type: "audio/mpeg".into(),
            },
        )
        .await;
    let info = url_head_info(
        &Client::new(),
        &cache,
        "ep-cached",
        "http://127.0.0.1:1/never-called",
        "nl-NL",
    )
    .await
    .expect("cached");
    assert_eq!(info.content_length, "42");
}
