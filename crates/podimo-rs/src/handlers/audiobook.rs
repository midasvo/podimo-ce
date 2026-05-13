//! GET /audiobook/<audiobook_id>.xml.
//!
//! Mirrors `handlers::feed` for podcasts. A single-item RSS feed is rendered
//! per audiobook: one channel, one item (the book), one enclosure pointing at
//! a short-lived signed audio URL.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;

use crate::config::{is_known_locale, is_known_region};
use crate::error::AppError;
use crate::handlers::feed::unauthorized_response;
use crate::podimo::{ClientError, PodimoClient};
use crate::state::AppState;
use crate::util::{amp_arg, parse_basic_auth, split_username_region_locale, PODCAST_ID_RE};

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/audiobook/:audiobook_id", get(serve))
}

async fn serve(
    State(state): State<AppState>,
    Path(audiobook_id_with_ext): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    uri: Uri,
    req_headers: axum::http::HeaderMap,
) -> Response {
    let Some(audiobook_id) = audiobook_id_with_ext.strip_suffix(".xml") else {
        return (StatusCode::NOT_FOUND, "404 Not found.").into_response();
    };

    let (username, password, region, locale) = if state.config.local_credentials {
        let region =
            amp_arg(|k| params.get(k).map(String::as_str), "region").unwrap_or_else(|| "nl".into());
        let locale = amp_arg(|k| params.get(k).map(String::as_str), "locale")
            .unwrap_or_else(|| "nl-NL".into());
        let Some(email) = state.config.podimo_email.clone() else {
            return AppError::Internal("LOCAL_CREDENTIALS enabled but PODIMO_EMAIL unset".into())
                .into_response();
        };
        let Some(pw) = state.config.podimo_password.clone() else {
            return AppError::Internal(
                "LOCAL_CREDENTIALS enabled but PODIMO_PASSWORD unset".into(),
            )
            .into_response();
        };
        (email, pw, region, locale)
    } else {
        match parse_basic_auth(&req_headers) {
            Some((user_field, password)) => {
                let (username, region, locale) = split_username_region_locale(&user_field);
                (username, password, region, locale)
            }
            None => return unauthorized_response(&state.config.hostname),
        }
    };

    if !PODCAST_ID_RE.is_match(audiobook_id) {
        return AppError::BadRequest("Invalid audiobook id format".into()).into_response();
    }
    if !is_known_region(&region) {
        return AppError::BadRequest("Invalid region".into()).into_response();
    }
    if !is_known_locale(&locale) {
        return AppError::BadRequest("Invalid locale".into()).into_response();
    }

    let url_for_blocklist = uri.path_and_query().map(|p| p.as_str()).unwrap_or("");
    if state.blocklist.contains_substring(url_for_blocklist) {
        return AppError::Gone.into_response();
    }

    let mut client = match PodimoClient::new(&username, &password, &region, &locale) {
        Ok(c) => c,
        Err(_) => return unauthorized_response(&state.config.hostname),
    };
    if let Some(token) = state.caches.tokens.get(&client.key).await {
        client.token = Some(token);
    } else {
        match client.login(&state.scraper, &state.config).await {
            Ok(token) => state.caches.tokens.insert(client.key.clone(), token).await,
            Err(ClientError::InvalidCredentials(_)) => {
                return unauthorized_response(&state.config.hostname);
            }
            Err(err) => {
                tracing::error!(target: "podimo", "upstream auth failure: {err}");
                return AppError::UpstreamUnavailable(err.to_string()).into_response();
            }
        }
    }

    // Metadata and audio-URL queries are independent (different GraphQL fields)
    // and both go through the same Cloudflare-bypass path, so the cold-cache
    // wall-time is dominated by the slower of the two — `tokio::join!` halves
    // it. On warm cache each branch short-circuits before any network call.
    let (meta_result, audio_result) = tokio::join!(
        client.get_audiobook(
            &state.scraper,
            &state.config,
            audiobook_id,
            &state.caches.audiobook_meta,
        ),
        client.get_audiobook_audio_url(
            &state.scraper,
            &state.config,
            audiobook_id,
            &state.caches.audiobook_audio,
        ),
    );

    let meta = match meta_result {
        Ok(v) => v,
        Err(err) if err.is_not_found() => return AppError::NotFound.into_response(),
        Err(err) => return AppError::Internal(format!("fetch audiobook: {err}")).into_response(),
    };

    let audio_url = match audio_result {
        Ok(v) => v,
        Err(err) if err.is_not_found() => return AppError::NotFound.into_response(),
        Err(err) => {
            return AppError::Internal(format!("fetch audiobook audio: {err}")).into_response()
        }
    };

    match crate::podimo::rss::audiobook_to_rss(
        &meta,
        &audio_url,
        audiobook_id,
        &locale,
        state.config.public_feeds,
        &state.scraper,
        &state.caches.head,
    )
    .await
    {
        Ok(rss) => (StatusCode::OK, [(header::CONTENT_TYPE, "text/xml")], rss).into_response(),
        Err(err) => AppError::Internal(format!("rss render: {err}")).into_response(),
    }
}
