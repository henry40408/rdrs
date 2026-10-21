use axum::{
    Form, Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::{category, feed, image};
use crate::services::{feed_discovery, opml};

use super::auth::GReaderUser;
use super::types::{Subscription, SubscriptionCategory, SubscriptionListResponse};

/// `GET /reader/api/0/subscription/list`
pub async fn subscription_list(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<Json<SubscriptionListResponse>> {
    let user_id = auth.user.id;
    let feeds = feed::list_by_user(&state.db, user_id).await?;
    let categories = category::list_by_user(&state.db, user_id).await?;

    // Resolve which feeds have an icon in one query instead of one per feed.
    let feed_ids: Vec<i64> = feeds.iter().map(|f| f.id).collect();
    let feeds_with_icon = image::existing_ids(&state.db, image::ENTITY_FEED, &feed_ids).await?;

    let subscriptions: Vec<Subscription> = feeds
        .into_iter()
        .map(|f| {
            let cat = categories.iter().find(|c| c.id == f.category_id);
            let cat_name = cat.map_or("Uncategorized", |c| c.name.as_str());
            let cat_id = cat.map_or(0, |c| c.id);

            let has_icon = feeds_with_icon.contains(&f.id);
            let icon_url = if has_icon {
                format!("/api/feeds/{}/icon", f.id)
            } else {
                String::new()
            };

            Subscription {
                id: format!("feed/{}", f.url),
                title: f.title.unwrap_or_else(|| f.url.clone()),
                categories: vec![SubscriptionCategory {
                    id: format!("user/-/label/{cat_name}"),
                    label: cat_name.to_string(),
                }],
                sort_id: format!("{cat_id:08x}"),
                html_url: f.site_url.clone().unwrap_or_default(),
                url: f.url,
                icon_url,
                // RDRS extensions
                feed_id: f.id,
                fetch_error: f.fetch_error,
                description: f.description,
                custom_user_agent: f.custom_user_agent,
                http2_disabled: f.http2_disabled,
                custom_referrer: f.custom_referrer,
            }
        })
        .collect();

    Ok(Json(SubscriptionListResponse { subscriptions }))
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionEditForm {
    /// Action: "subscribe", "edit", "unsubscribe"
    pub ac: String,
    /// Stream ID: `feed/<url>`
    pub s: Option<String>,
    pub t: Option<String>,
    /// Add label/category: `user/-/label/<name>`
    pub a: Option<String>,
    /// Remove label/category: `user/-/label/<name>`
    pub r: Option<String>,
    /// POST token (optional, skipped for cookie auth)
    #[serde(rename = "T")]
    pub token: Option<String>,
    // RDRS extension fields (optional, ignored by standard GReader clients)
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: Option<bool>,
    pub custom_referrer: Option<String>,
}

/// The feed URL of the `s` stream ID (`feed/<url>`).
fn feed_url_from_stream(stream_id: Option<&str>) -> AppResult<String> {
    let stream_id =
        stream_id.ok_or_else(|| AppError::Validation("Missing stream ID (s parameter)".into()))?;
    let feed_url = stream_id
        .strip_prefix("feed/")
        .ok_or_else(|| AppError::Validation("Stream ID must start with feed/".into()))?;
    Ok(feed_url.to_string())
}

async fn category_id_for_label(db: &Db, user_id: i64, label_name: &str) -> AppResult<i64> {
    if let Some(cat) = category::find_by_name_and_user(db, label_name, user_id).await? {
        return Ok(cat.id);
    }
    Ok(category::create_category(db, user_id, label_name).await?.id)
}

/// The user's first category, creating "Uncategorized" for a user with none.
async fn first_or_uncategorized_category_id(db: &Db, user_id: i64) -> AppResult<i64> {
    let cats = category::list_by_user(db, user_id).await?;
    if let Some(first) = cats.first() {
        return Ok(first.id);
    }
    Ok(category::create_category(db, user_id, "Uncategorized")
        .await?
        .id)
}

async fn subscribe(state: &AppState, user_id: i64, form: &SubscriptionEditForm) -> AppResult<()> {
    let feed_url = feed_url_from_stream(form.s.as_deref())?;
    if feed_url.is_empty() {
        return Err(AppError::Validation("Empty feed URL".into()));
    }

    let label = extract_label_name(form.a.as_deref());
    let user_agent = state.config.user_agent.clone();

    let discovered = feed_discovery::discover_feed(&feed_url, &user_agent, &state.fetcher).await?;

    let category_id = match label {
        Some(label_name) => category_id_for_label(&state.db, user_id, &label_name).await?,
        None => first_or_uncategorized_category_id(&state.db, user_id).await?,
    };

    if feed::find_by_url_for_user(&state.db, &discovered.feed_url, user_id)
        .await?
        .is_some()
    {
        return Err(AppError::FeedExists);
    }

    feed::create_feed(
        &state.db,
        &feed::CreateFeedParams {
            category_id,
            url: &discovered.feed_url,
            title: form.t.as_deref().or(discovered.title.as_deref()),
            description: discovered.description.as_deref(),
            site_url: discovered.site_url.as_deref(),
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await?;
    Ok(())
}

/// Fields the form leaves out keep the feed's current value. The exception is
/// `custom_referrer`: present but empty clears it.
async fn edit_subscription(
    state: &AppState,
    user_id: i64,
    form: &SubscriptionEditForm,
) -> AppResult<()> {
    let feed_url = feed_url_from_stream(form.s.as_deref())?;

    let f = feed::find_by_url_for_user(&state.db, &feed_url, user_id)
        .await?
        .ok_or(AppError::FeedNotFound)?;

    let new_category_id = match extract_label_name(form.a.as_deref()) {
        Some(label_name) => category_id_for_label(&state.db, user_id, &label_name).await?,
        None => f.category_id,
    };

    let custom_referrer = match form.custom_referrer.as_deref() {
        Some(referrer) => Some(referrer).filter(|s| !s.is_empty()),
        None => f.custom_referrer.as_deref(),
    };

    feed::update_feed(
        &state.db,
        &feed::UpdateFeedParams {
            id: f.id,
            category_id: f.category_id,
            new_category_id,
            url: &f.url,
            title: form.t.as_deref().or(f.title.as_deref()),
            description: form.description.as_deref().or(f.description.as_deref()),
            site_url: form.site_url.as_deref().or(f.site_url.as_deref()),
            custom_user_agent: form
                .custom_user_agent
                .as_deref()
                .or(f.custom_user_agent.as_deref()),
            http2_disabled: form.http2_disabled.unwrap_or(f.http2_disabled),
            custom_referrer,
        },
    )
    .await?;
    Ok(())
}

async fn unsubscribe(state: &AppState, user_id: i64, form: &SubscriptionEditForm) -> AppResult<()> {
    let feed_url = feed_url_from_stream(form.s.as_deref())?;

    let f = feed::find_by_url_for_user(&state.db, &feed_url, user_id)
        .await?
        .ok_or(AppError::FeedNotFound)?;
    feed::delete_feed(&state.db, f.id, f.category_id).await?;
    Ok(())
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
        "subscribe" => subscribe(&state, user_id, &form).await?,
        "edit" => edit_subscription(&state, user_id, &form).await?,
        "unsubscribe" => unsubscribe(&state, user_id, &form).await?,
        _ => return Err(AppError::Validation(format!("Unknown action: {}", form.ac))),
    }
    Ok("OK".to_string())
}

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

    let discovered = feed_discovery::discover_feed(&url, &user_agent, &state.fetcher).await?;
    let feed_url = discovered.feed_url.clone();

    if feed::find_by_url_for_user(&state.db, &discovered.feed_url, user_id)
        .await?
        .is_some()
    {
        return Err(AppError::FeedExists);
    }

    let category_id = first_or_uncategorized_category_id(&state.db, user_id).await?;

    feed::create_feed(
        &state.db,
        &feed::CreateFeedParams {
            category_id,
            url: &discovered.feed_url,
            title: discovered.title.as_deref(),
            description: discovered.description.as_deref(),
            site_url: discovered.site_url.as_deref(),
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await?;

    Ok(Json(QuickAddResponse {
        num_results: 1,
        query: url,
        stream_id: format!("feed/{feed_url}"),
    }))
}

/// `GET /reader/api/0/subscription/export`
pub async fn export(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<impl IntoResponse> {
    let user_id = auth.user.id;
    let categories = category::list_by_user(&state.db, user_id).await?;
    let feeds = feed::list_by_user(&state.db, user_id).await?;
    let opml_content = opml::export_opml(&categories, &feeds);

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

/// `POST /reader/api/0/subscription/import`
///
/// Body is raw OPML (application/xml).
pub async fn import(
    auth: GReaderUser,
    State(state): State<AppState>,
    body: String,
) -> AppResult<([(&'static str, String); 1], String)> {
    let outlines = opml::parse_opml(&body)?;
    let summary = opml::import_outlines(&state.db, auth.user.id, outlines, &state.fetcher).await?;
    Ok(super::ok_with_affected(
        i64::try_from(summary.feeds_added).unwrap_or(i64::MAX),
    ))
}

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
    let is_subscribed = feed::find_by_url_for_user(&state.db, &feed_url, user_id)
        .await?
        .is_some();

    Ok(if is_subscribed {
        "true".to_string()
    } else {
        "false".to_string()
    })
}

/// Extract label name from a `user/-/label/<name>` string.
fn extract_label_name(s: Option<&str>) -> Option<String> {
    s.and_then(|s| {
        // Handle "user/-/label/<name>" or "user/<id>/label/<name>"
        if let Some(after_user) = s.strip_prefix("user/") {
            if let Some(rest) = s.strip_prefix("user/-/label/") {
                return Some(rest.to_string());
            }
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
        // Cookie requests already passed `csrf_guard`; the web UI sends
        // `X-CSRF-Token`, not `T`.
        return Ok(());
    }

    if let Some(post_token) = token {
        super::auth::verify_post_token(
            &state.config.secret,
            auth.credential.post_token_subject(),
            post_token,
        )?;
    }
    // Many clients omit the POST token, so it stays optional.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_label_name;

    #[test]
    fn extract_label_name_dash_user() {
        assert_eq!(
            extract_label_name(Some("user/-/label/Foo")),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn extract_label_name_numeric_user() {
        assert_eq!(
            extract_label_name(Some("user/12345/label/Foo")),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn extract_label_name_none_input() {
        assert_eq!(extract_label_name(None), None);
    }

    #[test]
    fn extract_label_name_invalid_prefix() {
        assert_eq!(extract_label_name(Some("tag/something")), None);
    }
}
