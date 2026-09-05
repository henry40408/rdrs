use axum::{
    Form,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::{category, feed};
use crate::services::{feed_discovery, feed_sync, opml};
use url::Url;

// Form-action POST endpoints for the SSR /feeds page. Each accepts
// application/x-www-form-urlencoded (or multipart for import) bodies and
// returns a FlashRedirect response (303 + flash cookie + Location). The
// GReader /reader/api/0/subscription/{edit,import,export} endpoints stay
// alive — external clients (FreshRSS, Reeder) depend on them.

#[derive(Debug, Deserialize)]
pub struct CreateFeedForm {
    pub url: String,
    pub category_id: i64,
}

pub async fn create_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<CreateFeedForm>,
) -> impl IntoResponse {
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return FlashRedirect::error("/feeds", "Feed URL cannot be empty").into_response();
    }
    let user_id = auth_user.user.id;
    let category_id = req.category_id;
    let user_agent = state.config.user_agent.clone();

    let owned = category::find_by_id_and_user(&state.db, category_id, user_id)
        .await
        .is_ok_and(|c| c.is_some());
    if !owned {
        return FlashRedirect::error("/feeds", "Invalid category").into_response();
    }

    let discovered = match feed_discovery::discover_feed(&url, &user_agent, &state.fetcher).await {
        Ok(d) => d,
        Err(e) => {
            return FlashRedirect::error("/feeds", format!("Failed to discover feed: {e}"))
                .into_response();
        }
    };

    let create_url = discovered.feed_url.clone();
    let create_title = discovered.title.clone();
    let create_desc = discovered.description.clone();
    let create_site = discovered.site_url.clone();
    let result: AppResult<()> = async {
        if feed::find_by_url_for_user(&state.db, &create_url, user_id)
            .await?
            .is_some()
        {
            return Err(AppError::FeedExists);
        }
        feed::create_feed(
            &state.db,
            &feed::CreateFeedParams {
                category_id,
                url: &create_url,
                title: create_title.as_deref(),
                description: create_desc.as_deref(),
                site_url: create_site.as_deref(),
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "Feed added.").into_response()
        }
        Err(AppError::FeedExists) => {
            FlashRedirect::error("/feeds", "Feed already subscribed").into_response()
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/feeds", msg).into_response(),
        _ => FlashRedirect::error("/feeds", "Failed to add feed").into_response(),
    }
}

/// The optional text fields are `Option<String>` so that "the request omitted
/// this field" and "the request sent it blank" stay distinguishable:
///
/// - absent (`None`) — leave the stored value alone. A partial update that
///   only touches, say, the category cannot wipe the rest by accident.
/// - present and blank (`Some("")`) — a deliberate erase. The edit form
///   round-trips the current value into every input, so a blank one that
///   reaches the server was blanked by the user.
/// - present and non-blank — trimmed and stored.
///
/// `title` is the exception: a feed with no title has nothing to render in the
/// sidebar, so a blank title keeps the old one and `None` is unreachable in
/// practice. It stays `Option<String>` only for symmetry with the rest.
#[derive(Debug, Deserialize)]
pub struct EditFeedForm {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub site_url: Option<String>,
    pub category_id: i64,
    #[serde(default)]
    pub custom_user_agent: Option<String>,
    #[serde(default)]
    pub custom_referrer: Option<String>,
    #[serde(default)]
    pub http2_disabled: Option<String>,
}

/// Resolves one optional text field against the value already stored.
///
/// `submitted` is what the request carried, `stored` what the feed holds today.
/// Absent keeps `stored`; blank clears; anything else wins after trimming.
fn resolve_optional_field(submitted: Option<&str>, stored: Option<&str>) -> Option<String> {
    match submitted {
        None => stored.map(str::to_string),
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    }
}

pub async fn edit_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Form(req): Form<EditFeedForm>,
) -> impl IntoResponse {
    let edit_path = format!("/feeds/{id}/edit");
    let new_url = req.url.trim().to_string();
    if new_url.is_empty() {
        return FlashRedirect::error(edit_path, "Feed URL cannot be empty").into_response();
    }
    let user_id = auth_user.user.id;
    let new_category_id = req.category_id;

    // Editing the URL was the one way into the feed table that asked nothing of
    // the value at all — not even a scheme — so it could point the sync worker
    // at anything reachable from the server.
    let url_ok = Url::parse(&new_url).is_ok_and(|u| state.fetcher.validate(&u).is_ok());
    if !url_ok {
        return FlashRedirect::error(
            edit_path,
            "Feed URL must be an http(s) address that does not point to a private or local host",
        )
        .into_response();
    }

    let result: AppResult<()> = async {
        let f = feed::find_by_id(&state.db, id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        category::find_by_id_and_user(&state.db, f.category_id, user_id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        category::find_by_id_and_user(&state.db, new_category_id, user_id)
            .await?
            .ok_or(AppError::CategoryNotFound)?;

        // A blank title would leave the feed nameless, so it keeps the old one
        // rather than clearing.
        let title: Option<String> = match req.title.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => Some(t.to_string()),
            _ => f.title.clone(),
        };

        let description =
            resolve_optional_field(req.description.as_deref(), f.description.as_deref());
        let site_url = resolve_optional_field(req.site_url.as_deref(), f.site_url.as_deref());
        let custom_user_agent = resolve_optional_field(
            req.custom_user_agent.as_deref(),
            f.custom_user_agent.as_deref(),
        );
        let custom_referrer =
            resolve_optional_field(req.custom_referrer.as_deref(), f.custom_referrer.as_deref());

        let http2_disabled = req.http2_disabled.is_some();

        feed::update_feed(
            &state.db,
            &feed::UpdateFeedParams {
                id: f.id,
                category_id: f.category_id,
                new_category_id,
                url: &new_url,
                title: title.as_deref(),
                description: description.as_deref(),
                site_url: site_url.as_deref(),
                custom_user_agent: custom_user_agent.as_deref(),
                http2_disabled,
                custom_referrer: custom_referrer.as_deref(),
            },
        )
        .await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success(format!("/feeds/{id}/edit"), "Feed updated.").into_response()
        }
        Err(AppError::Validation(msg)) => {
            FlashRedirect::error(format!("/feeds/{id}/edit"), msg).into_response()
        }
        _ => FlashRedirect::error(format!("/feeds/{id}/edit"), "Failed to update feed")
            .into_response(),
    }
}

pub async fn delete_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let result: AppResult<()> = async {
        let f = feed::find_by_id(&state.db, id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        category::find_by_id_and_user(&state.db, f.category_id, user_id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        feed::delete_feed(&state.db, f.id, f.category_id).await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "Feed deleted.").into_response()
        }
        Err(AppError::FeedNotFound) => {
            FlashRedirect::error("/feeds", "Feed not found.").into_response()
        }
        _ => FlashRedirect::error("/feeds", "Failed to delete feed.").into_response(),
    }
}

pub async fn refresh_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let owned = async {
        let Some(f) = feed::find_by_id(&state.db, id).await? else {
            return Ok::<_, AppError>(false);
        };
        Ok(
            category::find_by_id_and_user(&state.db, f.category_id, user_id)
                .await?
                .is_some(),
        )
    }
    .await
    .unwrap_or(false);
    if !owned {
        return FlashRedirect::error("/feeds", "Feed not found").into_response();
    }
    match feed_sync::refresh_feed(
        state.db.clone(),
        id,
        &state.config.user_agent,
        &state.fetcher,
    )
    .await
    {
        Ok(r) => {
            if r.new_entries > 0 || r.updated_entries > 0 {
                state.sidebar_cache.bust(user_id);
            }
            FlashRedirect::success(
                "/feeds",
                format!(
                    "Refreshed: {} new, {} updated.",
                    r.new_entries, r.updated_entries
                ),
            )
            .into_response()
        }
        Err(e) => FlashRedirect::error("/feeds", format!("Refresh failed: {e}")).into_response(),
    }
}

pub async fn fetch_metadata_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let edit_path = format!("/feeds/{id}/edit");

    let feed_owned = async {
        let Some(f) = feed::find_by_id(&state.db, id).await? else {
            return Ok::<_, AppError>(None);
        };
        if category::find_by_id_and_user(&state.db, f.category_id, user_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        Ok(Some(f))
    }
    .await
    .ok()
    .flatten();
    let Some(feed) = feed_owned else {
        return FlashRedirect::error(edit_path, "Feed not found").into_response();
    };

    let user_agent = state.config.user_agent.clone();
    let discovered =
        match feed_discovery::discover_feed(&feed.url, &user_agent, &state.fetcher).await {
            Ok(d) => d,
            Err(e) => {
                return FlashRedirect::error(edit_path, format!("Failed to fetch metadata: {e}"))
                    .into_response();
            }
        };

    let category_id = feed.category_id;
    let result = feed::update_feed(
        &state.db,
        &feed::UpdateFeedParams {
            id: feed.id,
            category_id,
            new_category_id: category_id,
            url: &feed.url,
            title: discovered.title.as_deref().or(feed.title.as_deref()),
            description: discovered
                .description
                .as_deref()
                .or(feed.description.as_deref()),
            site_url: discovered.site_url.as_deref().or(feed.site_url.as_deref()),
            custom_user_agent: feed.custom_user_agent.as_deref(),
            http2_disabled: feed.http2_disabled,
            custom_referrer: feed.custom_referrer.as_deref(),
        },
    )
    .await;
    match result {
        Ok(_) => FlashRedirect::success(edit_path, "Metadata fetched.").into_response(),
        _ => FlashRedirect::error(edit_path, "Failed to update feed").into_response(),
    }
}

pub async fn import_opml_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // This route is multipart, so `csrf_guard` passes it through unread; the
    // token is validated here instead. Browsers inject it as the `_csrf` field
    // via `csrf.js`; programmatic clients may instead send the `X-CSRF-Token`
    // header (mirroring the patched `fetch`). Either source is accepted. The
    // whole form is read (no early break) so the field is seen wherever it sits
    // in the part order.
    let mut content = String::new();
    let mut csrf = headers
        .get(crate::middleware::CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" && name != "content" && name != "_csrf" {
            continue;
        }
        let Ok(bytes) = field.bytes().await else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if name == "_csrf" {
            let field_token = text.trim();
            if !field_token.is_empty() {
                csrf = field_token.to_string();
            }
        } else if content.trim().is_empty() && !text.trim().is_empty() {
            content = text.to_string();
        }
    }
    if !crate::secret::verify_csrf(
        &state.config.secret,
        &auth_user.session.session_token,
        &csrf,
    ) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if content.trim().is_empty() {
        return FlashRedirect::error(
            "/feeds/import",
            "Please upload a file or paste OPML content",
        )
        .into_response();
    }
    let outlines = match opml::parse_opml(&content) {
        Ok(o) => o,
        Err(e) => {
            return FlashRedirect::error("/feeds/import", format!("Failed to parse OPML: {e}"))
                .into_response();
        }
    };
    let user_id = auth_user.user.id;
    let result = opml::import_outlines(&state.db, user_id, outlines, &state.fetcher).await;
    // The import dropped its transient OPML parse tree and per-feed buffers;
    // return those freed pages to the OS now instead of waiting for the
    // allocator's lazy purge.
    crate::reclaim_memory();
    match result {
        Ok(summary) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", summary.describe()).into_response()
        }
        _ => FlashRedirect::error("/feeds/import", "Failed to import OPML").into_response(),
    }
}
