use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Form, Json,
};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::models::{category, feed, image};
use crate::services::{feed_discovery, opml};
use crate::AppState;

use super::auth::GReaderUser;
use super::types::{Subscription, SubscriptionCategory, SubscriptionListResponse};

/// `GET /reader/api/0/subscription/list`
pub async fn subscription_list(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<Json<SubscriptionListResponse>> {
    let user_id = auth.user.id;
    let subscriptions = state
        .db
        .user(move |conn| {
            let feeds = feed::list_by_user(conn, user_id)?;
            let categories = category::list_by_user(conn, user_id)?;

            let subs: Vec<Subscription> = feeds
                .into_iter()
                .map(|f| {
                    let cat = categories.iter().find(|c| c.id == f.category_id);
                    let cat_name = cat.map(|c| c.name.as_str()).unwrap_or("Uncategorized");
                    let cat_id = cat.map(|c| c.id).unwrap_or(0);

                    let has_icon =
                        image::exists(conn, image::ENTITY_FEED, f.id).unwrap_or(false);
                    let icon_url = if has_icon {
                        format!("/api/feeds/{}/icon", f.id)
                    } else {
                        String::new()
                    };

                    Subscription {
                        id: format!("feed/{}", f.url),
                        title: f.title.unwrap_or_else(|| f.url.clone()),
                        categories: vec![SubscriptionCategory {
                            id: format!("user/-/label/{}", cat_name),
                            label: cat_name.to_string(),
                        }],
                        sort_id: format!("{:08x}", cat_id),
                        html_url: f.site_url.unwrap_or_default(),
                        url: f.url,
                        icon_url,
                    }
                })
                .collect();

            Ok::<_, AppError>(subs)
        })
        .await??;

    Ok(Json(SubscriptionListResponse { subscriptions }))
}

// --- subscription/edit ---

#[derive(Debug, Deserialize)]
pub struct SubscriptionEditForm {
    /// Action: "subscribe", "edit", "unsubscribe"
    pub ac: String,
    /// Stream ID: "feed/<url>"
    pub s: Option<String>,
    /// Title
    pub t: Option<String>,
    /// Add label/category: "user/-/label/<name>"
    pub a: Option<String>,
    /// Remove label/category: "user/-/label/<name>"
    pub r: Option<String>,
    /// POST token (optional, skipped for cookie auth)
    #[serde(rename = "T")]
    pub token: Option<String>,
}

/// `POST /reader/api/0/subscription/edit`
pub async fn subscription_edit(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form): Form<SubscriptionEditForm>,
) -> AppResult<String> {
    verify_post_token_if_needed(&auth, &state, form.token.as_deref())?;

    let user_id = auth.user.id;

    match form.ac.as_str() {
        "subscribe" => {
            let stream_id = form.s.as_deref().ok_or_else(|| {
                AppError::Validation("Missing stream ID (s parameter)".into())
            })?;
            let feed_url = stream_id
                .strip_prefix("feed/")
                .ok_or_else(|| AppError::Validation("Stream ID must start with feed/".into()))?
                .to_string();

            if feed_url.is_empty() {
                return Err(AppError::Validation("Empty feed URL".into()));
            }

            let title = form.t.clone();
            let label = extract_label_name(form.a.as_deref());
            let user_agent = state.config.user_agent.clone();

            // Discover feed metadata
            let discovered = feed_discovery::discover_feed(&feed_url, &user_agent).await?;

            state
                .db
                .user(move |conn| {
                    // Find or create category
                    let category_id = if let Some(label_name) = label {
                        match category::find_by_name_and_user(conn, &label_name, user_id)? {
                            Some(cat) => cat.id,
                            None => category::create_category(conn, user_id, &label_name)?.id,
                        }
                    } else {
                        // Use first category or create "Uncategorized"
                        let cats = category::list_by_user(conn, user_id)?;
                        if let Some(first) = cats.first() {
                            first.id
                        } else {
                            category::create_category(conn, user_id, "Uncategorized")?.id
                        }
                    };

                    // Check if feed already exists for this user (across all categories)
                    if let Some(_existing) =
                        feed::find_by_url_for_user(conn, &discovered.feed_url, user_id)?
                    {
                        return Err(AppError::FeedExists);
                    }

                    feed::create_feed(
                        conn,
                        category_id,
                        &discovered.feed_url,
                        title
                            .as_deref()
                            .or(discovered.title.as_deref()),
                        discovered.description.as_deref(),
                        discovered.site_url.as_deref(),
                        None,
                        None,
                    )?;

                    Ok::<_, AppError>(())
                })
                .await??;

            Ok("OK".to_string())
        }
        "edit" => {
            let stream_id = form.s.as_deref().ok_or_else(|| {
                AppError::Validation("Missing stream ID (s parameter)".into())
            })?;
            let feed_url = stream_id
                .strip_prefix("feed/")
                .ok_or_else(|| AppError::Validation("Stream ID must start with feed/".into()))?
                .to_string();

            let title = form.t.clone();
            let add_label = extract_label_name(form.a.as_deref());
            let _remove_label = extract_label_name(form.r.as_deref());

            state
                .db
                .user(move |conn| {
                    let f = feed::find_by_url_for_user(conn, &feed_url, user_id)?
                        .ok_or(AppError::FeedNotFound)?;

                    // Determine new category if label is being changed
                    let new_category_id = if let Some(label_name) = add_label {
                        match category::find_by_name_and_user(conn, &label_name, user_id)? {
                            Some(cat) => cat.id,
                            None => category::create_category(conn, user_id, &label_name)?.id,
                        }
                    } else {
                        f.category_id
                    };

                    feed::update_feed(
                        conn,
                        f.id,
                        f.category_id,
                        new_category_id,
                        &f.url,
                        title.as_deref().or(f.title.as_deref()),
                        f.description.as_deref(),
                        f.site_url.as_deref(),
                        f.custom_user_agent.as_deref(),
                        f.http2_disabled,
                    )?;

                    Ok::<_, AppError>(())
                })
                .await??;

            Ok("OK".to_string())
        }
        "unsubscribe" => {
            let stream_id = form.s.as_deref().ok_or_else(|| {
                AppError::Validation("Missing stream ID (s parameter)".into())
            })?;
            let feed_url = stream_id
                .strip_prefix("feed/")
                .ok_or_else(|| AppError::Validation("Stream ID must start with feed/".into()))?
                .to_string();

            state
                .db
                .user(move |conn| {
                    let f = feed::find_by_url_for_user(conn, &feed_url, user_id)?
                        .ok_or(AppError::FeedNotFound)?;
                    feed::delete_feed(conn, f.id, f.category_id)?;
                    Ok::<_, AppError>(())
                })
                .await??;

            Ok("OK".to_string())
        }
        _ => Err(AppError::Validation(format!(
            "Unknown action: {}",
            form.ac
        ))),
    }
}

// --- quickadd ---

#[derive(Debug, Deserialize)]
pub struct QuickAddForm {
    /// Feed URL
    #[serde(rename = "quickadd")]
    pub quick_add: String,
    /// POST token
    #[serde(rename = "T")]
    pub token: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct QuickAddResponse {
    #[serde(rename = "numResults")]
    pub num_results: i32,
    pub query: String,
    #[serde(rename = "streamId")]
    pub stream_id: String,
}

/// `POST /reader/api/0/subscription/quickadd`
pub async fn quickadd(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form): Form<QuickAddForm>,
) -> AppResult<Json<QuickAddResponse>> {
    verify_post_token_if_needed(&auth, &state, form.token.as_deref())?;

    let url = form.quick_add.trim().to_string();
    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".into()));
    }

    let user_id = auth.user.id;
    let user_agent = state.config.user_agent.clone();

    // Discover feed
    let discovered = feed_discovery::discover_feed(&url, &user_agent).await?;
    let feed_url = discovered.feed_url.clone();

    state
        .db
        .user(move |conn| {
            // Check if already subscribed
            if feed::find_by_url_for_user(conn, &discovered.feed_url, user_id)?.is_some() {
                return Err(AppError::FeedExists);
            }

            // Use first category or create "Uncategorized"
            let cats = category::list_by_user(conn, user_id)?;
            let category_id = if let Some(first) = cats.first() {
                first.id
            } else {
                category::create_category(conn, user_id, "Uncategorized")?.id
            };

            feed::create_feed(
                conn,
                category_id,
                &discovered.feed_url,
                discovered.title.as_deref(),
                discovered.description.as_deref(),
                discovered.site_url.as_deref(),
                None,
                None,
            )?;

            Ok::<_, AppError>(())
        })
        .await??;

    Ok(Json(QuickAddResponse {
        num_results: 1,
        query: url,
        stream_id: format!("feed/{}", feed_url),
    }))
}

// --- export ---

/// `GET /reader/api/0/subscription/export`
pub async fn export(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let user_id = auth.user.id;
    let opml_content = state
        .db
        .user(move |conn| {
            let categories = category::list_by_user(conn, user_id)?;
            let feeds = feed::list_by_user(conn, user_id)?;
            Ok::<_, AppError>(opml::export_opml(&categories, &feeds))
        })
        .await??;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"subscriptions.opml\"",
            ),
        ],
        opml_content,
    ))
}

// --- import ---

/// `POST /reader/api/0/subscription/import`
///
/// Accepts OPML content as the request body (application/xml).
pub async fn import(
    auth: GReaderUser,
    State(state): State<AppState>,
    body: String,
) -> AppResult<String> {
    let outlines = opml::parse_opml(&body)?;

    let user_id = auth.user.id;
    state
        .db
        .user(move |conn| {
            for outline in outlines {
                let cat =
                    match category::find_by_name_and_user(conn, &outline.category_name, user_id)? {
                        Some(cat) => cat,
                        None => {
                            category::create_category(conn, user_id, &outline.category_name)?
                        }
                    };

                for opml_feed in outline.feeds {
                    if feed::find_by_url_and_category(conn, &opml_feed.xml_url, cat.id)?.is_some() {
                        continue;
                    }

                    let _ = feed::create_feed(
                        conn,
                        cat.id,
                        &opml_feed.xml_url,
                        opml_feed.title.as_deref(),
                        None,
                        opml_feed.html_url.as_deref(),
                        None,
                        None,
                    );
                }
            }
            Ok::<_, AppError>(())
        })
        .await??;

    Ok("OK".to_string())
}

// --- subscribed ---

#[derive(Debug, Deserialize)]
pub struct SubscribedQuery {
    pub s: String,
}

/// `GET /reader/api/0/subscribed`
pub async fn subscribed(
    auth: GReaderUser,
    State(state): State<AppState>,
    Query(query): Query<SubscribedQuery>,
) -> AppResult<String> {
    let feed_url = query
        .s
        .strip_prefix("feed/")
        .ok_or_else(|| AppError::Validation("Stream ID must start with feed/".into()))?
        .to_string();

    let user_id = auth.user.id;
    let is_subscribed = state
        .db
        .user(move |conn| {
            Ok::<_, AppError>(feed::find_by_url_for_user(conn, &feed_url, user_id)?.is_some())
        })
        .await??;

    Ok(if is_subscribed {
        "true".to_string()
    } else {
        "false".to_string()
    })
}

// --- Helpers ---

/// Extract label name from a `user/-/label/<name>` string.
fn extract_label_name(s: Option<&str>) -> Option<String> {
    s.and_then(|s| {
        // Handle "user/-/label/<name>" or "user/<id>/label/<name>"
        if let Some(after_user) = s.strip_prefix("user/") {
            if let Some(rest) = s.strip_prefix("user/-/label/") {
                return Some(rest.to_string());
            }
            // Handle "user/<numeric>/label/<name>"
            if let Some(pos) = after_user.find("/label/") {
                return Some(after_user[pos + 7..].to_string());
            }
        }
        None
    })
}

/// Verify POST token if request is not via cookie auth.
fn verify_post_token_if_needed(
    auth: &GReaderUser,
    state: &AppState,
    token: Option<&str>,
) -> AppResult<()> {
    if auth.via_cookie {
        // Cookie auth has SameSite protection, skip POST token
        return Ok(());
    }

    if let Some(post_token) = token {
        super::auth::verify_post_token(
            &state.config.image_proxy_secret,
            &auth.session.session_token,
            post_token,
        )?;
    }
    // Many clients don't send POST token, so we allow it for now
    Ok(())
}
