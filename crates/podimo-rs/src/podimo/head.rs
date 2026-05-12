//! Episode-media HEAD probe with retries + cache.

use std::time::Duration;

use reqwest::Client;

use crate::cache::{HeadInfo, TtlCache};

const RETRIES: u32 = 3;
const TIMEOUT_PER_TRY: Duration = Duration::from_secs(10);

pub async fn url_head_info(
    scraper: &Client,
    cache: &TtlCache<HeadInfo>,
    episode_id: &str,
    url: &str,
    locale: &str,
) -> Result<HeadInfo, reqwest::Error> {
    // Use get_no_expire: a stale-but-present cached entry beats a transient HEAD
    // failure, and the historical record stays on disk for inspection.
    if let Some(cached) = cache.get_no_expire(episode_id).await {
        return Ok(cached);
    }

    let headers = crate::util::generate_headers(None, locale);

    let mut last_err = None;
    for attempt in 0..RETRIES {
        let mut req = scraper.head(url).timeout(TIMEOUT_PER_TRY);
        for (k, v) in &headers {
            req = req.header(k, v);
        }

        match req.send().await {
            Ok(resp) => {
                let content_length = resp
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("0")
                    .to_string();

                let guessed = mime_guess::from_path(strip_query(url)).first_raw();
                let content_type = guessed
                    .map(|s| s.to_string())
                    .or_else(|| {
                        resp.headers()
                            .get(reqwest::header::CONTENT_TYPE)
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "audio/mpeg".to_string());

                let info = HeadInfo {
                    content_length,
                    content_type,
                };
                cache.insert(episode_id.to_string(), info.clone()).await;
                return Ok(info);
            }
            Err(err) => {
                if attempt + 1 < RETRIES {
                    let delay = 2u64.saturating_pow(attempt);
                    tracing::info!(target: "podimo", "retrying HEAD {url} after {err} (attempt {}/{RETRIES})", attempt + 2);
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
                last_err = Some(err);
            }
        }
    }
    Err(last_err.expect("loop ran at least once"))
}

fn strip_query(url: &str) -> &str {
    url.split_once('?').map(|(p, _)| p).unwrap_or(url)
}
