use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::middleware::auth::{LoginRedirect, PageAuthUser};
use crate::middleware::flash::Flash;

/// Chrome-wrapped 404 page for logged-in routes.
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub heading: &'static str,
    pub message: String,
}

impl IntoResponse for ErrorTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (StatusCode::NOT_FOUND, Html(html)).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Build an [`ErrorTemplate`] 404.
pub async fn render_not_found(
    state: &AppState,
    auth_user: &PageAuthUser,
    flash: &Flash,
    heading: &'static str,
    message: impl Into<String>,
) -> ErrorTemplate {
    let layout = build_app_layout(state, auth_user, flash).await;
    ErrorTemplate {
        title: "Not Found",
        git_version: crate::GIT_VERSION,
        layout,
        heading,
        message: message.into(),
    }
}

/// Router fallback: 404 page when logged in, login redirect otherwise.
pub async fn not_found_page(
    State(state): State<AppState>,
    flash: Flash,
    auth_user: Result<PageAuthUser, LoginRedirect>,
) -> Response {
    match auth_user {
        Ok(user) => render_not_found(
            &state,
            &user,
            &flash,
            "Page not found",
            "The page you're looking for doesn't exist.",
        )
        .await
        .into_response(),
        Err(redirect) => redirect.into_response(),
    }
}
