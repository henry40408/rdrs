use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
use crate::services::sanitize_summary;
use crate::utils::url_validation::validate_url;

/// Maximum URLs accepted in a single summarizer run.
pub const MAX_URLS: usize = 30;

/// Set of user ids with a `/summarizer/item` request currently in flight. The
/// client resolves cards one at a time, but that ordering lives only in the
/// browser; this registry enforces **one live Kagi call per user** on the
/// server so a hand-crafted burst of parallel `/summarizer/item` POSTs cannot
/// fan out 30 concurrent outbound requests.
pub type InFlightRegistry = Arc<Mutex<HashSet<i64>>>;

/// Construct an empty in-flight registry (used in `AppState` and tests).
pub fn new_inflight_registry() -> InFlightRegistry {
    Arc::new(Mutex::new(HashSet::new()))
}

/// RAII marker: removes the user from the in-flight set when dropped, so the
/// slot is released on every exit path (including early returns / panics).
pub struct InFlightGuard {
    registry: InFlightRegistry,
    user_id: i64,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.registry.lock() {
            set.remove(&self.user_id);
        }
    }
}

/// Reserve the user's single in-flight slot. Returns `Some(guard)` if the user
/// had no request in flight, or `None` if one is already running (the caller
/// should reject the new request). The lock is held only for the set insert —
/// never across an `.await`.
pub fn try_begin_inflight(registry: &InFlightRegistry, user_id: i64) -> Option<InFlightGuard> {
    let mut set = registry.lock().ok()?;
    if !set.insert(user_id) {
        return None; // already in flight for this user
    }
    Some(InFlightGuard {
        registry: registry.clone(),
        user_id,
    })
}

/// Parse the textarea into a validated, de-duplicated, order-preserving list of
/// URL strings. Rejects an empty list, more than `MAX_URLS`, and any line that
/// is not a fetchable http(s) URL (SSRF-validated).
pub(crate) fn parse_url_lines(input: &str) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
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
        return Err(format!("Too many URLs — {MAX_URLS} max per run."));
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
/// empty. `summary` is Kagi's output run through
/// [`crate::services::sanitize_summary`] — it is rendered with `|safe`, and
/// Kagi writes it from a page nobody here controls, so it is not trusted.
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
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
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
            csrf_token: auth_user.csrf_token.clone(),
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
            csrf_token: auth_user.csrf_token.clone(),
            urls_text: form.urls,
            error,
            cards,
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct ItemForm {
    pub url: String,
    pub index: usize,
}

pub async fn item(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Form(form): Form<ItemForm>,
) -> axum::response::Response {
    let host = url_host(&form.url);
    let err_card = |msg: String| SummarizerCard {
        index: form.index,
        title: host.clone(),
        url: form.url.clone(),
        state: "error",
        summary: String::new(),
        error: msg,
    };

    let render = |card: SummarizerCard| match (SummarizerCardTemplate { card }).render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Re-validate (defense in depth — the browser could POST anything).
    let parsed = match url::Url::parse(&form.url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u,
        _ => return render(err_card("Not a valid URL.".into())),
    };
    if validate_url(&parsed).is_err() {
        return render(err_card("URL not allowed.".into()));
    }

    // Enforce one live summary per user on the server (the client already
    // serialises, but a hand-crafted burst must not fan out concurrent Kagi
    // calls). Held until the function returns, releasing the slot on every path.
    let Some(_slot) = try_begin_inflight(&state.summarizer_inflight, auth_user.user.id) else {
        return render(err_card(
            "Another summary is already in progress — wait for it to finish.".into(),
        ));
    };

    let kagi = match user_settings::get_save_services_config(&state.db, auth_user.user.id).await {
        Ok(c) => c.kagi,
        Err(_) => None,
    };
    let Some(config) =
        kagi.filter(super::super::services::summarize::kagi::KagiConfig::is_configured)
    else {
        return render(err_card("Kagi is not configured.".into()));
    };

    let card = match crate::services::summarize::kagi::summarize_url(&config, &form.url).await {
        Ok(r) if r.success => SummarizerCard {
            index: form.index,
            title: r.title.unwrap_or(host),
            url: form.url.clone(),
            state: "completed",
            summary: r
                .output_text
                .as_deref()
                .map(sanitize_summary)
                .unwrap_or_default(),
            error: String::new(),
        },
        Ok(r) => err_card(r.error.unwrap_or_else(|| "Summarization failed.".into())),
        Err(e) => err_card(e.to_string()),
    };
    render(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflight_rejects_second_concurrent_request_for_same_user() {
        let reg = new_inflight_registry();
        let g1 = try_begin_inflight(&reg, 42);
        assert!(g1.is_some(), "first request acquires the slot");
        assert!(
            try_begin_inflight(&reg, 42).is_none(),
            "second concurrent request for the same user is rejected"
        );
    }

    #[test]
    fn inflight_allows_different_users_concurrently() {
        let reg = new_inflight_registry();
        let _g1 = try_begin_inflight(&reg, 1);
        assert!(
            try_begin_inflight(&reg, 2).is_some(),
            "a different user is not blocked"
        );
    }

    #[test]
    fn inflight_slot_released_on_guard_drop() {
        let reg = new_inflight_registry();
        {
            let _g = try_begin_inflight(&reg, 7);
            assert!(try_begin_inflight(&reg, 7).is_none());
        }
        // Guard dropped → slot free again.
        assert!(
            try_begin_inflight(&reg, 7).is_some(),
            "dropping the guard releases the user's slot"
        );
    }

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
        let many = (0..=MAX_URLS)
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
