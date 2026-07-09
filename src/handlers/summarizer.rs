use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use serde::Deserialize;
use url::Url;

use crate::AppState;
use crate::handlers::pages::{AppLayoutContext, build_app_layout};
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::user_settings;
use crate::utils::url_validation::validate_url;

/// Maximum URLs accepted in a single summarizer run.
pub const MAX_URLS: usize = 30;

/// Parse the textarea into a validated, de-duplicated, order-preserving list of
/// URL strings. Rejects an empty list, more than `MAX_URLS`, and any line that
/// is not a fetchable http(s) URL (SSRF-validated).
pub(crate) fn parse_url_lines(input: &str) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in input.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = Url::parse(line).map_err(|_err| format!("Not a valid URL: {line}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("Only http(s) URLs are supported: {line}"));
        }
        validate_url(&parsed).map_err(|e| format!("URL not allowed ({line}): {e}"))?;
        if seen.insert(line.to_string()) {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        return Err("Enter at least one URL.".to_string());
    }
    if out.len() > MAX_URLS {
        return Err(format!("Too many URLs — {} max per run.", MAX_URLS));
    }
    Ok(out)
}

/// Host for the card-title fallback; returns the input unchanged if unparseable.
pub(crate) fn url_host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// One URL's card. `state` selects the rendered branch; unused string fields are
/// empty. `summary` is trusted HTML/markdown from Kagi (rendered with `|safe`).
///
/// `pub` (not `pub(crate)`): it's a field type of the `pub` `SummarizerTemplate`.
#[derive(Debug, Clone)]
pub struct SummarizerCard {
    pub index: usize,
    pub url: String,
    pub title: String,
    pub state: &'static str,
    pub summary: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "_summarizer_card_fragment.html")]
pub(crate) struct SummarizerCardTemplate {
    pub card: SummarizerCard,
}

/// Renders the `/summarizer` page: a settings prompt when Kagi isn't
/// configured, or the URL-list form + result cards when it is.
#[derive(Template)]
#[template(path = "summarizer.html")]
pub struct SummarizerTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub kagi_configured: bool,
    pub urls_text: String,
    pub error: Option<String>,
    pub cards: Vec<SummarizerCard>,
}

impl IntoResponse for SummarizerTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => {
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

async fn kagi_configured(state: &AppState, user_id: i64) -> bool {
    user_settings::get_save_services_config(&state.db, user_id)
        .await
        .ok()
        .and_then(|c| c.kagi)
        .is_some_and(|k| k.is_configured())
}

pub async fn summarizer_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SummarizerTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let configured = kagi_configured(&state, auth_user.user.id).await;
    (
        flash,
        SummarizerTemplate {
            title: "Summarizer",
            git_version: crate::GIT_VERSION,
            layout,
            kagi_configured: configured,
            urls_text: String::new(),
            error: None,
            cards: Vec::new(),
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct StartForm {
    pub urls: String,
}

pub async fn start(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Form(form): Form<StartForm>,
) -> (Flash, SummarizerTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let configured = kagi_configured(&state, auth_user.user.id).await;

    let (error, cards) = match parse_url_lines(&form.urls) {
        Ok(urls) => (
            None,
            urls.into_iter()
                .enumerate()
                .map(|(index, url)| SummarizerCard {
                    index,
                    title: url_host(&url),
                    url,
                    state: "queued",
                    summary: String::new(),
                    error: String::new(),
                })
                .collect(),
        ),
        Err(msg) => (Some(msg), Vec::new()),
    };

    (
        flash,
        SummarizerTemplate {
            title: "Summarizer",
            git_version: crate::GIT_VERSION,
            layout,
            kagi_configured: configured,
            urls_text: form.urls,
            error,
            cards,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trims_and_drops_blanks() {
        let out = parse_url_lines("  https://a.com/x \n\n https://b.com/y\n").unwrap();
        assert_eq!(out, vec!["https://a.com/x", "https://b.com/y"]);
    }

    #[test]
    fn dedupes_preserving_order() {
        let out = parse_url_lines("https://a.com\nhttps://a.com\nhttps://b.com").unwrap();
        assert_eq!(out, vec!["https://a.com", "https://b.com"]);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_url_lines("   \n  ").is_err());
    }

    #[test]
    fn rejects_over_max() {
        let many = (0..(MAX_URLS + 1))
            .map(|i| format!("https://ex.com/{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_url_lines(&many).unwrap_err();
        assert!(err.contains("30"));
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(parse_url_lines("ftp://a.com/x").is_err());
        assert!(parse_url_lines("not a url").is_err());
    }

    #[test]
    fn host_fallback() {
        assert_eq!(url_host("https://news.example.net/a/b"), "news.example.net");
        assert_eq!(url_host("garbage"), "garbage");
    }
}
