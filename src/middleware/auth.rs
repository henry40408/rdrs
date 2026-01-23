use axum::{
    extract::FromRequestParts,
    http::request::Parts,
};
use axum_extra::extract::CookieJar;

use crate::error::AppError;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};
use crate::AppState;

pub const SESSION_COOKIE_NAME: &str = "session_token";

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;

        let token = jar
            .get(SESSION_COOKIE_NAME)
            .map(|c| c.value().to_string())
            .ok_or(AppError::Unauthorized)?;

        let conn = state.db.lock().map_err(|_| AppError::Internal("Database lock error".to_string()))?;

        let session = session::find_by_token(&conn, &token)?
            .ok_or(AppError::Unauthorized)?;

        if session.is_expired() {
            session::delete_session(&conn, &token)?;
            return Err(AppError::Unauthorized);
        }

        let user = user::find_by_id(&conn, session.user_id)?
            .ok_or(AppError::Unauthorized)?;

        if user.is_disabled() {
            return Err(AppError::UserDisabled);
        }

        Ok(AuthUser { user, session })
    }
}

#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user: User,
    pub session: Session,
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let auth_user = AuthUser::from_request_parts(parts, state).await?;

        if auth_user.session.is_masquerading() {
            let conn = state.db.lock().map_err(|_| AppError::Internal("Database lock error".to_string()))?;
            if let Some(original_user_id) = auth_user.session.original_user_id {
                let original_user = user::find_by_id(&conn, original_user_id)?
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
