use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::config::DEFAULT_USER_AGENT;
use crate::error::AppError;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::{category, entry, feed};
use crate::models::user_settings;
use crate::AppState;

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub signup_enabled: bool,
    pub flash_messages: Vec<FlashMessage>,
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
        .user(|c| crate::models::user::count(c).ok())
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
        },
    )
}

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterTemplate {
    pub error: Option<String>,
    pub flash_messages: Vec<FlashMessage>,
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
        .user(|c| crate::models::user::count(c).ok())
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
        },
    )
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub username: String,
    pub role: String,
    pub sign_in_time: String,
    pub unread_count: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
}

impl IntoResponse for HomeTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn home_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, HomeTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        // When masquerading, check if original user is admin
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (unread_count, entries_per_page) = state
        .db
        .user(move |c| {
            let unread = entry::count_unread_by_user(c, user_id).unwrap_or(0);
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            (unread, epp)
        })
        .await
        .unwrap_or((0, user_settings::DEFAULT_ENTRIES_PER_PAGE));

    (
        flash.clone(),
        HomeTemplate {
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
        },
    )
}

#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub username: String,
    pub current_user_id: i64,
    pub original_user_id: i64,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
}

impl IntoResponse for AdminTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn admin_page(admin: PageAdminUser, flash: Flash) -> (Flash, AdminTemplate) {
    let is_masquerading = admin.session.is_masquerading();
    let original_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);

    (
        flash.clone(),
        AdminTemplate {
            username: admin.user.username,
            current_user_id: admin.user.id,
            original_user_id,
            is_masquerading,
            flash_messages: flash.messages,
        },
    )
}

#[derive(Template)]
#[template(path = "user-settings.html")]
pub struct UserSettingsTemplate {
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub entries_per_page: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: String,
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
    let (entries_per_page, linkding_configured, linkding_api_url, kagi_configured, kagi_language) =
        state
            .db
            .user(move |c| {
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

                (
                    epp,
                    linkding_configured,
                    api_url,
                    kagi_configured,
                    kagi_lang,
                )
            })
            .await
            .unwrap_or((
                user_settings::DEFAULT_ENTRIES_PER_PAGE,
                false,
                String::new(),
                false,
                String::new(),
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
            entries_per_page,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
        },
    )
}

#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
}

impl IntoResponse for CategoriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn categories_page(auth_user: PageAuthUser, flash: Flash) -> (Flash, CategoriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    (
        flash.clone(),
        CategoriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
        },
    )
}

#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
}

impl IntoResponse for FeedsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn feeds_page(auth_user: PageAuthUser, flash: Flash) -> (Flash, FeedsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    (
        flash.clone(),
        FeedsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
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
    let entries_per_page = state
        .db
        .user(move |c| {
            user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        })
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

    (
        flash.clone(),
        EntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
        },
    )
}

#[derive(Template)]
#[template(path = "entry.html")]
pub struct EntryTemplate {
    pub username: String,
    pub entry_id: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
}

impl IntoResponse for EntryTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn entry_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> (Flash, EntryTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (has_save_services, has_kagi_configured) = state
        .db
        .user(move |c| {
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);

            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();

            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);

            (save_services, kagi_configured)
        })
        .await
        .unwrap_or((false, false));

    (
        flash.clone(),
        EntryTemplate {
            username: auth_user.user.username,
            entry_id: id,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            has_save_services,
            has_kagi_configured,
        },
    )
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
    pub page_mode: String,
    pub page_title: String,
}

impl IntoResponse for ArchiveEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
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

    let user_id = auth_user.user.id;
    let entries_per_page = state
        .db
        .user(move |c| {
            user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        })
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            page_mode: "read".to_string(),
            page_title: "Read Entries".to_string(),
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

    let user_id = auth_user.user.id;
    let entries_per_page = state
        .db
        .user(move |c| {
            user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        })
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            page_mode: "starred".to_string(),
            page_title: "Starred Entries".to_string(),
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

    let user_id = auth_user.user.id;
    let entries_per_page = state
        .db
        .user(move |c| {
            user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        })
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
            page_mode: "summarized".to_string(),
            page_title: "Summarized Entries".to_string(),
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
    pub category_id: i64,
    pub category_name: String,
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
    let (entries_per_page, category_name) = state
        .db
        .user(move |c| {
            let cat = category::find_by_id_and_user(c, id, user_id)?
                .ok_or(AppError::CategoryNotFound)?;
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            Ok::<_, AppError>((epp, cat.name))
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
            category_id: id,
            category_name,
        },
    ))
}

// Search page
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
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

    let user_id = auth_user.user.id;
    let entries_per_page = state
        .db
        .user(move |c| {
            user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        })
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

    (
        flash.clone(),
        SearchTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page,
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
    pub feed_id: i64,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub category_id: i64,
    pub category_name: String,
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
    let (entries_per_page, feed_title, feed_has_icon, category_id, category_name) = state
        .db
        .user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let feed_title = f.title.unwrap_or_else(|| f.url.clone());
            let has_icon: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok::<_, AppError>((epp, feed_title, has_icon > 0, cat.id, cat.name))
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
            feed_id: id,
            feed_title,
            feed_has_icon,
            category_id,
            category_name,
        },
    ))
}
