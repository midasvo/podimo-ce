//! RSS 2.0 rendering with iTunes extensions.

use std::collections::BTreeMap;

use futures::future::join_all;
use reqwest::Client;
use rss::extension::itunes::{
    ITunesChannelExtensionBuilder, ITunesItemExtensionBuilder, ITunesOwnerBuilder,
};
use rss::{ChannelBuilder, EnclosureBuilder, ItemBuilder};
use serde_json::Value;

use crate::cache::{HeadInfo, TtlCache};
use crate::podimo::head::url_head_info;
use crate::util::jpg_fragment;

const ITUNES_NS: &str = "http://www.itunes.com/dtds/podcast-1.0.dtd";
const CONCURRENT_HEAD_PROBES: usize = 5;

pub async fn podcasts_to_rss(
    payload: &Value,
    podcast_id: &str,
    locale: &str,
    public_feeds: bool,
    scraper: &Client,
    head_cache: &TtlCache<HeadInfo>,
) -> anyhow::Result<String> {
    let podcast = payload.get("podcast");
    let episodes: &[Value] = payload
        .get("episodes")
        .and_then(|e| e.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let last_episode = episodes.first();

    let title = first_non_null_string(&[
        podcast.and_then(|p| p.get("title")),
        last_episode.and_then(|e| e.get("podcastName")),
    ])
    .unwrap_or_else(|| "Podimo".to_string());

    let description = first_non_null_string(&[podcast.and_then(|p| p.get("description"))])
        .unwrap_or_else(|| title.clone());

    let image = first_non_null_string(&[
        podcast
            .and_then(|p| p.get("images"))
            .and_then(|i| i.get("coverImageUrl")),
        last_episode.and_then(|e| e.get("imageUrl")),
    ])
    .map(|s| jpg_fragment(&s));

    let language = first_non_null_string(&[podcast.and_then(|p| p.get("language"))])
        .unwrap_or_else(|| locale.to_string());

    let author = first_non_null_string(&[
        podcast.and_then(|p| p.get("authorName")),
        last_episode.and_then(|e| e.get("artist")),
    ])
    .unwrap_or_default();

    let link = format!("https://podimo.com/shows/{podcast_id}");

    // Up to CONCURRENT_HEAD_PROBES probes in flight per batch; ordering preserved.
    let mut items: Vec<rss::Item> = Vec::with_capacity(episodes.len());
    for chunk in episodes.chunks(CONCURRENT_HEAD_PROBES) {
        let batch = join_all(
            chunk
                .iter()
                .map(|ep| build_item(scraper, head_cache, ep, locale)),
        )
        .await;
        for res in batch {
            match res {
                Ok(Some(it)) => items.push(it),
                Ok(None) => {}
                Err(err) => tracing::warn!(target: "podimo", "feed entry skipped: {err}"),
            }
        }
    }

    let itunes_owner = ITunesOwnerBuilder::default()
        .name(Some(author.clone()))
        .build();
    let mut itunes = ITunesChannelExtensionBuilder::default();
    itunes
        .author(Some(author))
        .image(image.clone())
        .owner(Some(itunes_owner));
    if !public_feeds {
        itunes.block(Some("Yes".to_string()));
    }
    let itunes = itunes.build();

    let mut channel = ChannelBuilder::default();
    channel
        .title(title.clone())
        .description(description)
        .link(link)
        .language(Some(language))
        .itunes_ext(Some(itunes))
        .items(items);
    let namespaces: BTreeMap<String, String> = [("itunes".to_string(), ITUNES_NS.to_string())]
        .into_iter()
        .collect();
    channel.namespaces(namespaces);

    if let Some(image_url) = image {
        let image_obj = rss::ImageBuilder::default()
            .url(image_url)
            .title(title.clone())
            .link(format!("https://podimo.com/shows/{podcast_id}"))
            .build();
        channel.image(Some(image_obj));
    }

    let channel = channel.build();
    Ok(channel.to_string())
}

async fn build_item(
    scraper: &Client,
    head_cache: &TtlCache<HeadInfo>,
    episode: &Value,
    locale: &str,
) -> anyhow::Result<Option<rss::Item>> {
    let id = episode.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let title = episode
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = episode
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let pub_date = episode
        .get("publishDatetime")
        .and_then(|v| v.as_str())
        .or_else(|| episode.get("datetime").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let (audio_url, duration) = extract_audio_url(episode);
    let Some(audio_url) = audio_url else {
        return Ok(None);
    };

    // url_head_info already bounds total time via RETRIES * TIMEOUT_PER_TRY +
    // backoff; no outer timeout needed.
    let head = url_head_info(scraper, head_cache, id, &audio_url, locale)
        .await
        .map_err(|err| anyhow::anyhow!("HEAD probe failed for episode {id}: {err}"))?;

    let image_url = episode
        .get("imageUrl")
        .and_then(|v| v.as_str())
        .map(jpg_fragment);
    let enclosure = EnclosureBuilder::default()
        .url(audio_url.clone())
        .length(head.content_length)
        .mime_type(head.content_type)
        .build();

    let mut itunes_item = ITunesItemExtensionBuilder::default();
    if duration > 0 {
        itunes_item.duration(Some(duration.to_string()));
    }
    if let Some(img) = image_url {
        itunes_item.image(Some(img));
    }

    let item = ItemBuilder::default()
        .guid(Some(
            rss::GuidBuilder::default()
                .value(id.to_string())
                .permalink(false)
                .build(),
        ))
        .title(Some(title))
        .description(Some(description))
        .pub_date(pub_date)
        .enclosure(Some(enclosure))
        .itunes_ext(Some(itunes_item.build()))
        .build();

    Ok(Some(item))
}

/// Returns `(Option<url>, duration_seconds)` from an episode object,
/// rewriting HLS URLs to the equivalent MP3 stream where possible.
fn extract_audio_url(episode: &Value) -> (Option<String>, i64) {
    let audio = episode.get("audio");
    let url = audio
        .and_then(|a| a.get("url"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let duration = audio
        .and_then(|a| a.get("duration"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if let Some(url) = url {
        return (Some(url), duration);
    }

    let stream = episode.get("streamMedia");
    if let Some(stream) = stream {
        let mut url = stream
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let duration = stream.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
        if let Some(ref mut u) = url {
            if u.contains("hls-media") && u.contains("/main.m3u8") {
                *u = u
                    .replace("hls-media", "audios")
                    .replace("/main.m3u8", ".mp3");
            }
        }
        return (url, duration);
    }
    (None, 0)
}

fn first_non_null_string(candidates: &[Option<&Value>]) -> Option<String> {
    for v in candidates.iter().flatten() {
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_audio_url_prefers_audio_block() {
        let ep = json!({
            "audio": {"url": "https://a/file.mp3", "duration": 42},
            "streamMedia": {"url": "https://s/main.m3u8", "duration": 1},
        });
        assert_eq!(
            extract_audio_url(&ep),
            (Some("https://a/file.mp3".into()), 42)
        );
    }

    #[test]
    fn extract_audio_url_falls_back_to_stream_media_when_audio_empty() {
        let ep = json!({
            "audio": {"url": "", "duration": 0},
            "streamMedia": {"url": "https://s/file.mp3", "duration": 99},
        });
        assert_eq!(
            extract_audio_url(&ep),
            (Some("https://s/file.mp3".into()), 99)
        );
    }

    #[test]
    fn extract_audio_url_rewrites_hls_to_mp3() {
        let ep = json!({
            "audio": null,
            "streamMedia": {
                "url": "https://hls-media.podimo.com/foo/bar/main.m3u8?sig=x",
                "duration": 60
            },
        });
        let (url, dur) = extract_audio_url(&ep);
        assert_eq!(dur, 60);
        let url = url.unwrap();
        assert!(url.contains("audios"), "url should contain 'audios': {url}");
        assert!(url.contains(".mp3"), "url should contain '.mp3': {url}");
        assert!(!url.contains("hls-media"));
        assert!(!url.contains("/main.m3u8"));
    }

    #[test]
    fn extract_audio_url_returns_none_when_no_audio() {
        let ep = json!({"audio": null, "streamMedia": null});
        assert_eq!(extract_audio_url(&ep), (None, 0));
    }

    #[test]
    fn first_non_null_string_skips_empty_and_null() {
        let a = Value::Null;
        let b = json!("");
        let c = json!("found");
        assert_eq!(
            first_non_null_string(&[Some(&a), Some(&b), Some(&c)]),
            Some("found".into())
        );
        assert_eq!(first_non_null_string(&[Some(&a), Some(&b)]), None);
        assert_eq!(first_non_null_string(&[None, None]), None);
    }
}
