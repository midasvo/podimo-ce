//! GET /feed/<podcast_id>.xml.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::collections::HashMap;

use crate::config::{is_known_locale, is_known_region};
use crate::error::AppError;
use crate::podimo::{ClientError, PodimoClient};
use crate::state::AppState;
use crate::util::{amp_arg, split_username_region_locale, PODCAST_ID_RE};

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/feed/:podcast_id", get(serve))
}

pub(crate) fn unauthorized_response(hostname: &str) -> Response {
    let body = format!(
        "401 Unauthorized.\n\
You need to login with the correct credentials for Podimo.\n\n\
{}",
        crate::handlers::not_found::example_block(hostname)
    );
    let mut resp = (StatusCode::UNAUTHORIZED, body).into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm='Podimo credentials'"),
    );
    resp
}

async fn serve(
    State(state): State<AppState>,
    Path(podcast_id_with_ext): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    uri: Uri,
    req_headers: axum::http::HeaderMap,
) -> Response {
    // The route matches anything under /feed/<segment>; we require `.xml` and
    // strip it here so the router doesn't have to treat the dot specially.
    let Some(podcast_id) = podcast_id_with_ext.strip_suffix(".xml") else {
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

    if !PODCAST_ID_RE.is_match(podcast_id) {
        return AppError::BadRequest("Invalid podcast id format".into()).into_response();
    }
    if !is_known_region(&region) {
        return AppError::BadRequest("Invalid region".into()).into_response();
    }
    if !is_known_locale(&locale) {
        return AppError::BadRequest("Invalid locale".into()).into_response();
    }

    let limit = match amp_arg(|k| params.get(k).map(String::as_str), "limit") {
        Some(s) if s.is_empty() => None,
        Some(s) => match s.parse::<usize>() {
            Ok(n) if n >= 1 => Some(n),
            _ => {
                return AppError::BadRequest("Invalid limit (must be a positive integer)".into())
                    .into_response();
            }
        },
        None => None,
    };

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

    let payload = match client
        .get_podcasts(
            &state.scraper,
            &state.config,
            podcast_id,
            &state.caches.podcasts,
        )
        .await
    {
        Ok(v) => v,
        Err(err) if err.is_not_found() => return AppError::NotFound.into_response(),
        Err(err) => return AppError::Internal(format!("fetch podcasts: {err}")).into_response(),
    };

    match crate::podimo::rss::podcasts_to_rss(
        &payload,
        podcast_id,
        &locale,
        state.config.public_feeds,
        limit,
        &state.scraper,
        &state.caches.head,
    )
    .await
    {
        Ok(rss) => (StatusCode::OK, [(header::CONTENT_TYPE, "text/xml")], rss).into_response(),
        Err(err) => AppError::Internal(format!("rss render: {err}")).into_response(),
    }
}

/// Parses an HTTP Basic header. Returns `(username_field, password)`.
fn parse_basic_auth(headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw
        .strip_prefix("Basic ")
        .or_else(|| raw.strip_prefix("basic "))?;
    let decoded = BASE64.decode(token.trim()).ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    let (user, pass) = s.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_auth() {
        let mut h = axum::http::HeaderMap::new();
        let raw = BASE64.encode("a@b.com,nl,nl-NL:secret");
        h.insert(
            header::AUTHORIZATION,
            format!("Basic {raw}").parse().unwrap(),
        );
        let (user, pass) = parse_basic_auth(&h).unwrap();
        assert_eq!(user, "a@b.com,nl,nl-NL");
        assert_eq!(pass, "secret");
    }

    #[test]
    fn missing_basic_auth_returns_none() {
        let h = axum::http::HeaderMap::new();
        assert!(parse_basic_auth(&h).is_none());
    }
}
