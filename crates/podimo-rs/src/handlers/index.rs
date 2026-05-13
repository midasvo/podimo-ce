//! Form rendering + POST validation.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Form;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::config::{is_known_locale, is_known_region, LOCALES, REGIONS};
use crate::error::AppError;
use crate::state::AppState;
use crate::util::{parse_podimo_input, random_hex_id, PodimoKind, PODCAST_ID_RE};

pub(crate) fn router() -> Router<AppState> {
    Router::new().route("/", get(render_form).post(handle_submit))
}

#[derive(Serialize)]
struct IndexCtx<'a> {
    error: &'a str,
    locales: &'a [&'a str],
    regions: &'a [(&'a str, &'a str)],
    need_credentials: bool,
}

#[derive(Serialize)]
struct FeedLocationCtx<'a> {
    url: &'a str,
}

async fn render_form(State(state): State<AppState>) -> Response {
    render_with_error(&state, "")
}

fn render_with_error(state: &AppState, error: &str) -> Response {
    let ctx = IndexCtx {
        error,
        locales: LOCALES,
        regions: REGIONS,
        need_credentials: !state.config.local_credentials,
    };
    match state.templates.render("index.html", &ctx) {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(target: "podimo", "template render failed: {err}");
            AppError::Internal(format!("template render: {err}")).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct SubmitForm {
    email: Option<String>,
    password: Option<String>,
    podcast_id: Option<String>,
    region: Option<String>,
    locale: Option<String>,
}

async fn handle_submit(State(state): State<AppState>, Form(form): Form<SubmitForm>) -> Response {
    let mut error = String::new();

    if !state.config.local_credentials {
        if form.email.as_deref().unwrap_or("").is_empty() {
            error.push_str("Email is required");
        }
        if form.password.as_deref().unwrap_or("").is_empty() {
            error.push_str("Password is required");
        }
    }
    // Accept either a bare UUID or a full Podimo URL. For URLs we additionally
    // detect whether it's an audiobook (`/audiobook/<uuid>`) or a podcast.
    let raw_input = form.podcast_id.as_deref().unwrap_or("");
    let (kind, podcast_id) = match parse_podimo_input(raw_input) {
        Some((k, id)) => (k, id),
        None => (PodimoKind::Podcast, ""),
    };
    if raw_input.trim().is_empty() {
        error.push_str("Podcast or audiobook ID is required");
    } else if podcast_id.is_empty() || !PODCAST_ID_RE.is_match(podcast_id) {
        error.push_str("ID is not valid");
    }

    let region = form.region.as_deref().unwrap_or("");
    if region.is_empty() {
        error.push_str("Region is required");
    } else if !is_known_region(region) {
        error.push_str("Region is not valid");
    }

    let locale = form.locale.as_deref().unwrap_or("");
    if locale.is_empty() {
        error.push_str("Locale is required");
    } else if !is_known_locale(locale) {
        error.push_str("Locale is not valid");
    }

    if !error.is_empty() {
        return render_with_error(&state, &error);
    }

    let podcast_id_q = urlencoding::encode(podcast_id);
    let region_q = urlencoding::encode(region);
    let locale_q = urlencoding::encode(locale);
    let route = kind.route_segment();

    let url = if state.config.local_credentials {
        format!(
            "{proto}://{host}/{route}/{pid}.xml?{rand}&region={r}&locale={l}",
            proto = state.config.protocol,
            host = state.config.hostname,
            route = route,
            pid = podcast_id_q,
            rand = random_hex_id(10),
            r = region_q,
            l = locale_q,
        )
    } else {
        let email = form.email.as_deref().unwrap_or("");
        let password = form.password.as_deref().unwrap_or("");
        let email_q = urlencoding::encode(email);
        let password_q = urlencoding::encode(password);
        let comma = urlencoding::encode(",");
        format!(
            "{proto}://{email}{comma}{region}{comma}{locale}:{password}@{host}/{route}/{pid}.xml?{rand}&region={r}&locale={l}",
            proto = state.config.protocol,
            email = email_q,
            comma = comma,
            region = region_q,
            locale = locale_q,
            password = password_q,
            host = state.config.hostname,
            route = route,
            pid = podcast_id_q,
            rand = random_hex_id(10),
            r = region_q,
            l = locale_q,
        )
    };

    let ctx = FeedLocationCtx { url: &url };
    match state.templates.render("feed_location.html", &ctx) {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(target: "podimo", "template render failed: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "text/plain")],
                "template render error",
            )
                .into_response()
        }
    }
}
