use std::sync::Arc;

use axum::{
    routing::{delete, get, post, put},
    Router,
};
use tokio::sync::mpsc;
use tower_http::{compression::CompressionLayer, timeout::TimeoutLayer};
use webauthn_rs::prelude::Webauthn;

use services::http::SERVER_REQUEST_TIMEOUT;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod services;
pub mod utils;
pub mod version;

pub use config::Config;
pub use db::DbPool;
pub use middleware::auth::SESSION_COOKIE_NAME;
pub use models::{Role, User};
pub use version::GIT_VERSION;

use services::{SummaryCache, SummaryJob};

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub config: Arc<Config>,
    pub webauthn: Arc<Webauthn>,
    pub summary_cache: Arc<SummaryCache>,
    pub summary_tx: mpsc::Sender<SummaryJob>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Health check
        .route("/health", get(handlers::health::health_check))
        // Favicon routes
        .route("/favicon.ico", get(handlers::favicon::favicon_ico))
        .route("/favicon.svg", get(handlers::favicon::favicon_svg))
        .route("/favicon-16x16.png", get(handlers::favicon::favicon_16))
        .route("/favicon-32x32.png", get(handlers::favicon::favicon_32))
        .route(
            "/apple-touch-icon.png",
            get(handlers::favicon::apple_touch_icon),
        )
        .route("/", get(handlers::pages::unread_page))
        .route("/login", get(handlers::pages::login_page))
        .route("/register", get(handlers::pages::register_page))
        .route("/user-settings", get(handlers::pages::user_settings_page))
        .route("/admin", get(handlers::pages::admin_page))
        .route("/settings", get(handlers::pages::settings_page))
        .route("/api/register", post(handlers::auth::register))
        .route("/api/session", post(handlers::auth::login))
        .route("/api/session", delete(handlers::auth::logout))
        .route("/api/user", get(handlers::user::get_current_user))
        .route("/api/me", get(handlers::user::get_me))
        .route("/api/sidebar", get(handlers::user::get_sidebar))
        .route("/api/statistics", get(handlers::statistics::get_statistics))
        .route("/api/feeds", get(handlers::feeds::list_feeds))
        .route("/api/user/password", put(handlers::user::change_password))
        .route("/api/user/settings", put(handlers::user::update_settings))
        .route(
            "/api/user/settings/linkding",
            get(handlers::user::get_linkding_settings),
        )
        .route(
            "/api/user/settings/linkding",
            put(handlers::user::update_linkding_settings),
        )
        .route(
            "/api/user/settings/kagi",
            get(handlers::user::get_kagi_settings),
        )
        .route(
            "/api/user/settings/kagi",
            put(handlers::user::update_kagi_settings),
        )
        .route("/api/user/settings/theme", get(handlers::user::get_theme))
        .route(
            "/api/user/settings/theme",
            put(handlers::user::update_theme),
        )
        .route("/api/admin/users", get(handlers::admin::list_users))
        .route("/api/admin/users/{id}", put(handlers::admin::update_user))
        .route(
            "/api/admin/users/{id}",
            delete(handlers::admin::delete_user),
        )
        .route(
            "/api/admin/masquerade/{id}",
            post(handlers::admin::start_masquerade),
        )
        .route(
            "/api/admin/unmasquerade",
            post(handlers::admin::stop_masquerade),
        )
        // Page routes
        .route("/categories", get(handlers::pages::categories_page))
        .route("/feeds", get(handlers::pages::feeds_page))
        .route("/entries", get(handlers::pages::entries_page))
        .route("/entries/read", get(handlers::pages::read_entries_page))
        .route(
            "/entries/starred",
            get(handlers::pages::starred_entries_page),
        )
        .route(
            "/entries/summarized",
            get(handlers::pages::summarized_entries_page),
        )
        .route("/entries/{id}", get(handlers::pages::entry_page))
        .route("/search", get(handlers::pages::search_page))
        .route("/statistics", get(handlers::pages::statistics_page))
        .route(
            "/categories/{id}/entries",
            get(handlers::pages::category_entries_page),
        )
        .route(
            "/feeds/{id}/entries",
            get(handlers::pages::feed_entries_page),
        )
        // RDRS-specific feed endpoints (not replaced by GReader API)
        .route(
            "/api/feeds/fetch-metadata",
            post(handlers::feed::fetch_metadata),
        )
        .route("/api/feeds/{id}/icon", get(handlers::feed::get_feed_icon))
        .route(
            "/api/feeds/{id}/refresh",
            post(handlers::entry::refresh_feed_handler),
        )
        // RDRS-specific entry endpoints (not replaced by GReader API)
        .route(
            "/api/entries/{id}/fetch-full-content",
            post(handlers::entry::fetch_full_content),
        )
        .route(
            "/api/entries/{id}/save",
            post(handlers::entry::save_to_services),
        )
        .route(
            "/api/entries/{id}/summarize",
            post(handlers::entry::summarize_entry),
        )
        .route(
            "/api/entries/{id}/summary",
            get(handlers::entry::get_entry_summary),
        )
        .route(
            "/api/entries/{id}/summary",
            delete(handlers::entry::delete_entry_summary),
        )
        .route(
            "/api/entries/{id}/neighbors",
            get(handlers::entry::get_entry_neighbors),
        )
        // Proxy routes
        .route("/api/proxy/image", get(handlers::proxy::proxy_image))
        // Passkey routes
        .route(
            "/api/passkey/register/start",
            post(handlers::passkey::start_registration),
        )
        .route(
            "/api/passkey/register/finish",
            post(handlers::passkey::finish_registration),
        )
        .route(
            "/api/passkey/auth/start",
            post(handlers::passkey::start_authentication),
        )
        .route(
            "/api/passkey/auth/finish",
            post(handlers::passkey::finish_authentication),
        )
        .route("/api/passkeys", get(handlers::passkey::list_passkeys))
        .route("/api/passkeys/{id}", put(handlers::passkey::rename_passkey))
        .route(
            "/api/passkeys/{id}",
            delete(handlers::passkey::delete_passkey),
        )
        // Google Reader API (standard paths + FreshRSS-compatible /api/greader.php prefix)
        .merge(handlers::greader::greader_routes())
        .nest("/api/greader.php", handlers::greader::greader_routes())
        .route("/static/{*path}", get(handlers::static_assets::serve))
        .with_state(state)
        .layer(middleware::DateHeaderLayer::new())
        .layer(CompressionLayer::new().gzip(true))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            SERVER_REQUEST_TIMEOUT,
        ))
}
