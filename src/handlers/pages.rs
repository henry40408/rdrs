use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::middleware::auth::AuthUser;
use crate::AppState;

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub signup_enabled: bool,
}

impl IntoResponse for LoginTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn login_page(State(state): State<AppState>) -> LoginTemplate {
    let signup_enabled = {
        let conn = state.db.lock().ok();
        conn.and_then(|c| crate::models::user::count(&c).ok())
            .map(|count| state.config.can_register(count))
            .unwrap_or(false)
    };

    LoginTemplate { signup_enabled }
}

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterTemplate {
    pub error: Option<String>,
}

impl IntoResponse for RegisterTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn register_page(State(state): State<AppState>) -> RegisterTemplate {
    let can_register = {
        let conn = state.db.lock().ok();
        conn.and_then(|c| crate::models::user::count(&c).ok())
            .map(|count| state.config.can_register(count))
            .unwrap_or(false)
    };

    RegisterTemplate {
        error: if !can_register {
            Some("Registration is currently disabled".to_string())
        } else {
            None
        },
    }
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub username: String,
    pub role: String,
    pub sign_in_time: String,
}

impl IntoResponse for HomeTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn home_page(auth_user: AuthUser) -> HomeTemplate {
    HomeTemplate {
        username: auth_user.user.username,
        role: auth_user.user.role.as_str().to_string(),
        sign_in_time: auth_user
            .session
            .created_at
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    }
}

#[derive(Template)]
#[template(path = "change-password.html")]
pub struct ChangePasswordTemplate {}

impl IntoResponse for ChangePasswordTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn change_password_page(_auth_user: AuthUser) -> ChangePasswordTemplate {
    ChangePasswordTemplate {}
}
