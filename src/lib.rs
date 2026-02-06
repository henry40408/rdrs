use std::sync::Arc;

use axum::{
    routing::{delete, get, post, put},
    Router,
};
use tokio::sync::mpsc;
use webauthn_rs::prelude::Webauthn;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod services;
pub mod version;

pub use config::Config;
pub use db::DbPool;
pub use middleware::auth::SESSION_COOKIE_NAME;
pub use models::{Role, User};
pub use version::{GIT_VERSION, PKG_VERSION};

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
        .route("/", get(handlers::pages::home_page))
        .route("/login", get(handlers::pages::login_page))
        .route("/register", get(handlers::pages::register_page))
        .route("/user-settings", get(handlers::pages::user_settings_page))
        .route("/admin", get(handlers::pages::admin_page))
        .route("/settings", get(handlers::pages::settings_page))
        .route("/api/register", post(handlers::auth::register))
        .route("/api/session", post(handlers::auth::login))
        .route("/api/session", delete(handlers::auth::logout))
        .route("/api/user", get(handlers::user::get_current_user))
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
        // Category routes
        .route("/categories", get(handlers::pages::categories_page))
        .route("/api/categories", get(handlers::category::list_categories))
        .route("/api/categories", post(handlers::category::create_category))
        .route(
            "/api/categories/{id}",
            get(handlers::category::get_category),
        )
        .route(
            "/api/categories/{id}",
            put(handlers::category::update_category),
        )
        .route(
            "/api/categories/{id}",
            delete(handlers::category::delete_category),
        )
        // Feed routes
        .route("/feeds", get(handlers::pages::feeds_page))
        .route("/api/feeds", get(handlers::feed::list_feeds))
        .route("/api/feeds", post(handlers::feed::create_feed))
        .route(
            "/api/feeds/fetch-metadata",
            post(handlers::feed::fetch_metadata),
        )
        .route("/api/feeds/{id}", get(handlers::feed::get_feed))
        .route("/api/feeds/{id}", put(handlers::feed::update_feed))
        .route("/api/feeds/{id}", delete(handlers::feed::delete_feed))
        .route("/api/feeds/{id}/icon", get(handlers::feed::get_feed_icon))
        // OPML routes
        .route("/api/opml/export", get(handlers::feed::export_opml))
        .route("/api/opml/import", post(handlers::feed::import_opml))
        // Entry routes
        .route("/entries", get(handlers::pages::entries_page))
        .route("/entries/read", get(handlers::pages::read_entries_page))
        .route("/entries/starred", get(handlers::pages::starred_entries_page))
        .route("/entries/summarized", get(handlers::pages::summarized_entries_page))
        .route("/entries/{id}", get(handlers::pages::entry_page))
        .route("/search", get(handlers::pages::search_page))
        // Category entries page
        .route("/categories/{id}/entries", get(handlers::pages::category_entries_page))
        // Feed entries page
        .route("/feeds/{id}/entries", get(handlers::pages::feed_entries_page))
        .route("/api/entries", get(handlers::entry::list_entries))
        .route("/api/entries/{id}", get(handlers::entry::get_entry))
        .route(
            "/api/entries/{id}/read",
            put(handlers::entry::mark_entry_read),
        )
        .route(
            "/api/entries/{id}/unread",
            put(handlers::entry::mark_entry_unread),
        )
        .route(
            "/api/entries/{id}/star",
            put(handlers::entry::toggle_entry_star),
        )
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
        .route(
            "/api/entries/mark-all-read",
            put(handlers::entry::mark_all_read),
        )
        .route(
            "/api/entries/mark-read-by-ids",
            put(handlers::entry::mark_read_by_ids),
        )
        .route(
            "/api/entries/unread-stats",
            get(handlers::entry::get_unread_stats),
        )
        .route(
            "/api/feeds/{id}/entries",
            get(handlers::entry::list_feed_entries),
        )
        .route(
            "/api/feeds/{id}/refresh",
            post(handlers::entry::refresh_feed_handler),
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
        .with_state(state)
}
