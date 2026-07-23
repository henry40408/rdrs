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

    let discovered = match feed_discovery::discover_feed(&url, &user_agent).await {
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

#[derive(Debug, Deserialize)]
pub struct EditFeedForm {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub site_url: String,
    pub category_id: i64,
    #[serde(default)]
    pub custom_user_agent: String,
    #[serde(default)]
    pub custom_referrer: String,
    #[serde(default)]
    pub http2_disabled: Option<String>,
    #[serde(default, rename = "_clear_referrer")]
    pub clear_referrer: Option<String>,
    #[serde(default, rename = "_clear_user_agent")]
    pub clear_user_agent: Option<String>,
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

        let trimmed_title = req.title.trim();
        let title: Option<String> = if trimmed_title.is_empty() {
            f.title.clone()
        } else {
            Some(trimmed_title.to_string())
        };

        let trimmed_desc = req.description.trim();
        let description: Option<String> = if trimmed_desc.is_empty() {
            None
        } else {
            Some(trimmed_desc.to_string())
        };

        let trimmed_site = req.site_url.trim();
        let site_url: Option<String> = if trimmed_site.is_empty() {
            None
        } else {
            Some(trimmed_site.to_string())
        };

        let custom_user_agent: Option<String> = if req.clear_user_agent.is_some() {
            None
        } else {
            let trimmed = req.custom_user_agent.trim();
            if trimmed.is_empty() {
                f.custom_user_agent.clone()
            } else {
                Some(trimmed.to_string())
            }
        };

        let custom_referrer: Option<String> = if req.clear_referrer.is_some() {
            None
        } else {
            let trimmed = req.custom_referrer.trim();
            if trimmed.is_empty() {
                f.custom_referrer.clone()
            } else {
                Some(trimmed.to_string())
            }
        };

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
    match feed_sync::refresh_feed(state.db.clone(), id, &state.config.user_agent).await {
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
    let discovered = match feed_discovery::discover_feed(&feed.url, &user_agent).await {
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
    let result: AppResult<()> = async {
        for outline in outlines {
            let cat =
                match category::find_by_name_and_user(&state.db, &outline.category_name, user_id)
                    .await?
                {
                    Some(cat) => cat,
                    None => {
                        category::create_category(&state.db, user_id, &outline.category_name)
                            .await?
                    }
                };
            for opml_feed in outline.feeds {
                if feed::find_by_url_and_category(&state.db, &opml_feed.xml_url, cat.id)
                    .await?
                    .is_some()
                {
                    continue;
                }
                let _ = feed::create_feed(
                    &state.db,
                    &feed::CreateFeedParams {
                        category_id: cat.id,
                        url: &opml_feed.xml_url,
                        title: opml_feed.title.as_deref(),
                        description: None,
                        site_url: opml_feed.html_url.as_deref(),
                        custom_user_agent: None,
                        http2_disabled: None,
                        custom_referrer: None,
                    },
                )
                .await;
            }
        }
        Ok(())
    }
    .await;
    // The import dropped its transient OPML parse tree and per-feed buffers;
    // return those freed pages to the OS now instead of waiting for the
    // allocator's lazy purge.
    crate::reclaim_memory();
    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "OPML imported.").into_response()
        }
        _ => FlashRedirect::error("/feeds/import", "Failed to import OPML").into_response(),
    }
}
