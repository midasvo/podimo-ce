//! Structural snapshot tests for `podimo::rss::podcasts_to_rss`.
//!
//! Assertions are substring-based so they don't couple to attribute ordering,
//! whitespace, or generator metadata. The HEAD probe is pre-empted by
//! pre-populating the head cache, so the test stays offline.

use std::time::Duration;

use podimo_rs::cache::{HeadInfo, TtlCache};
use podimo_rs::podimo::rss::podcasts_to_rss;
use reqwest::Client;
use serde_json::json;

fn fixed_payload() -> serde_json::Value {
    json!({
        "podcast": {
            "title": "Test Show",
            "description": "A deterministic test show",
            "webAddress": serde_json::Value::Null,
            "authorName": "Author",
            "language": "nl",
            "images": { "coverImageUrl": "https://example.com/cover.jpg" }
        },
        "episodes": [
            {
                "id": "ep1",
                "title": "Episode 1",
                "description": "First episode body",
                "publishDatetime": "2024-01-01T12:00:00Z",
                "datetime": "2024-01-01T12:00:00Z",
                "imageUrl": "https://example.com/ep1.jpg",
                "audio": { "url": "https://example.com/ep1.mp3", "duration": 1234 },
                "streamMedia": serde_json::Value::Null,
                "artist": "Author",
                "podcastName": "Test Show"
            },
            {
                "id": "ep2",
                "title": "Episode 2",
                "description": "Second episode body",
                "publishDatetime": "2024-01-02T12:00:00Z",
                "datetime": "2024-01-02T12:00:00Z",
                "imageUrl": "https://example.com/ep2.jpg",
                "audio": { "url": "https://example.com/ep2.mp3", "duration": 5678 },
                "streamMedia": serde_json::Value::Null,
                "artist": "Author",
                "podcastName": "Test Show"
            }
        ]
    })
}

async fn stub_head_cache_for(
    episode_ids: &[&str],
    length: &str,
    ctype: &str,
) -> TtlCache<HeadInfo> {
    let cache: TtlCache<HeadInfo> = TtlCache::new("head_test", None, Duration::from_secs(60)).await;
    for id in episode_ids {
        cache
            .insert(
                (*id).to_string(),
                HeadInfo {
                    content_length: length.into(),
                    content_type: ctype.into(),
                },
            )
            .await;
    }
    cache
}

#[tokio::test]
async fn podcasts_to_rss_renders_expected_structure() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "12345", "audio/mpeg").await;
    let scraper = Client::new();

    let rss = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        false,
        None,
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");

    // Top-level RSS skeleton.
    assert!(rss.contains("<rss"));
    assert!(rss.contains("<channel>"));
    assert!(rss.contains("</channel>"));
    assert!(rss.contains("</rss>"));

    // Channel metadata.
    assert!(rss.contains("<title>Test Show</title>"));
    assert!(rss.contains("<description>A deterministic test show</description>"));
    assert!(rss.contains("<language>nl</language>"));
    assert!(rss.contains("<itunes:author>Author</itunes:author>"));
    assert!(rss.contains("https://podimo.com/shows/podcast-uuid"));

    // Two items.
    assert_eq!(rss.matches("<item>").count(), 2, "want 2 <item>: {rss}");
    assert_eq!(rss.matches("</item>").count(), 2);
    assert!(rss.contains("<title>Episode 1</title>"));
    assert!(rss.contains("<title>Episode 2</title>"));

    // GUIDs come through unchanged.
    assert!(rss.contains("<guid"));
    assert!(rss.contains("ep1"));
    assert!(rss.contains("ep2"));

    // itunes:duration carried through for each episode.
    assert!(rss.contains("<itunes:duration>1234</itunes:duration>"));
    assert!(rss.contains("<itunes:duration>5678</itunes:duration>"));

    // Enclosures rendered for both episodes with the head-info metadata.
    assert!(rss.contains("https://example.com/ep1.mp3"));
    assert!(rss.contains("https://example.com/ep2.mp3"));
    assert!(rss.contains("audio/mpeg"));
    assert!(rss.contains("12345"));
}

#[tokio::test]
async fn podcasts_to_rss_appends_jpg_fragment_to_extensionless_image_urls() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "0", "audio/mpeg").await;
    let scraper = Client::new();

    let mut payload = fixed_payload();
    payload["podcast"]["images"]["coverImageUrl"] =
        json!("https://images.podimo.com/cover?sig=abcdef");
    payload["episodes"][0]["imageUrl"] = json!("https://images.podimo.com/ep1?sig=xyz");
    payload["episodes"][1]["imageUrl"] = json!("https://images.podimo.com/ep2?sig=qrs");

    let rss = podcasts_to_rss(
        &payload,
        "podcast-uuid",
        "nl-NL",
        false,
        None,
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");

    assert!(
        rss.contains("https://images.podimo.com/cover?sig=abcdef#.jpg"),
        "channel cover should have #.jpg appended: {rss}"
    );
    assert!(
        rss.contains("https://images.podimo.com/ep1?sig=xyz#.jpg"),
        "ep1 image should have #.jpg appended: {rss}"
    );
    assert!(
        rss.contains("https://images.podimo.com/ep2?sig=qrs#.jpg"),
        "ep2 image should have #.jpg appended: {rss}"
    );
    // Sanity: never append a second fragment to an already-extensioned URL.
    assert!(!rss.contains("ep1.jpg#.jpg"));
}

#[tokio::test]
async fn podcasts_to_rss_preserves_existing_jpg_extension() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "0", "audio/mpeg").await;
    let scraper = Client::new();

    let rss = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        false,
        None,
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");

    assert!(rss.contains("https://example.com/cover.jpg"));
    assert!(!rss.contains("https://example.com/cover.jpg#.jpg"));
    assert!(rss.contains("https://example.com/ep1.jpg"));
    assert!(!rss.contains("https://example.com/ep1.jpg#.jpg"));
}

#[tokio::test]
async fn podcasts_to_rss_limits_to_n_newest_episodes() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "0", "audio/mpeg").await;
    let scraper = Client::new();

    let rss = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        false,
        Some(1),
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");

    // Episodes arrive PUBLISHED_DESCENDING; limit=1 keeps the head of the slice.
    assert_eq!(rss.matches("<item>").count(), 1, "want 1 <item>: {rss}");
    assert!(rss.contains("<title>Episode 1</title>"));
    assert!(!rss.contains("<title>Episode 2</title>"));
}

#[tokio::test]
async fn podcasts_to_rss_limit_larger_than_episode_count_is_noop() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "0", "audio/mpeg").await;
    let scraper = Client::new();

    let rss = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        false,
        Some(999),
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");

    assert_eq!(rss.matches("<item>").count(), 2);
}

#[tokio::test]
async fn podcasts_to_rss_sets_itunes_block_when_public_feeds_disabled() {
    let head_cache = stub_head_cache_for(&["ep1", "ep2"], "0", "audio/mpeg").await;
    let scraper = Client::new();

    let rss_blocked = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        /*public_feeds=*/ false,
        None,
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");
    assert!(
        rss_blocked.contains("itunes:block"),
        "PUBLIC_FEEDS=false must emit itunes:block: {rss_blocked}"
    );

    let rss_public = podcasts_to_rss(
        &fixed_payload(),
        "podcast-uuid",
        "nl-NL",
        /*public_feeds=*/ true,
        None,
        &scraper,
        &head_cache,
    )
    .await
    .expect("render");
    assert!(
        !rss_public.contains("itunes:block"),
        "PUBLIC_FEEDS=true must NOT emit itunes:block: {rss_public}"
    );
}
