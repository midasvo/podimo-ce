//! GraphQL client for Podimo. Mirrors `podimo/client.py`.

use std::time::Duration;

use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::cache::TtlCache;
use crate::config::Config;
use crate::util::{generate_headers, is_correct_email, random_flyer_id, token_key};

// The endpoint URL lives on `Config::graphql_url` so integration tests can
// point at a wiremock server. Production callers leave it at the default.

#[derive(Debug, Error)]
pub enum ClientError {
    /// Bad credentials. Maps to 401.
    #[error("invalid credentials: {0}")]
    InvalidCredentials(String),

    /// Network / upstream failure. Maps to 503.
    #[error("upstream unavailable: {0}")]
    Upstream(String),

    /// GraphQL returned errors. Inspect the message for "not found" routing.
    #[error("graphql error: {0}")]
    GraphQl(String),
}

impl ClientError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, ClientError::GraphQl(msg) if msg.to_lowercase().contains("not found"))
    }
}

#[derive(Debug, Clone)]
pub struct PodimoClient {
    pub username: String,
    pub password: String,
    pub region: String,
    pub locale: String,
    pub key: String,
    pub token: Option<String>,
    preauth_token: Option<String>,
    prereg_id: Option<String>,
}

impl PodimoClient {
    pub fn new(
        username: &str,
        password: &str,
        region: &str,
        locale: &str,
    ) -> Result<Self, ClientError> {
        if username.is_empty() || password.is_empty() {
            return Err(ClientError::InvalidCredentials(
                "empty username or password".into(),
            ));
        }
        if username.len() > 256 || password.len() > 256 {
            return Err(ClientError::InvalidCredentials(
                "username or password are too long".into(),
            ));
        }
        if !is_correct_email(username) {
            return Err(ClientError::InvalidCredentials(
                "email is not in the correct format".into(),
            ));
        }

        Ok(Self {
            username: username.to_string(),
            password: password.to_string(),
            region: region.to_string(),
            locale: locale.to_string(),
            key: token_key(username, password),
            token: None,
            preauth_token: None,
            prereg_id: None,
        })
    }

    /// Three-step login dance: pre-register → onboarding → authorize. Returns
    /// the bearer token to use on subsequent requests.
    pub async fn login(
        &mut self,
        scraper: &Client,
        config: &Config,
    ) -> Result<String, ClientError> {
        self.get_preregister_token(scraper, config).await?;
        self.get_onboarding_id(scraper, config).await?;

        let preauth = self.preauth_token.as_deref().ok_or_else(|| {
            ClientError::Upstream("preauth token missing after pre-register".into())
        })?;

        let headers = generate_headers(Some(preauth), &self.locale);
        let query = r#"
            query AuthorizationAuthorize($email: String!, $password: String!, $locale: String!, $preregisterId: String) {
                tokenWithCredentials(
                    email: $email
                    password: $password
                    locale: $locale
                    preregisterId: $preregisterId
                ) {
                    token
                }
            }
        "#;
        let variables = json!({
            "email": self.username,
            "password": self.password,
            "locale": self.locale,
            "preregisterId": self.prereg_id,
        });
        let result = post_graphql(scraper, config, &headers, query, &variables).await?;
        let token = result
            .get("tokenWithCredentials")
            .and_then(|t| t.get("token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| ClientError::InvalidCredentials("no token in response".into()))?
            .to_string();
        self.token = Some(token.clone());
        Ok(token)
    }

    async fn get_preregister_token(
        &mut self,
        scraper: &Client,
        config: &Config,
    ) -> Result<(), ClientError> {
        let headers = generate_headers(None, &self.locale);
        let query = r#"
            query AuthorizationPreregisterUser($locale: String!, $referenceUser: String, $countryCode: String, $appsFlyerId: String) {
                tokenWithPreregisterUser(
                    locale: $locale
                    referenceUser: $referenceUser
                    countryCode: $countryCode
                    source: MOBILE
                    appsFlyerId: $appsFlyerId
                    currentCountry: $countryCode
                ) {
                    token
                }
            }
        "#;
        let variables = json!({
            "locale": self.locale,
            "countryCode": self.region,
            "appsFlyerId": random_flyer_id(),
        });
        let result = post_graphql(scraper, config, &headers, query, &variables).await?;
        let token = result
            .get("tokenWithPreregisterUser")
            .and_then(|t| t.get("token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| ClientError::Upstream("no tokenWithPreregisterUser".into()))?
            .to_string();
        self.preauth_token = Some(token);
        Ok(())
    }

    async fn get_onboarding_id(
        &mut self,
        scraper: &Client,
        config: &Config,
    ) -> Result<(), ClientError> {
        let preauth = self.preauth_token.as_deref().ok_or_else(|| {
            ClientError::Upstream("preauth token missing before onboarding".into())
        })?;
        let headers = generate_headers(Some(preauth), &self.locale);
        let query = r#"
            query OnboardingQuery {
                userOnboardingFlow {
                    id
                }
            }
        "#;
        let variables = json!({ "locale": self.locale, "countryCode": self.region, "appsFlyerId": random_flyer_id() });
        let result = post_graphql(scraper, config, &headers, query, &variables).await?;
        let id = result
            .get("userOnboardingFlow")
            .and_then(|t| t.get("id"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| ClientError::Upstream("no userOnboardingFlow.id".into()))?
            .to_string();
        self.prereg_id = Some(id);
        Ok(())
    }

    /// Page through `podcastEpisodes` 100 at a time. Returns the full payload
    /// (the first page's structure with all episodes concatenated).
    pub async fn get_podcasts(
        &self,
        scraper: &Client,
        config: &Config,
        podcast_id: &str,
        podcast_cache: &TtlCache<Value>,
    ) -> Result<Value, ClientError> {
        if let Some(cached) = podcast_cache.get(podcast_id).await {
            return Ok(cached);
        }

        let token = self
            .token
            .as_deref()
            .ok_or_else(|| ClientError::InvalidCredentials("login not yet completed".into()))?;
        let headers = generate_headers(Some(token), &self.locale);

        let query = r#"
            query ChannelEpisodesQuery($podcastId: String!, $limit: Int!, $offset: Int!, $sorting: PodcastEpisodeSorting) {
                episodes: podcastEpisodes(
                    podcastId: $podcastId
                    converted: true
                    published: true
                    limit: $limit
                    offset: $offset
                    sorting: $sorting
                ) {
                    ...EpisodeBase
                }
                podcast: podcastById(podcastId: $podcastId) {
                    title
                    description
                    webAddress
                    authorName
                    language
                    images {
                        coverImageUrl
                    }
                }
            }

            fragment EpisodeBase on PodcastEpisode {
                id
                artist
                podcastName
                imageUrl
                description
                datetime
                publishDatetime
                title
                audio {
                    url
                    duration
                }
                streamMedia {
                    duration
                    url
                }
            }
        "#;

        let limit = 100_i64;
        let mut offset = 0_i64;
        let mut full: Option<Value> = None;

        loop {
            let variables = json!({
                "podcastId": podcast_id,
                "limit": limit,
                "offset": offset,
                "sorting": "PUBLISHED_DESCENDING",
            });
            let result = post_graphql(scraper, config, &headers, query, &variables).await?;
            let page_episodes_len = result
                .get("episodes")
                .and_then(|e| e.as_array())
                .map(|a| a.len() as i64)
                .unwrap_or(0);

            match full.as_mut() {
                None => {
                    full = Some(result);
                }
                Some(existing) => {
                    if let (Some(existing_eps), Some(new_eps)) = (
                        existing.get_mut("episodes").and_then(|e| e.as_array_mut()),
                        result.get("episodes").and_then(|e| e.as_array()),
                    ) {
                        existing_eps.extend_from_slice(new_eps);
                    }
                }
            }

            if page_episodes_len == limit {
                offset += limit;
            } else {
                break;
            }
        }

        let result = full.ok_or_else(|| ClientError::Upstream("no episodes returned".into()))?;
        podcast_cache
            .insert(podcast_id.to_string(), result.clone())
            .await;
        Ok(result)
    }
}

/// Decides which URL/client to use for the Cloudflare bypass and posts a
/// GraphQL query. Mirrors `PodimoClient.post` in Python.
async fn post_graphql(
    scraper: &Client,
    config: &Config,
    headers: &std::collections::HashMap<String, String>,
    query: &str,
    variables: &Value,
) -> Result<Value, ClientError> {
    #[derive(Serialize)]
    struct Body<'a> {
        query: &'a str,
        variables: &'a Value,
    }

    let body = Body { query, variables };

    let (url, response) = if let Some(api_key) = &config.scraper_api {
        // ScraperAPI: rewrite the URL through their proxy endpoint.
        let url = format!(
            "https://api.scraperapi.com?api_key={}&url={}&keep_headers=true",
            urlencoding::encode(api_key),
            urlencoding::encode(config.graphql_url.as_str()),
        );
        let resp = build_request(scraper, &url, headers, &body).await?;
        (url, resp)
    } else if let Some(api_key) = &config.zenrows_api {
        // ZenRows: same URL, but with a Bearer-style API key as part of the host.
        // The Python client swaps to `ZenRowsClient(api_key)`. We mirror that here
        // by hitting their proxy endpoint directly with the api key as a query arg.
        let url = format!(
            "https://api.zenrows.com/v1/?apikey={}&url={}",
            urlencoding::encode(api_key),
            urlencoding::encode(config.graphql_url.as_str()),
        );
        let resp = build_request(scraper, &url, headers, &body).await?;
        (url, resp)
    } else {
        let resp = build_request(scraper, config.graphql_url.as_str(), headers, &body).await?;
        (config.graphql_url.as_str().to_string(), resp)
    };

    if !response.status().is_success() {
        // Don't leak proxy API keys in the URL — log the configured destination
        // host, never the formatted SCRAPER_API/ZENROWS_API URL that includes the key.
        return Err(ClientError::Upstream(format!(
            "Podimo returned status {} (target host: {})",
            response.status(),
            host_only(&url),
        )));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| ClientError::Upstream(format!("invalid JSON: {e}")))?;

    if let Some(errors) = body.get("errors").and_then(|e| e.as_array()) {
        let msg = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ClientError::GraphQl(msg));
    }

    body.get("data")
        .cloned()
        .ok_or_else(|| ClientError::GraphQl("no data field in response".into()))
}

fn host_only(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or("")
        .to_string()
}

async fn build_request<B: Serialize>(
    scraper: &Client,
    url: &str,
    headers: &std::collections::HashMap<String, String>,
    body: &B,
) -> Result<reqwest::Response, ClientError> {
    let mut req = scraper
        .post(url)
        .timeout(Duration::from_secs(30))
        .json(body);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    req.send()
        .await
        .map_err(|e| ClientError::Upstream(e.to_string()))
}
