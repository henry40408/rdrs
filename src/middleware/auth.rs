use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts},
    http::request::Parts,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use time::Duration;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::flash::FlashRedirect;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};

pub const SESSION_COOKIE_NAME: &str = "session_token";

/// Build the session cookie carrying `token`.
///
/// Every login path (password, passkey, forward-auth) goes through here so the
/// attributes cannot drift apart between them. The cookie *value* is the token
/// plus an HMAC signature (`<token>.<hmac>`, see [`crate::secret::sign_session`]),
/// so a tampered or forged cookie is rejected before any database lookup, and a
/// leaked `session.session_token` is not usable without the root key. `secure`
/// comes from [`crate::config::Config::cookie_secure`], which is derived from
/// `RDRS_PUBLIC_BASE_URL`'s scheme.
pub fn build_session_cookie(token: &str, secret: &[u8], secure: bool) -> Cookie<'static> {
    Cookie::build((
        SESSION_COOKIE_NAME,
        crate::secret::sign_session(secret, token),
    ))
    .path("/")
    .http_only(true)
    .secure(secure)
    .same_site(SameSite::Lax)
    .max_age(Duration::days(session::SESSION_ABSOLUTE_MAX_DAYS))
    .build()
}

/// Read the signed session cookie from `jar` and return the database token it
/// carries, or `None` when the cookie is absent or its signature does not
/// verify. Every extractor and the forward-auth middleware funnel through here,
/// so the signature is always checked before the token reaches the database.
pub fn session_token_from_jar(jar: &CookieJar, secret: &[u8]) -> Option<String> {
    let value = jar.get(SESSION_COOKIE_NAME)?.value().to_string();
    crate::secret::verify_session(secret, &value)
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| AppError::Unauthorized)?;

        let token =
            session_token_from_jar(&jar, &state.config.secret).ok_or(AppError::Unauthorized)?;

        let mut session = session::find_by_token(&state.db, &token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let expired = if session.is_expired() {
            session::delete_session(&state.db, &token).await?;
            true
        } else {
            if let Some(new_expires_at) = session::refresh_if_needed(&state.db, &session).await? {
                session.expires_at = new_expires_at;
            }
            let _ = session::touch_last_seen(&state.db, &session).await;
            false
        };

        if expired {
            return Err(AppError::Unauthorized);
        }

        let user_id = session.user_id;
        let user = user::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        if user.is_disabled() {
            return Err(AppError::UserDisabled);
        }

        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let via_forward_auth = crate::middleware::forward_auth::forward_auth_identity(
            &state.config,
            peer_ip,
            &parts.headers,
        )
        .is_some();

        Ok(AuthUser {
            user,
            session,
            via_forward_auth,
        })
    }
}

/// Auth extractor for page routes that redirects to login on unauthorized
#[derive(Debug, Clone)]
pub struct PageAuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}

/// Redirect response for unauthorized page access
pub struct LoginRedirect;

impl IntoResponse for LoginRedirect {
    fn into_response(self) -> Response {
        FlashRedirect::warning("/login", "Please log in to continue.").into_response()
    }
}

impl FromRequestParts<AppState> for PageAuthUser {
    type Rejection = LoginRedirect;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| LoginRedirect)?;

        let token = session_token_from_jar(&jar, &state.config.secret).ok_or(LoginRedirect)?;

        let Ok(Some(mut session)) = session::find_by_token(&state.db, &token).await else {
            return Err(LoginRedirect);
        };
        if session.is_expired() {
            let _ = session::delete_session(&state.db, &token).await;
            return Err(LoginRedirect);
        }
        if let Ok(Some(new_expires_at)) = session::refresh_if_needed(&state.db, &session).await {
            session.expires_at = new_expires_at;
        }
        let _ = session::touch_last_seen(&state.db, &session).await;
        let Ok(Some(user)) = user::find_by_id(&state.db, session.user_id).await else {
            return Err(LoginRedirect);
        };
        if user.is_disabled() {
            return Err(LoginRedirect);
        }

        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let via_forward_auth = crate::middleware::forward_auth::forward_auth_identity(
            &state.config,
            peer_ip,
            &parts.headers,
        )
        .is_some();

        Ok(PageAuthUser {
            user,
            session,
            via_forward_auth,
        })
    }
}

impl OptionalFromRequestParts<AppState> for PageAuthUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        match <PageAuthUser as FromRequestParts<AppState>>::from_request_parts(parts, state).await {
            Ok(user) => Ok(Some(user)),
            Err(_) => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user: User,
    pub session: Session,
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_user = AuthUser::from_request_parts(parts, state).await?;

        if auth_user.session.is_masquerading() {
            if let Some(original_user_id) = auth_user.session.original_user_id {
                let original_user = user::find_by_id(&state.db, original_user_id)
                    .await?
                    .ok_or(AppError::Unauthorized)?;
                if !original_user.is_admin() {
                    return Err(AppError::Forbidden);
                }
            } else {
                return Err(AppError::Forbidden);
            }
        } else if !auth_user.user.is_admin() {
            return Err(AppError::Forbidden);
        }

        Ok(AdminUser {
            user: auth_user.user,
            session: auth_user.session,
        })
    }
}

/// Admin extractor for page routes that redirects to login on unauthorized
#[derive(Debug, Clone)]
pub struct PageAdminUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}

impl FromRequestParts<AppState> for PageAdminUser {
    type Rejection = LoginRedirect;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let page_auth_user =
            <PageAuthUser as FromRequestParts<AppState>>::from_request_parts(parts, state).await?;

        if page_auth_user.session.is_masquerading() {
            if let Some(original_user_id) = page_auth_user.session.original_user_id {
                let Ok(Some(original_user)) = user::find_by_id(&state.db, original_user_id).await
                else {
                    return Err(LoginRedirect);
                };
                if !original_user.is_admin() {
                    return Err(LoginRedirect);
                }
            } else {
                return Err(LoginRedirect);
            }
        } else if !page_auth_user.user.is_admin() {
            return Err(LoginRedirect);
        }

        Ok(PageAdminUser {
            user: page_auth_user.user,
            session: page_auth_user.session,
            via_forward_auth: page_auth_user.via_forward_auth,
        })
    }
}
