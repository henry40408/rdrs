use askama::Template;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};

use crate::AppState;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::{Flash, FlashMessage};

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    /// First-run setup link; only when no accounts exist.
    pub setup_available: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    pub local_auth_enabled: bool,
    /// CSRF token for the no-JS `POST /login`, from the anonymous session.
    pub csrf_token: String,
    /// Sign-in error for the no-JS path.
    pub error: Option<String>,
    /// Validated page to land on after sign-in; see [`login_next`].
    pub next: Option<String>,
}

/// `/login?next=` (and the form's `next` field) when it is a safe, same-origin
/// page; see [`crate::middleware::auth::LoginRedirect`].
pub fn login_next(raw: Option<&str>) -> Option<String> {
    use crate::handlers::return_to::{is_login_destination, safe_return_to};
    raw.and_then(|r| safe_return_to(r, is_login_destination))
}

/// `/login` query.
#[derive(serde::Deserialize)]
pub struct LoginQuery {
    pub next: Option<String>,
}

pub async fn login_page(
    auth: Option<PageAuthUser>,
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    flash: Flash,
    Query(query): Query<LoginQuery>,
) -> Response {
    // Not `next`: a signed-in user sent here was refused a page (e.g. admin).
    if auth.is_some() {
        return Redirect::to("/").into_response();
    }

    let setup_available = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_setup(count));

    (
        flash.clone(),
        LoginTemplate {
            setup_available,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            local_auth_enabled: !state.config.disable_local_auth,
            csrf_token: crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
            error: None,
            next: login_next(query.next.as_deref()),
        },
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "setup.html")]
pub struct SetupTemplate {
    pub error: Option<String>,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    /// Passed through so `minlength`/`maxlength` match server validation.
    pub password_min_length: usize,
    pub password_max_length: usize,
    /// CSRF token for the no-JS `POST /setup`.
    pub csrf_token: String,
}

/// `GET /setup` — first-run only; redirects to `/login` once any account exists.
pub async fn setup_page(
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    flash: Flash,
) -> Response {
    let can_setup = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_setup(count));

    if !can_setup {
        return Redirect::to("/login").into_response();
    }

    (
        flash.clone(),
        SetupTemplate {
            error: None,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
            csrf_token: crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
        },
    )
        .into_response()
}

/// Invite "set your password" page: form, dead end, or throttled notice. The
/// dead end must not reveal why (unknown / expired / used) or whose link it is.
#[derive(Template)]
#[template(path = "invite.html")]
pub struct InviteTemplate {
    pub git_version: &'static str,
    /// Anonymous-session CSRF token; empty when no form renders.
    pub csrf_token: String,
    /// `None` renders the dead end; `Some` renders the form for that account.
    pub username: Option<String>,
    /// Echoed into the form action so the POST lands on the same link.
    pub token: String,
    pub error: Option<String>,
    pub password_min_length: usize,
    pub password_max_length: usize,
}

impl InviteTemplate {
    pub fn form(token: &str, username: String, csrf_token: String) -> Self {
        Self {
            git_version: crate::GIT_VERSION,
            csrf_token,
            username: Some(username),
            token: token.to_string(),
            error: None,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
        }
    }

    pub fn error(token: &str, username: String, message: &str, csrf_token: String) -> Self {
        Self {
            error: Some(message.to_string()),
            ..Self::form(token, username, csrf_token)
        }
    }

    /// The one response every failing token gets, whatever the reason.
    pub fn invalid() -> Self {
        Self {
            git_version: crate::GIT_VERSION,
            // Renders no form, so there is nothing to protect.
            csrf_token: String::new(),
            username: None,
            token: String::new(),
            error: None,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
        }
    }

    /// Rate-limited; unlike [`InviteTemplate::invalid`], the link may be fine.
    pub fn throttled(retry_after_secs: u64) -> Self {
        Self {
            error: Some(format!(
                "Too many attempts. Please try again in {retry_after_secs} seconds."
            )),
            ..Self::invalid()
        }
    }
}
