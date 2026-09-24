use axum::{
    Form,
    extract::{Multipart, Path, State, rejection::FormRejection},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::handlers::return_to::{FEEDS_LIST, feeds_list, return_to_query};
use crate::middleware::AuthUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::{category, feed};
use crate::services::{feed_discovery, feed_sync, opml};
use url::Url;

// Form-POST endpoints for the SSR /feeds page, answered with FlashRedirect.
// Each lands back on the filtered list it came from (`return_to`).

/// A form whose only field besides `_csrf` is where to land afterwards.
#[derive(Debug, Deserialize)]
pub struct ReturnToForm {
    #[serde(default)]
    pub return_to: Option<String>,
}

/// `return_to` from a bodyless-tolerant form: a POST without a form body (an
/// API client, a test) still lands on the bare list.
fn back_from(form: Result<Form<ReturnToForm>, FormRejection>) -> String {
    let raw = form.ok().and_then(|Form(f)| f.return_to);
    feeds_list(raw.as_deref())
}

/// `/feeds/{id}/edit`, carrying `back` unless it is the bare list.
fn edit_path(id: i64, back: &str) -> String {
    if back == FEEDS_LIST {
        format!("/feeds/{id}/edit")
    } else {
        format!("/feeds/{id}/edit?{}", return_to_query(back))
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateFeedForm {
    pub url: String,
    pub category_id: i64,
    #[serde(default)]
    pub return_to: Option<String>,
}

pub async fn create_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<CreateFeedForm>,
) -> impl IntoResponse {
    let back = feeds_list(req.return_to.as_deref());
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return FlashRedirect::error(back, "Feed URL cannot be empty").into_response();
    }
    let user_id = auth_user.user.id;
    let category_id = req.category_id;
    let user_agent = state.config.user_agent.clone();

    let owned = category::find_by_id_and_user(&state.db, category_id, user_id)
        .await
        .is_ok_and(|c| c.is_some());
    if !owned {
        return FlashRedirect::error(back, "Invalid category").into_response();
    }

    let discovered = match feed_discovery::discover_feed(&url, &user_agent, &state.fetcher).await {
        Ok(d) => d,
        Err(e) => {
            return FlashRedirect::error(back, format!("Failed to discover feed: {e}"))
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
            FlashRedirect::success(back, "Feed added.").into_response()
        }
        Err(AppError::FeedExists) => {
            FlashRedirect::error(back, "Feed already subscribed").into_response()
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error(back, msg).into_response(),
        _ => FlashRedirect::error(back, "Failed to add feed").into_response(),
    }
}

/// Optional text fields: absent keeps the stored value, blank erases, anything
/// else is trimmed and stored. Exception: a blank `title` keeps the old one.
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
    #[serde(default)]
    pub return_to: Option<String>,
}

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
    // Success returns to the list; errors stay here, still carrying `back`.
    let back = feeds_list(req.return_to.as_deref());
    let edit_path = edit_path(id, &back);
    let new_url = req.url.trim().to_string();
    if new_url.is_empty() {
        return FlashRedirect::error(edit_path, "Feed URL cannot be empty").into_response();
    }
    let user_id = auth_user.user.id;
    let new_category_id = req.category_id;

    // SSRF-validate: the sync worker will fetch this URL.
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
            FlashRedirect::success(back, "Feed updated.").into_response()
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error(edit_path, msg).into_response(),
        _ => FlashRedirect::error(edit_path, "Failed to update feed").into_response(),
    }
}

pub async fn delete_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    form: Result<Form<ReturnToForm>, FormRejection>,
) -> impl IntoResponse {
    let back = back_from(form);
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
            FlashRedirect::success(back, "Feed deleted.").into_response()
        }
        Err(AppError::FeedNotFound) => {
            FlashRedirect::error(back, "Feed not found.").into_response()
        }
        _ => FlashRedirect::error(back, "Failed to delete feed.").into_response(),
    }
}

pub async fn refresh_feed_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    form: Result<Form<ReturnToForm>, FormRejection>,
) -> impl IntoResponse {
    let back = back_from(form);
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
        return FlashRedirect::error(back, "Feed not found").into_response();
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
                back,
                format!(
                    "Refreshed: {} new, {} updated.",
                    r.new_entries, r.updated_entries
                ),
            )
            .into_response()
        }
        Err(e) => FlashRedirect::error(back, format!("Refresh failed: {e}")).into_response(),
    }
}

pub async fn fetch_metadata_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    form: Result<Form<ReturnToForm>, FormRejection>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let back = back_from(form);
    let edit_path = edit_path(id, &back);

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
        // No edit page to return to.
        return FlashRedirect::error(back, "Feed not found").into_response();
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
    // Multipart bypasses `csrf_guard`, so CSRF is checked here: `_csrf` field
    // or `X-CSRF-Token` header. Read every part so the field is found anywhere.
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
    // Return the import's freed buffers to the OS now.
    crate::reclaim_memory();
    match result {
        Ok(summary) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", summary.describe()).into_response()
        }
        _ => FlashRedirect::error("/feeds/import", "Failed to import OPML").into_response(),
    }
}
