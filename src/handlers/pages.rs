use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

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
