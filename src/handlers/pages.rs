use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::config::DEFAULT_USER_AGENT;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::entry;
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
    let signup_enabled = {
        let conn = state.db.lock().ok();
        conn.and_then(|c| crate::models::user::count(&c).ok())
            .map(|count| state.config.can_register(count))
            .unwrap_or(false)
    };

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
    let can_register = {
        let conn = state.db.lock().ok();
        conn.and_then(|c| crate::models::user::count(&c).ok())
            .map(|count| state.config.can_register(count))
            .unwrap_or(false)
    };

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

    let (unread_count, entries_per_page) = {
        let conn = state.db.lock().ok();
        let unread = conn
            .as_ref()
            .and_then(|c| entry::count_unread_by_user(c, auth_user.user.id).ok())
            .unwrap_or(0);
        let epp = conn
            .as_ref()
            .and_then(|c| user_settings::get_entries_per_page(c, auth_user.user.id).ok())
            .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
        (unread, epp)
    };

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

    let (entries_per_page, linkding_configured, linkding_api_url, kagi_configured, kagi_language) = {
        let conn = state.db.lock().ok();
        let epp = conn
            .as_ref()
            .and_then(|c| user_settings::get_entries_per_page(c, auth_user.user.id).ok())
            .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

        let save_config = conn
            .as_ref()
            .and_then(|c| user_settings::get_save_services_config(c, auth_user.user.id).ok())
            .unwrap_or_default();

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
    };

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

    let entries_per_page = {
        let conn = state.db.lock().ok();
        conn.and_then(|c| user_settings::get_entries_per_page(&c, auth_user.user.id).ok())
            .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
    };

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

    let (has_save_services, has_kagi_configured) = {
        let conn = state.db.lock().ok();
        let save_services = conn
            .as_ref()
            .and_then(|c| user_settings::has_save_services(c, auth_user.user.id).ok())
            .unwrap_or(false);

        let save_config = conn
            .as_ref()
            .and_then(|c| user_settings::get_save_services_config(c, auth_user.user.id).ok())
            .unwrap_or_default();

        let kagi_configured = save_config
            .kagi
            .as_ref()
            .map(|c| c.is_configured())
            .unwrap_or(false);

        (save_services, kagi_configured)
    };

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
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            signup_enabled: state.config.signup_enabled,
            multi_user_enabled: state.config.multi_user_enabled,
        },
    )
}
