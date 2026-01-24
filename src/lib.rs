use std::sync::{Arc, Mutex};

use axum::{
    routing::{delete, get, post, put},
    Router,
};
use rusqlite::Connection;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod services;

pub use config::Config;
pub use middleware::auth::SESSION_COOKIE_NAME;
pub use models::{Role, User};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub config: Arc<Config>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::pages::home_page))
        .route("/login", get(handlers::pages::login_page))
        .route("/register", get(handlers::pages::register_page))
        .route(
            "/change-password",
            get(handlers::pages::change_password_page),
        )
        .route("/admin", get(handlers::pages::admin_page))
        .route("/settings", get(handlers::pages::settings_page))
        .route("/api/register", post(handlers::auth::register))
        .route("/api/session", post(handlers::auth::login))
        .route("/api/session", delete(handlers::auth::logout))
        .route("/api/user", get(handlers::user::get_current_user))
        .route("/api/user/password", put(handlers::user::change_password))
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
        .route("/entries/{id}", get(handlers::pages::entry_page))
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
            "/api/entries/mark-all-read",
            put(handlers::entry::mark_all_read),
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
        .with_state(state)
}
