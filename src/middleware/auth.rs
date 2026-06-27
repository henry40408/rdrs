use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts},
    http::request::Parts,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::flash::FlashRedirect;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};

pub const SESSION_COOKIE_NAME: &str = "session_token";

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

        let token = jar
            .get(SESSION_COOKIE_NAME)
            .map(|c| c.value().to_string())
            .ok_or(AppError::Unauthorized)?;

        let token_clone = token.clone();
        let (session, expired) = state
            .db
            .user(move |conn| {
                let mut session =
                    session::find_by_token(conn, &token_clone)?.ok_or(AppError::Unauthorized)?;
                if session.is_expired() {
                    session::delete_session(conn, &token_clone)?;
                    return Ok::<_, AppError>((session, true));
                }
                if let Some(new_expires_at) = session::refresh_if_needed(conn, &session)? {
                    session.expires_at = new_expires_at;
                }
                Ok((session, false))
            })
            .await??;

        if expired {
            return Err(AppError::Unauthorized);
        }

        let user_id = session.user_id;
        let user = state
            .db
            .read_user(move |conn| user::find_by_id(conn, user_id)?.ok_or(AppError::Unauthorized))
            .await??;

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

        let token = jar
            .get(SESSION_COOKIE_NAME)
            .map(|c| c.value().to_string())
            .ok_or(LoginRedirect)?;

        let token_clone = token.clone();
        let result = state
            .db
            .user(move |conn| {
                let mut session = session::find_by_token(conn, &token_clone)
                    .map_err(|_e| ())?
                    .ok_or(())?;
                if session.is_expired() {
                    let _ = session::delete_session(conn, &token_clone);
                    return Err(());
                }
                if let Ok(Some(new_expires_at)) = session::refresh_if_needed(conn, &session) {
                    session.expires_at = new_expires_at;
                }
                let user = user::find_by_id(conn, session.user_id)
                    .map_err(|_e| ())?
                    .ok_or(())?;
                if user.is_disabled() {
                    return Err(());
                }
                Ok((user, session))
            })
            .await
            .map_err(|_e| LoginRedirect)?;

        let (user, session) = result.map_err(|_e| LoginRedirect)?;

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
                let original_user = state
                    .db
                    .read_user(move |conn| {
                        user::find_by_id(conn, original_user_id)?.ok_or(AppError::Unauthorized)
                    })
                    .await??;
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
                let original_user = state
                    .db
                    .read_user(move |conn| {
                        user::find_by_id(conn, original_user_id)
                            .map_err(|_e| ())?
                            .ok_or(())
                    })
                    .await
                    .map_err(|_e| LoginRedirect)?
                    .map_err(|_e| LoginRedirect)?;
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
