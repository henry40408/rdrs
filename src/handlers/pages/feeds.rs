use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};

use super::{
    AppLayoutContext, FRESH_MAX_DAYS, WARNING_MAX_DAYS, build_app_layout, compute_freshness,
    format_relative_time, render_not_found,
};
use crate::AppState;
use crate::db::Db;
use crate::error::AppError;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::entry_open::{self, FeedOpenRate};
use crate::models::user_settings;
use crate::models::{category, feed, image};

/// `/feeds` filter/sort query; the URL is the source of truth.
#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<String>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}

/// What the row builder looks feeds up in, fetched once per request.
struct RowLookups<'a> {
    category_names: HashMap<i64, String>,
    feeds_with_icon: HashSet<i64>,
    open_rates: HashMap<i64, FeedOpenRate>,
    unread_by_feed: &'a HashMap<i64, i64>,
}

/// "Last updated" text and `datetime` attribute. A feed that was fetched
/// recently but carries no date says so, instead of looking abandoned.
fn feed_updated_display(f: &feed::Feed) -> (String, String) {
    if f.feed_updated_at.is_some() {
        return format_relative_time(f.feed_updated_at);
    }
    let fetched_recently = f
        .fetched_at
        .is_some_and(|ft| (chrono::Utc::now() - ft).num_days() <= FRESH_MAX_DAYS);
    if fetched_recently {
        return ("No date info".to_string(), String::new());
    }
    ("Never".to_string(), String::new())
}

fn feed_row_view(f: feed::Feed, lookups: &RowLookups<'_>) -> FeedRowView {
    let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
    let (updated_rel, updated_dt) = feed_updated_display(&f);
    let (freshness_class, freshness_key) = compute_freshness(f.feed_updated_at, f.fetched_at);
    let open_rate = lookups.open_rates.get(&f.id);
    let open_rate_percent = open_rate.and_then(FeedOpenRate::percent);
    // Show raw counts: "40%" alone hides the sample size.
    let open_rate_label = open_rate_percent
        .zip(open_rate)
        .map(|(pct, r)| format!("{pct}% ({}/{})", r.opened, r.tracked));
    FeedRowView {
        title: f.title.clone().unwrap_or_else(|| f.url.clone()),
        category_name: lookups
            .category_names
            .get(&f.category_id)
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string()),
        has_icon: lookups.feeds_with_icon.contains(&f.id),
        unread_count: *lookups.unread_by_feed.get(&f.id).unwrap_or(&0),
        id: f.id,
        url: f.url,
        category_id: f.category_id,
        fetch_error: f.fetch_error,
        fetched_at_relative: fetched_rel,
        fetched_at_datetime: fetched_dt,
        feed_updated_at_relative: updated_rel,
        feed_updated_at_datetime: updated_dt,
        freshness_class,
        freshness_key,
        open_rate_label,
        open_rate_percent,
    }
}

struct FeedsListing {
    rows: Vec<FeedRowView>,
    categories: Vec<FeedCategoryOption>,
    total_feed_count: i64,
}

async fn fetch_feeds_listing(
    db: &Db,
    user_id: i64,
    unread_by_feed: &HashMap<i64, i64>,
) -> FeedsListing {
    let cats = category::list_by_user(db, user_id)
        .await
        .unwrap_or_default();
    let all_feeds = feed::list_by_user(db, user_id).await.unwrap_or_default();
    let open_rates = entry_open::open_rates_by_feed(db, user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.feed_id, r))
        .collect();

    let mut count_by_cat: HashMap<i64, i64> = HashMap::new();
    for f in &all_feeds {
        *count_by_cat.entry(f.category_id).or_insert(0) += 1;
    }
    let total_feed_count = all_feeds.len() as i64;

    let feed_ids: Vec<i64> = all_feeds.iter().map(|f| f.id).collect();
    let feeds_with_icon = image::existing_ids(db, image::ENTITY_FEED, &feed_ids)
        .await
        .unwrap_or_default();

    let lookups = RowLookups {
        category_names: cats.iter().map(|cat| (cat.id, cat.name.clone())).collect(),
        feeds_with_icon,
        open_rates,
        unread_by_feed,
    };
    let rows = all_feeds
        .into_iter()
        .map(|f| feed_row_view(f, &lookups))
        .collect();

    let categories = cats
        .into_iter()
        .map(|cat| FeedCategoryOption {
            feed_count: count_by_cat.get(&cat.id).copied().unwrap_or(0),
            id: cat.id,
            name: cat.name,
        })
        .collect();

    FeedsListing {
        rows,
        categories,
        total_feed_count,
    }
}

/// An empty or non-numeric `category` means "all categories".
fn parse_category_filter(raw: Option<&str>) -> Option<i64> {
    raw.filter(|s| !s.is_empty())?.parse().ok()
}

/// Filter first, then sort; `filter` and `sort` come from `normalize_*`.
fn filter_and_sort_rows(
    rows: &mut Vec<FeedRowView>,
    category: Option<i64>,
    filter: &str,
    sort: &str,
) {
    if let Some(cid) = category {
        rows.retain(|r| r.category_id == cid);
    }
    match filter {
        "errors" => rows.retain(|r| r.fetch_error.is_some()),
        "stale" => rows.retain(|r| r.freshness_key == "stale"),
        _ => {}
    }
    match sort {
        "unread" => rows.sort_by_key(|b| std::cmp::Reverse(b.unread_count)),
        "category" => rows.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        // Ascending (least-opened first); feeds without a rate sort last.
        "open_rate" => rows.sort_by_key(|a| (a.open_rate_percent.is_none(), a.open_rate_percent)),
        _ => rows.sort_by_key(|a| a.title.to_lowercase()),
    }
}

fn filter_links(
    active_category: Option<i64>,
    active_sort: &str,
    active_filter: &str,
) -> Vec<FeedFilterLink> {
    [("All", "all"), ("Errors", "errors"), ("Stale", "stale")]
        .into_iter()
        .map(|(label, filter)| FeedFilterLink {
            label,
            href: feeds_url(active_category, active_sort, filter),
            active: active_filter == filter,
        })
        .collect()
}

/// `GET /feeds`.
pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<FeedsQuery>,
) -> (Flash, FeedsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    // Gated on the setting, so an opted-in reader sees the column even when empty.
    let open_rate_shown = user_settings::get_pixel_tracking_enabled_at(&state.db, user_id)
        .await
        .unwrap_or(None)
        .is_some();

    let FeedsListing {
        mut rows,
        categories,
        total_feed_count,
    } = fetch_feeds_listing(&state.db, user_id, &layout.unread_by_feed).await;

    let active_filter = normalize_filter(query.filter.as_deref());
    let active_sort = normalize_sort(query.sort.as_deref());
    let active_category = parse_category_filter(query.category.as_deref());
    filter_and_sort_rows(&mut rows, active_category, active_filter, active_sort);

    // This exact view, for row forms and the edit link to come back to.
    let return_to = feeds_url(active_category, active_sort, active_filter);
    let edit_query = crate::handlers::return_to::return_to_query(&return_to);

    (
        flash,
        FeedsTemplate {
            title: "Feeds",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            feeds: rows,
            categories,
            total_feed_count,
            active_filter,
            active_sort,
            active_category_id: active_category,
            filter_links: filter_links(active_category, active_sort, active_filter),
            return_to,
            edit_query,
            fresh_max_days: FRESH_MAX_DAYS,
            warning_max_days: WARNING_MAX_DAYS,
            open_rate_shown,
            min_tracked_for_rate: entry_open::MIN_TRACKED_FOR_RATE,
        },
    )
}

/// Unknown values fall back to the default so a crafted `sort` never reaches
/// the links built from it.
fn normalize_sort(raw: Option<&str>) -> &'static str {
    match raw {
        Some("unread") => "unread",
        Some("category") => "category",
        Some("open_rate") => "open_rate",
        _ => "title",
    }
}

fn normalize_filter(raw: Option<&str>) -> &'static str {
    match raw {
        Some("errors") => "errors",
        Some("stale") => "stale",
        _ => "all",
    }
}

/// One query-string builder for every link to this view, so each value is encoded.
fn feeds_url(category: Option<i64>, sort: &str, filter: &str) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    if let Some(c) = category {
        q.append_pair("category", &c.to_string());
    }
    q.append_pair("sort", sort);
    q.append_pair("filter", filter);
    format!("/feeds?{}", q.finish())
}

/// `?return_to=` on pages reached from a filtered list.
#[derive(serde::Deserialize)]
pub struct ReturnToQuery {
    pub return_to: Option<String>,
}

/// `GET /feeds/{id}/edit`.
pub async fn feed_edit_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Path(id): Path<i64>,
    Query(query): Query<ReturnToQuery>,
) -> Result<Response, AppError> {
    let user_id = auth_user.user.id;
    let return_to = crate::handlers::return_to::feeds_list(query.return_to.as_deref());

    let lookup = async {
        let f = feed::find_by_id(&state.db, id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        category::find_by_id_and_user(&state.db, f.category_id, user_id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        let cats = category::list_by_user(&state.db, user_id).await?;
        let referrer_suggestions = referrer_suggestions(f.site_url.as_deref(), &f.url);
        Ok::<_, AppError>((
            FeedEditView {
                id: f.id,
                url: f.url,
                title: f.title.unwrap_or_default(),
                description: f.description.unwrap_or_default(),
                site_url: f.site_url.unwrap_or_default(),
                category_id: f.category_id,
                custom_user_agent: f.custom_user_agent.unwrap_or_default(),
                http2_disabled: f.http2_disabled,
                custom_referrer: f.custom_referrer.unwrap_or_default(),
                referrer_suggestions,
            },
            cats.into_iter()
                .map(|c| FeedCategoryOption {
                    id: c.id,
                    name: c.name,
                    feed_count: 0,
                })
                .collect::<Vec<_>>(),
        ))
    }
    .await;

    let (feed_view, cats) = match lookup {
        Ok(v) => v,
        Err(AppError::FeedNotFound) => {
            let page = render_not_found(
                &state,
                &auth_user,
                &flash,
                "Feed not found",
                "This feed doesn't exist or you don't have access to it.",
            )
            .await;
            return Ok((flash, page).into_response());
        }
        Err(e) => return Err(e),
    };

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    Ok((
        flash,
        FeedEditTemplate {
            title: "Edit Feed",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            feed: feed_view,
            categories: cats,
            return_to,
            user_agent_suggestions: crate::config::CUSTOM_USER_AGENT_SUGGESTIONS,
        },
    )
        .into_response())
}

/// `GET /feeds/import`.
pub async fn feeds_import_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, FeedsImportTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    (
        flash,
        FeedsImportTemplate {
            title: "Import OPML",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
        },
    )
}

/// One row of the SSR `/feeds` table.
pub struct FeedRowView {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub category_id: i64,
    pub category_name: String,
    pub has_icon: bool,
    pub fetch_error: Option<String>,
    pub unread_count: i64,
    pub fetched_at_relative: String,
    pub fetched_at_datetime: String,
    pub feed_updated_at_relative: String,
    pub feed_updated_at_datetime: String,
    pub freshness_class: String,
    pub freshness_key: String,
    /// `"42% (10/24)"`; `None` below `entry_open::MIN_TRACKED_FOR_RATE`.
    pub open_rate_label: Option<String>,
    /// Sort key; `None` sorts last.
    pub open_rate_percent: Option<i64>,
}

/// Category option for selects + sidebar counts on `/feeds`.
pub struct FeedCategoryOption {
    pub id: i64,
    pub name: String,
    pub feed_count: i64,
}

/// Filter pill (All / Errors / Stale) on the `/feeds` filter bar.
pub struct FeedFilterLink {
    pub label: &'static str,
    pub href: String,
    pub active: bool,
}

/// Per-route template for `/feeds`.
#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub feeds: Vec<FeedRowView>,
    pub categories: Vec<FeedCategoryOption>,
    pub total_feed_count: i64,
    pub active_filter: &'static str,
    pub active_sort: &'static str,
    pub active_category_id: Option<i64>,
    pub filter_links: Vec<FeedFilterLink>,
    /// This filtered view, posted back by row forms as `return_to`.
    pub return_to: String,
    /// `return_to` as a query string for the edit links.
    pub edit_query: String,
    /// Thresholds `compute_freshness` applies, shown in the help text.
    pub fresh_max_days: i64,
    pub warning_max_days: i64,
    /// Show the "Open rate" column only when tracking is enabled.
    pub open_rate_shown: bool,
    pub min_tracked_for_rate: i64,
}

/// Editable view of a single feed for `/feeds/{id}/edit`.
pub struct FeedEditView {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub site_url: String,
    pub category_id: i64,
    pub custom_user_agent: String,
    pub http2_disabled: bool,
    pub custom_referrer: String,
    /// Referrer `<datalist>` options; see `referrer_suggestions`.
    pub referrer_suggestions: Vec<String>,
}

/// Deduplicated HTTP(S) origins to offer as `Referer`: the site first (it
/// serves the embedded images), then the feed URL's origin.
fn referrer_suggestions(site_url: Option<&str>, feed_url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for candidate in [site_url, Some(feed_url)].into_iter().flatten() {
        let Ok(parsed) = url::Url::parse(candidate.trim()) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let origin = match parsed.port() {
            Some(port) => format!("{}://{host}:{port}/", parsed.scheme()),
            None => format!("{}://{host}/", parsed.scheme()),
        };
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out
}

/// Per-route template for `/feeds/{id}/edit`.
#[derive(Template)]
#[template(path = "feed_edit.html")]
pub struct FeedEditTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub feed: FeedEditView,
    pub categories: Vec<FeedCategoryOption>,
    /// The list Save and Cancel return to; see [`crate::handlers::return_to`].
    pub return_to: String,
    /// User-agent `<datalist>` options.
    pub user_agent_suggestions: &'static [&'static str],
}

/// Per-route template for `/feeds/import`.
#[derive(Template)]
#[template(path = "feeds_import.html")]
pub struct FeedsImportTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::{feeds_url, normalize_filter, normalize_sort, referrer_suggestions};

    #[test]
    fn feeds_url_keeps_parameter_order_and_omits_missing_category() {
        assert_eq!(
            feeds_url(None, "title", "all"),
            "/feeds?sort=title&filter=all"
        );
        assert_eq!(
            feeds_url(Some(5), "unread", "errors"),
            "/feeds?category=5&sort=unread&filter=errors"
        );
    }

    #[test]
    fn normalize_sort_rejects_injected_parameters() {
        assert_eq!(normalize_sort(Some("title&category=5")), "title");
        assert_eq!(normalize_sort(Some("")), "title");
        assert_eq!(normalize_sort(None), "title");
        assert_eq!(normalize_sort(Some("open_rate")), "open_rate");
    }

    #[test]
    fn normalize_filter_falls_back_to_all() {
        assert_eq!(normalize_filter(Some("stale")), "stale");
        assert_eq!(normalize_filter(Some("nope")), "all");
    }

    #[test]
    fn referrer_suggestion_leads_with_the_site_the_feed_describes() {
        assert_eq!(
            referrer_suggestions(
                Some("https://example.com/blog"),
                "https://cdn.example.net/rss"
            ),
            vec![
                "https://example.com/".to_string(),
                "https://cdn.example.net/".to_string()
            ]
        );
    }

    #[test]
    fn referrer_suggestions_are_deduplicated() {
        assert_eq!(
            referrer_suggestions(Some("https://example.com/"), "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestion_falls_back_to_the_feed_url() {
        assert_eq!(
            referrer_suggestions(None, "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
        // A blank site_url is what the DB holds for "unset", not a URL.
        assert_eq!(
            referrer_suggestions(Some(""), "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestion_keeps_a_non_default_port() {
        assert_eq!(
            referrer_suggestions(None, "http://localhost:8080/feed.xml"),
            vec!["http://localhost:8080/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestions_skip_what_cannot_be_a_referer() {
        assert_eq!(
            referrer_suggestions(None, "not a url"),
            [] as [std::string::String; 0]
        );
        assert_eq!(
            referrer_suggestions(Some("file:///etc/hosts"), "not a url"),
            [] as [std::string::String; 0]
        );
    }
}
