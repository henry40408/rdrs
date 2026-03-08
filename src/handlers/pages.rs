use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::config::DEFAULT_USER_AGENT;
use crate::error::AppError;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::entry_summary;
use crate::models::user_settings;
use crate::models::{category, entry, feed};
use crate::AppState;

/// Entry data for SSR embedding as JSON.
/// Field names match what the JS `_transformItem()` expects.
#[derive(serde::Serialize)]
struct SsrEntry {
    id: i64,
    feed_id: i64,
    category_id: i64,
    category_name: String,
    feed_title: String,
    feed_url: String,
    feed_has_icon: bool,
    title: String,
    link: Option<String>,
    content: String,
    author: String,
    published_at: Option<String>,
    read_at: Option<String>,
    starred_at: Option<String>,
    summary_status: Option<String>,
}

/// SSR data for the entry list component, embedded as JSON.
#[derive(serde::Serialize)]
struct SsrEntryListData {
    entries: Vec<SsrEntry>,
    continuation: Option<String>,
}

/// Convert EntryWithFeed + summary statuses to SSR entries.
fn entries_to_ssr(
    entries: Vec<entry::EntryWithFeed>,
    summary_statuses: &std::collections::HashMap<i64, entry_summary::SummaryStatus>,
) -> Vec<SsrEntry> {
    entries
        .into_iter()
        .map(|e| {
            let status = summary_statuses.get(&e.entry.id).map(|s| s.as_str().to_string());
            SsrEntry {
                id: e.entry.id,
                feed_id: e.entry.feed_id,
                category_id: e.category_id,
                category_name: e.category_name,
                feed_title: e.feed_title.unwrap_or_default(),
                feed_url: e.feed_url,
                feed_has_icon: e.feed_has_icon,
                title: e.entry.title.unwrap_or_default(),
                link: e.entry.link,
                content: String::new(), // Don't include content in list view
                author: e.entry.author.unwrap_or_default(),
                published_at: e.entry.published_at.map(|dt| dt.to_rfc3339()),
                read_at: e.entry.read_at.map(|dt| dt.to_rfc3339()),
                starred_at: e.entry.starred_at.map(|dt| dt.to_rfc3339()),
                summary_status: status,
            }
        })
        .collect()
}

/// Fetch first page of entries for SSR, returns (ssr_json, has_more).
fn fetch_entries_for_ssr(
    conn: &rusqlite::Connection,
    user_id: i64,
    filter: &entry::EntryFilter,
    limit: i64,
) -> (String, Option<String>) {
    let pagination = entry::ContinuationParams {
        oldest_first: false,
        limit: limit + 1, // fetch one extra to check for continuation
        continuation_id: None,
        ot: None,
        nt: None,
    };

    let mut entries =
        entry::list_by_user_with_continuation(conn, user_id, filter, &pagination)
            .unwrap_or_default();

    let continuation = if entries.len() as i64 > limit {
        let last = entries.pop().unwrap();
        Some(last.entry.id.to_string())
    } else {
        None
    };

    // Fetch summary statuses
    let entry_ids: Vec<i64> = entries.iter().map(|e| e.entry.id).collect();
    let summary_statuses =
        entry_summary::get_statuses_for_entries(conn, user_id, &entry_ids).unwrap_or_default();

    let ssr_entries = entries_to_ssr(entries, &summary_statuses);
    let data = SsrEntryListData {
        entries: ssr_entries,
        continuation,
    };

    let json = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
    (json, data.continuation)
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub signup_enabled: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
}

impl IntoResponse for LoginTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn login_page(State(state): State<AppState>, flash: Flash) -> (Flash, LoginTemplate) {
    let signup_enabled = state
        .db
        .read_user(|c| crate::models::user::count(c).ok())
        .await
        .ok()
        .flatten()
        .map(|count| state.config.can_register(count))
        .unwrap_or(false);

    (
        flash.clone(),
        LoginTemplate {
            signup_enabled,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
        },
    )
}

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterTemplate {
    pub error: Option<String>,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
}

impl IntoResponse for RegisterTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn register_page(
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, RegisterTemplate) {
    let can_register = state
        .db
        .read_user(|c| crate::models::user::count(c).ok())
        .await
        .ok()
        .flatten()
        .map(|count| state.config.can_register(count))
        .unwrap_or(false);

    (
        flash.clone(),
        RegisterTemplate {
            error: if !can_register {
                Some("Registration is currently disabled".to_string())
            } else {
                None
            },
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
        },
    )
}

#[derive(Template)]
#[template(path = "unread.html")]
pub struct UnreadTemplate {
    pub username: String,
    pub role: String,
    pub sign_in_time: String,
    pub unread_count: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub ssr_entries_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for UnreadTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UnreadTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        // When masquerading, check if original user is admin
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (unread_count, entries_per_page, has_save_services, has_kagi_configured, ssr_entries_json, theme) = state
        .db
        .read_user(move |c| {
            let unread = entry::count_unread_by_user(c, user_id).unwrap_or(0);
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            // SSR: fetch first page of unread entries
            let filter = entry::EntryFilter {
                unread_only: true,
                ..Default::default()
            };
            let (ssr_json, _) = fetch_entries_for_ssr(c, user_id, &filter, epp);

            (unread, epp, save_services, kagi_configured, ssr_json, theme)
        })
        .await
        .unwrap_or((
            0,
            user_settings::DEFAULT_ENTRIES_PER_PAGE,
            false,
            false,
            "null".to_string(),
            None,
        ));

    (
        flash.clone(),
        UnreadTemplate {
            username: auth_user.user.username.clone(),
            role: auth_user.user.role.as_str().to_string(),
            sign_in_time: auth_user
                .session
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            unread_count,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

/// User info for SSR admin display.
pub struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub is_disabled: bool,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub username: String,
    pub current_user_id: i64,
    pub original_user_id: i64,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub users: Vec<AdminUserRow>,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for AdminTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AdminTemplate) {
    let is_masquerading = admin.session.is_masquerading();
    let original_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);

    let user_id = admin.user.id;
    let (users, theme) = state
        .db
        .read_user(move |c| {
            let user_list = crate::models::user::list_all(c).unwrap_or_default();
            let rows: Vec<AdminUserRow> = user_list
                .into_iter()
                .map(|u| {
                    let is_disabled = u.is_disabled();
                    let role = u.role.as_str().to_string();
                    let created_at = u.created_at.format("%Y-%m-%d").to_string();
                    AdminUserRow {
                        id: u.id,
                        username: u.username,
                        role,
                        is_disabled,
                        created_at,
                    }
                })
                .collect();
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            (rows, theme)
        })
        .await
        .unwrap_or((vec![], None));

    (
        flash.clone(),
        AdminTemplate {
            username: admin.user.username,
            current_user_id: admin.user.id,
            original_user_id,
            is_masquerading,
            flash_messages: flash.messages,
            users,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

#[derive(Template)]
#[template(path = "user-settings.html")]
pub struct UserSettingsTemplate {
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub logged_in_at: String,
    pub entries_per_page: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for UserSettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (
        entries_per_page,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
        theme,
    ) = state
        .db
        .read_user(move |c| {
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();

            let linkding = save_config.linkding.as_ref();
            let linkding_configured = linkding.map(|c| c.is_configured()).unwrap_or(false);
            let api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

            let kagi = save_config.kagi.as_ref();
            let kagi_configured = kagi.map(|c| c.is_configured()).unwrap_or(false);
            let kagi_lang = kagi.and_then(|c| c.language.clone()).unwrap_or_default();

            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            (
                epp,
                linkding_configured,
                api_url,
                kagi_configured,
                kagi_lang,
                theme,
            )
        })
        .await
        .unwrap_or((
            user_settings::DEFAULT_ENTRIES_PER_PAGE,
            false,
            String::new(),
            false,
            String::new(),
            None,
        ));

    (
        flash.clone(),
        UserSettingsTemplate {
            username: auth_user.user.username,
            role: auth_user.user.role.as_str().to_string(),
            created_at: auth_user
                .user
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            logged_in_at: auth_user
                .session
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            entries_per_page,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

/// A category with its feed count for SSR display.
pub struct CategoryWithCount {
    pub id: i64,
    pub name: String,
    pub feed_count: usize,
}

#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub categories: Vec<CategoryWithCount>,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for CategoriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (categories_with_counts, theme) = state
        .db
        .read_user(move |c| {
            let cats = category::list_by_user(c, user_id).unwrap_or_default();
            let cats_with_counts: Vec<CategoryWithCount> = cats
                .into_iter()
                .map(|cat| {
                    let feed_count =
                        feed::list_by_category(c, cat.id).map(|f| f.len()).unwrap_or(0);
                    CategoryWithCount {
                        id: cat.id,
                        name: cat.name,
                        feed_count,
                    }
                })
                .collect();
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            (cats_with_counts, theme)
        })
        .await
        .unwrap_or((vec![], None));

    (
        flash.clone(),
        CategoriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            categories: categories_with_counts,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

/// A feed row for SSR display.
#[derive(serde::Serialize)]
pub struct FeedRow {
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
}

/// A category option for SSR dropdowns.
pub struct CategoryOption {
    pub id: i64,
    pub name: String,
    pub feed_count: usize,
}

#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub feeds: Vec<FeedRow>,
    pub categories: Vec<CategoryOption>,
    pub feed_data_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for FeedsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, FeedsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (feeds_data, cats_data, theme) = state
        .db
        .read_user(move |c| {
            let cats = category::list_by_user(c, user_id).unwrap_or_default();
            let all_feeds = feed::list_by_user(c, user_id).unwrap_or_default();
            let unread_map = entry::count_unread_by_feed(c, user_id).unwrap_or_default();

            // Build category name map
            let cat_map: std::collections::HashMap<i64, String> = cats
                .iter()
                .map(|cat| (cat.id, cat.name.clone()))
                .collect();

            // Count feeds per category
            let mut feed_count_by_cat: std::collections::HashMap<i64, usize> =
                std::collections::HashMap::new();
            for f in &all_feeds {
                *feed_count_by_cat.entry(f.category_id).or_insert(0) += 1;
            }

            let feed_rows: Vec<FeedRow> = all_feeds
                .into_iter()
                .map(|f| {
                    let has_icon: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                            [f.id],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    FeedRow {
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
                    }
                })
                .collect();

            let cat_options: Vec<CategoryOption> = cats
                .into_iter()
                .map(|cat| {
                    let fc = feed_count_by_cat.get(&cat.id).copied().unwrap_or(0);
                    CategoryOption {
                        id: cat.id,
                        name: cat.name,
                        feed_count: fc,
                    }
                })
                .collect();

            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            (feed_rows, cat_options, theme)
        })
        .await
        .unwrap_or((vec![], vec![], None));

    // Build a JSON map: { feedId: { url, title, description, ... }, ... }
    let feed_data_map: std::collections::HashMap<i64, &FeedRow> =
        feeds_data.iter().map(|f| (f.id, f)).collect();
    let feed_data_json = serde_json::to_string(&feed_data_map).unwrap_or_else(|_| "{}".to_string());

    (
        flash.clone(),
        FeedsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            feeds: feeds_data,
            categories: cats_data,
            feed_data_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

#[derive(Template)]
#[template(path = "entries.html")]
pub struct EntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub ssr_entries_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for EntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, EntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (entries_per_page, has_save_services, has_kagi_configured, ssr_entries_json, theme) = state
        .db
        .read_user(move |c| {
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            // SSR: fetch first page of all entries
            let filter = entry::EntryFilter::default();
            let (ssr_json, _) = fetch_entries_for_ssr(c, user_id, &filter, epp);

            (epp, save_services, kagi_configured, ssr_json, theme)
        })
        .await
        .unwrap_or((user_settings::DEFAULT_ENTRIES_PER_PAGE, false, false, "null".to_string(), None));

    (
        flash.clone(),
        EntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

/// Query parameters for the entry page redirect.
#[derive(serde::Deserialize, Default)]
pub struct EntryPageQuery {
    pub origin: Option<String>,
    pub feed: Option<i64>,
    pub category: Option<i64>,
    pub read_only: Option<String>,
    pub starred_only: Option<String>,
    pub has_summary: Option<String>,
}

/// Redirect `/entries/{id}` to the appropriate list page with `?entry={id}`.
pub async fn entry_page(
    _auth_user: PageAuthUser,
    Path(id): Path<i64>,
    Query(query): Query<EntryPageQuery>,
) -> Redirect {
    let origin = query.origin.as_deref().unwrap_or("");

    let base_url = match origin {
        "feed" => {
            if let Some(feed_id) = query.feed {
                format!("/feeds/{}/entries", feed_id)
            } else {
                "/entries".to_string()
            }
        }
        "category" => {
            if let Some(cat_id) = query.category {
                format!("/categories/{}/entries", cat_id)
            } else {
                "/entries".to_string()
            }
        }
        "read" => "/entries/read".to_string(),
        "starred" => "/entries/starred".to_string(),
        "summarized" => "/entries/summarized".to_string(),
        "entries" => "/entries".to_string(),
        "search" => "/search".to_string(),
        _ => "/".to_string(),
    };

    let redirect_url = format!("{}?entry={}", base_url, id);
    Redirect::to(&redirect_url)
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
    pub theme: Option<String>,
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_agent_is_default = state.config.user_agent == DEFAULT_USER_AGENT;

    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    (
        flash.clone(),
        SettingsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            signup_enabled: state.config.signup_enabled,
            multi_user_enabled: state.config.multi_user_enabled,
            theme,
        },
    )
}

// Archive entries pages (read/starred/summarized)
#[derive(Template)]
#[template(path = "entries_archive.html")]
pub struct ArchiveEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub page_mode: String,
    pub page_title: String,
    pub ssr_entries_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for ArchiveEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Helper to fetch common entry-list config + SSR entries from DB.
async fn fetch_entry_list_config(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
) -> (i64, bool, bool, String, Option<String>) {
    state
        .db
        .read_user(move |c| {
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (ssr_json, _) = fetch_entries_for_ssr(c, user_id, &filter, epp);
            (epp, save_services, kagi_configured, ssr_json, theme)
        })
        .await
        .unwrap_or((user_settings::DEFAULT_ENTRIES_PER_PAGE, false, false, "null".to_string(), None))
}

pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        read_only: true,
        ..Default::default()
    };
    let (entries_per_page, has_save_services, has_kagi_configured, ssr_entries_json, theme) =
        fetch_entry_list_config(&state, auth_user.user.id, filter).await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            page_mode: "read".to_string(),
            page_title: "Read Entries".to_string(),
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        starred_only: true,
        ..Default::default()
    };
    let (entries_per_page, has_save_services, has_kagi_configured, ssr_entries_json, theme) =
        fetch_entry_list_config(&state, auth_user.user.id, filter).await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            page_mode: "starred".to_string(),
            page_title: "Starred Entries".to_string(),
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        has_summary: Some(true),
        ..Default::default()
    };
    let (entries_per_page, has_save_services, has_kagi_configured, ssr_entries_json, theme) =
        fetch_entry_list_config(&state, auth_user.user.id, filter).await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            page_mode: "summarized".to_string(),
            page_title: "Summarized Entries".to_string(),
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

// Category entries page
#[derive(Template)]
#[template(path = "category_entries.html")]
pub struct CategoryEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub category_id: i64,
    pub category_name: String,
    pub ssr_entries_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for CategoryEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, CategoryEntriesTemplate), AppError> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (entries_per_page, has_save_services, has_kagi_configured, category_name, ssr_entries_json, theme) = state
        .db
        .read_user(move |c| {
            let cat =
                category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            // SSR: fetch first page of category entries (default: unread)
            let filter = entry::EntryFilter {
                category_id: Some(id),
                unread_only: true,
                ..Default::default()
            };
            let (ssr_json, _) = fetch_entries_for_ssr(c, user_id, &filter, epp);

            Ok::<_, AppError>((epp, save_services, kagi_configured, cat.name, ssr_json, theme))
        })
        .await??;

    Ok((
        flash.clone(),
        CategoryEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            category_id: id,
            category_name,
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    ))
}

// Search page (no SSR entries - loads on user input)
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for SearchTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SearchTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    // Search doesn't need SSR entries - pass a dummy filter that won't fetch
    let filter = entry::EntryFilter::default();
    let (entries_per_page, has_save_services, has_kagi_configured, _, theme) =
        fetch_entry_list_config(&state, auth_user.user.id, filter).await;

    (
        flash.clone(),
        SearchTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            theme,
            git_version: crate::GIT_VERSION,
        },
    )
}

// Feed entries page
#[derive(Template)]
#[template(path = "feed_entries.html")]
pub struct FeedEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub feed_id: i64,
    pub feed_url: String,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub category_id: i64,
    pub category_name: String,
    pub ssr_entries_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
}

impl IntoResponse for FeedEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, FeedEntriesTemplate), AppError> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (
        entries_per_page,
        has_save_services,
        has_kagi_configured,
        feed_url,
        feed_title,
        feed_has_icon,
        category_id,
        category_name,
        ssr_entries_json,
        theme,
    ) = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let feed_url = f.url.clone();
            let feed_title = f.title.unwrap_or_else(|| f.url.clone());
            let has_icon: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            // SSR: fetch first page of feed entries (default: unread)
            let filter = entry::EntryFilter {
                feed_id: Some(id),
                unread_only: true,
                ..Default::default()
            };
            let (ssr_json, _) = fetch_entries_for_ssr(c, user_id, &filter, epp);

            Ok::<_, AppError>((
                epp,
                save_services,
                kagi_configured,
                feed_url,
                feed_title,
                has_icon > 0,
                cat.id,
                cat.name,
                ssr_json,
                theme,
            ))
        })
        .await??;

    Ok((
        flash.clone(),
        FeedEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            has_save_services,
            has_kagi_configured,
            feed_id: id,
            feed_url,
            feed_title,
            feed_has_icon,
            category_id,
            category_name,
            ssr_entries_json,
            theme,
            git_version: crate::GIT_VERSION,
        },
    ))
}
