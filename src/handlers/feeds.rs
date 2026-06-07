use axum::{
    extract::{Multipart, Path, State},
    response::IntoResponse,
    Form,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::middleware::flash::FlashRedirect;
use crate::middleware::AuthUser;
use crate::models::{category, feed};
use crate::services::{feed_discovery, feed_sync, opml};
use crate::AppState;

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

    let owned = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(category::find_by_id_and_user(conn, category_id, user_id)?.is_some())
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false);
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
    let result = state
        .db
        .user(move |conn| {
            if feed::find_by_url_for_user(conn, &create_url, user_id)?.is_some() {
                return Err(AppError::FeedExists);
            }
            feed::create_feed(
                conn,
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
            )?;
            Ok::<_, AppError>(())
        })
        .await;

    match result {
        Ok(Ok(())) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "Feed added.").into_response()
        }
        Ok(Err(AppError::FeedExists)) => {
            FlashRedirect::error("/feeds", "Feed already subscribed").into_response()
        }
        Ok(Err(AppError::Validation(msg))) => FlashRedirect::error("/feeds", msg).into_response(),
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
    #[serde(default)]
    pub _clear_referrer: Option<String>,
    #[serde(default)]
    pub _clear_user_agent: Option<String>,
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

    let result = state
        .db
        .user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;
            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;
            category::find_by_id_and_user(conn, new_category_id, user_id)?
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

            let custom_user_agent: Option<String> = if req._clear_user_agent.is_some() {
                None
            } else {
                let trimmed = req.custom_user_agent.trim();
                if trimmed.is_empty() {
                    f.custom_user_agent.clone()
                } else {
                    Some(trimmed.to_string())
                }
            };

            let custom_referrer: Option<String> = if req._clear_referrer.is_some() {
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
                conn,
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
            )?;
            Ok::<_, AppError>(())
        })
        .await;

    match result {
        Ok(Ok(())) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success(format!("/feeds/{id}/edit"), "Feed updated.").into_response()
        }
        Ok(Err(AppError::Validation(msg))) => {
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
    let result = state
        .db
        .user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;
            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;
            feed::delete_feed(conn, f.id, f.category_id)?;
            Ok::<_, AppError>(())
        })
        .await;
    match result {
        Ok(Ok(())) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "Feed deleted.").into_response()
        }
        Ok(Err(AppError::FeedNotFound)) => {
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
    let owned = state
        .db
        .read_user(move |conn| {
            let f = match feed::find_by_id(conn, id)? {
                Some(f) => f,
                None => return Ok::<_, AppError>(false),
            };
            Ok(category::find_by_id_and_user(conn, f.category_id, user_id)?.is_some())
        })
        .await
        .ok()
        .and_then(|r| r.ok())
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

    let feed_owned = state
        .db
        .read_user(move |conn| {
            let f = match feed::find_by_id(conn, id)? {
                Some(f) => f,
                None => return Ok::<_, AppError>(None),
            };
            if category::find_by_id_and_user(conn, f.category_id, user_id)?.is_none() {
                return Ok(None);
            }
            Ok(Some(f))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten();
    let feed = match feed_owned {
        Some(f) => f,
        None => return FlashRedirect::error(edit_path, "Feed not found").into_response(),
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
    let result = state
        .db
        .user(move |conn| {
            feed::update_feed(
                conn,
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
            )?;
            Ok::<_, AppError>(())
        })
        .await;
    match result {
        Ok(Ok(())) => FlashRedirect::success(edit_path, "Metadata fetched.").into_response(),
        _ => FlashRedirect::error(edit_path, "Failed to update feed").into_response(),
    }
}

pub async fn import_opml_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut content = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" && name != "content" {
            continue;
        }
        let bytes = match field.bytes().await {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.is_empty() {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if !text.trim().is_empty() {
                content = text.to_string();
                break;
            }
        }
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
    let result = state
        .db
        .user(move |conn| {
            for outline in outlines {
                let cat =
                    match category::find_by_name_and_user(conn, &outline.category_name, user_id)? {
                        Some(cat) => cat,
                        None => category::create_category(conn, user_id, &outline.category_name)?,
                    };
                for opml_feed in outline.feeds {
                    if feed::find_by_url_and_category(conn, &opml_feed.xml_url, cat.id)?.is_some() {
                        continue;
                    }
                    let _ = feed::create_feed(
                        conn,
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
                    );
                }
            }
            Ok::<_, AppError>(())
        })
        .await;
    // The import dropped its transient OPML parse tree and per-feed buffers;
    // return those freed pages to the OS now instead of waiting for the
    // allocator's lazy purge.
    crate::reclaim_memory();
    match result {
        Ok(Ok(())) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/feeds", "OPML imported.").into_response()
        }
        _ => FlashRedirect::error("/feeds/import", "Failed to import OPML").into_response(),
    }
}
