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
        .with_state(state)
}
