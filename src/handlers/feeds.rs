use axum::{
    extract::{Query, State},
    Json,
};
use serde::Serialize;

use crate::error::AppResult;
use crate::handlers::pages::{compute_freshness, format_relative_time, FeedsQuery};
use crate::middleware::AuthUser;
use crate::models::{category, entry, feed};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct FeedDto {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub category_id: i64,
    pub category_name: String,
    pub has_icon: bool,
    pub fetch_error: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: bool,
    pub custom_referrer: Option<String>,
    pub unread_count: i64,
    pub fetched_at_relative: String,
    pub fetched_at_datetime: String,
    pub feed_updated_at_relative: String,
    pub feed_updated_at_datetime: String,
    pub freshness_class: String,
    pub freshness_key: String,
}

#[derive(Debug, Serialize)]
pub struct CategoryOptionDto {
    pub id: i64,
    pub name: String,
    pub feed_count: usize,
}

#[derive(Debug, Serialize)]
pub struct FeedsResponse {
    pub feeds: Vec<FeedDto>,
    pub categories: Vec<CategoryOptionDto>,
    pub total_feed_count: usize,
    pub active_filter: String,
    pub active_sort: String,
    pub active_category: Option<i64>,
}

/// Returns the feeds list payload that previously powered the SSR `/feeds`
/// page. Filtering by category / `errors` / `stale` and sorting by
/// title / unread / category are applied server-side so the URL stays the
/// stable source of truth (matches the old SSR behaviour and means
/// `<select onchange>` can keep doing a full reload to a different URL).
pub async fn list_feeds(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Query(query): Query<FeedsQuery>,
) -> AppResult<Json<FeedsResponse>> {
    let user_id = auth_user.user.id;

    let (mut feeds, categories) = state
        .db
        .read_user(move |c| {
            let cats = category::list_by_user(c, user_id).unwrap_or_default();
            let all_feeds = feed::list_by_user(c, user_id).unwrap_or_default();
            let unread_map = entry::count_unread_by_feed(c, user_id).unwrap_or_default();

            let cat_map: std::collections::HashMap<i64, String> = cats
                .iter()
                .map(|cat| (cat.id, cat.name.clone()))
                .collect();

            let mut feed_count_by_cat: std::collections::HashMap<i64, usize> =
                std::collections::HashMap::new();
            for f in &all_feeds {
                *feed_count_by_cat.entry(f.category_id).or_insert(0) += 1;
            }

            let feed_dtos: Vec<FeedDto> = all_feeds
                .into_iter()
                .map(|f| {
                    let has_icon: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                            [f.id],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
                    let (updated_rel, updated_dt) = if f.feed_updated_at.is_some() {
                        format_relative_time(f.feed_updated_at)
                    } else if f
                        .fetched_at
                        .map(|ft| (chrono::Utc::now() - ft).num_days() <= 30)
                        .unwrap_or(false)
                    {
                        ("No date info".to_string(), String::new())
                    } else {
                        ("Never".to_string(), String::new())
                    };
                    let (freshness_class, freshness_key) =
                        compute_freshness(f.feed_updated_at, f.fetched_at);
                    FeedDto {
                        title: f.title.clone().unwrap_or_else(|| f.url.clone()),
                        category_name: cat_map
                            .get(&f.category_id)
                            .cloned()
                            .unwrap_or_else(|| "Unknown".to_string()),
                        has_icon: has_icon > 0,
                        unread_count: *unread_map.get(&f.id).unwrap_or(&0),
                        id: f.id,
                        url: f.url,
                        category_id: f.category_id,
                        fetch_error: f.fetch_error,
                        description: f.description,
                        site_url: f.site_url,
                        custom_user_agent: f.custom_user_agent,
                        http2_disabled: f.http2_disabled,
                        custom_referrer: f.custom_referrer,
                        fetched_at_relative: fetched_rel,
                        fetched_at_datetime: fetched_dt,
                        feed_updated_at_relative: updated_rel,
                        feed_updated_at_datetime: updated_dt,
                        freshness_class,
                        freshness_key,
                    }
                })
                .collect();

            let cat_options: Vec<CategoryOptionDto> = cats
                .into_iter()
                .map(|cat| CategoryOptionDto {
                    feed_count: feed_count_by_cat.get(&cat.id).copied().unwrap_or(0),
                    id: cat.id,
                    name: cat.name,
                })
                .collect();

            Ok::<_, crate::error::AppError>((feed_dtos, cat_options))
        })
        .await??;

    let active_filter = query.filter.as_deref().unwrap_or("all").to_string();
    let active_sort = query.sort.as_deref().unwrap_or("title").to_string();
    let active_category = query
        .category
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok());

    let total_feed_count = feeds.len();

    if let Some(cat_id) = active_category {
        feeds.retain(|f| f.category_id == cat_id);
    }

    match active_filter.as_str() {
        "errors" => feeds.retain(|f| f.fetch_error.is_some()),
        "stale" => feeds.retain(|f| f.freshness_key == "stale"),
        _ => {}
    }

    match active_sort.as_str() {
        "unread" => feeds.sort_by(|a, b| b.unread_count.cmp(&a.unread_count)),
        "category" => feeds.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        _ => feeds.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }

    let normalized_filter = match active_filter.as_str() {
        "errors" | "stale" | "all" => active_filter,
        _ => "all".to_string(),
    };

    Ok(Json(FeedsResponse {
        feeds,
        categories,
        total_feed_count,
        active_filter: normalized_filter,
        active_sort,
        active_category,
    }))
}
