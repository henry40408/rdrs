use axum::{
    Form,
    extract::{FromRequestParts, State},
    http::request::Parts,
};
use axum_extra::extract::CookieJar;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::AppState;
use crate::auth::verify_password;
use crate::error::{AppError, AppResult};
use crate::middleware::auth::SESSION_COOKIE_NAME;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};

type HmacSha256 = Hmac<Sha256>;

/// POST token validity duration in seconds (30 minutes).
const POST_TOKEN_VALIDITY_SECS: i64 = 30 * 60;

// --- GReaderUser extractor ---

/// Authentication extractor for Google Reader API endpoints.
/// Supports dual auth:
///   1. `Authorization: GoogleLogin auth=<token>` header (external clients)
///   2. Session cookie fallback (Web UI)
#[derive(Debug, Clone)]
pub struct GReaderUser {
    pub user: User,
    pub session: Session,
    /// Whether the user was authenticated via cookie (skip POST token check).
    pub via_cookie: bool,
}

impl FromRequestParts<AppState> for GReaderUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 1. Try Authorization header: "GoogleLogin auth=<token>"
        if let Some(auth_header) = parts.headers.get("authorization")
            && let Ok(auth_str) = auth_header.to_str()
            && let Some(token) = auth_str
                .strip_prefix("GoogleLogin auth=")
                .map(|s| s.trim().to_string())
            && !token.is_empty()
        {
            let (session, user) = validate_token(state, &token).await?;
            return Ok(GReaderUser {
                user,
                session,
                via_cookie: false,
            });
        }

        // 2. Fallback: session cookie
        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| AppError::Unauthorized)?;

        let token = jar
            .get(SESSION_COOKIE_NAME)
            .map(|c| c.value().to_string())
            .ok_or(AppError::Unauthorized)?;

        let (session, user) = validate_token(state, &token).await?;
        Ok(GReaderUser {
            user,
            session,
            via_cookie: true,
        })
    }
}

/// Validate a session token and return (Session, User).
async fn validate_token(state: &AppState, token: &str) -> AppResult<(Session, User)> {
    let token_owned = token.to_string();
    let (session, expired) = state
        .db
        .user(move |conn| {
            let session =
                session::find_by_token(conn, &token_owned)?.ok_or(AppError::Unauthorized)?;
            if session.is_expired() {
                session::delete_session(conn, &token_owned)?;
                return Ok::<_, AppError>((session, true));
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

    Ok((session, user))
}

// --- ClientLogin endpoint ---

#[derive(Debug, serde::Deserialize)]
pub struct ClientLoginForm {
    #[serde(rename = "Email")]
    pub email: String,
    #[serde(rename = "Passwd")]
    pub passwd: String,
}

/// `POST /accounts/ClientLogin`
///
/// Google Reader `ClientLogin`: accepts form-encoded Email + Passwd,
/// returns `SID`, `LSID`, `Auth` in text/plain.
pub async fn client_login(
    State(state): State<AppState>,
    Form(form): Form<ClientLoginForm>,
) -> AppResult<String> {
    let username = form.email.clone();
    let password = form.passwd.clone();

    let (user, new_session) = state
        .db
        .user(move |conn| {
            let user =
                user::find_by_username(conn, &username)?.ok_or(AppError::InvalidCredentials)?;

            if !verify_password(&password, &user.password_hash) {
                return Err(AppError::InvalidCredentials);
            }

            if user.is_disabled() {
                return Err(AppError::UserDisabled);
            }

            let new_session = session::create_session(conn, user.id)?;
            Ok::<_, AppError>((user, new_session))
        })
        .await??;

    let _ = user; // user info not needed in response

    Ok(format!(
        "SID=unused\nLSID=unused\nAuth={}",
        new_session.session_token
    ))
}

// --- POST Token endpoint ---

/// `GET /reader/api/0/token`
///
/// Returns a short-lived POST token for CSRF protection.
/// The token is HMAC(secret, `session_token` + timestamp).
pub async fn get_post_token(auth: GReaderUser, State(state): State<AppState>) -> AppResult<String> {
    let token = generate_post_token(
        &state.config.image_proxy_secret,
        &auth.session.session_token,
    );
    Ok(token)
}

/// Generate a POST token: `<timestamp>/<hmac_hex>`.
pub fn generate_post_token(secret: &[u8], session_token: &str) -> String {
    let timestamp = Utc::now().timestamp();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(session_token.as_bytes());
    mac.update(timestamp.to_string().as_bytes());
    let result = mac.finalize();
    let hex = hex::encode(result.into_bytes());
    format!("{}/{}", timestamp, hex)
}

/// Verify a POST token. Returns `Ok(())` if valid, `Err` if invalid or expired.
pub fn verify_post_token(secret: &[u8], session_token: &str, post_token: &str) -> AppResult<()> {
    let parts: Vec<&str> = post_token.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(AppError::Unauthorized);
    }

    let timestamp: i64 = parts[0].parse().map_err(|_e| AppError::Unauthorized)?;

    let now = Utc::now().timestamp();
    if now - timestamp > POST_TOKEN_VALIDITY_SECS {
        return Err(AppError::Unauthorized);
    }

    // Recompute HMAC
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(session_token.as_bytes());
    mac.update(timestamp.to_string().as_bytes());

    let expected = hex::encode(mac.finalize().into_bytes());
    if expected != parts[1] {
        return Err(AppError::Unauthorized);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_post_token_roundtrip() {
        let secret = b"test-secret-key-for-hmac";
        let session_token = "abc123session";

        let token = generate_post_token(secret, session_token);
        assert!(verify_post_token(secret, session_token, &token).is_ok());
    }

    #[test]
    fn test_post_token_wrong_secret() {
        let secret = b"test-secret-key-for-hmac";
        let session_token = "abc123session";

        let token = generate_post_token(secret, session_token);
        assert!(verify_post_token(b"wrong-secret", session_token, &token).is_err());
    }

    #[test]
    fn test_post_token_wrong_session() {
        let secret = b"test-secret-key-for-hmac";

        let token = generate_post_token(secret, "session1");
        assert!(verify_post_token(secret, "session2", &token).is_err());
    }

    #[test]
    fn test_post_token_invalid_format() {
        let secret = b"test-secret-key-for-hmac";
        assert!(verify_post_token(secret, "session", "invalid").is_err());
    }
}
