use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, FromRequestParts, State},
    http::{HeaderMap, request::Parts},
};
use axum_extra::extract::CookieJar;
use chrono::Utc;

use crate::AppState;
use crate::auth::verify_password;
use crate::error::{AppError, AppResult};
use crate::middleware::Bucket;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};
use crate::secret::{DOMAIN_GREADER_TOKEN, tag, verify_tag};
use crate::utils::http::request_user_agent;

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

        let token = crate::middleware::auth::session_token_from_jar(&jar, &state.config.secret)
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
    let session = session::find_by_token(&state.db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if session.is_expired() {
        session::delete_session(&state.db, token).await?;
        return Err(AppError::Unauthorized);
    }

    let user = user::find_by_id(&state.db, session.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

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
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Form(form): Form<ClientLoginForm>,
) -> AppResult<String> {
    let username = form.email.clone();
    let password = form.passwd.clone();

    // Reserve an attempt before the username lookup or password
    // verification, mirroring the web login endpoint — this is the same
    // credential check, just fronting a different client protocol.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if !state.login_rate_limiter.try_acquire(Bucket::Login, ip) {
        tracing::warn!(%ip, bucket = ?Bucket::Login, endpoint = "POST /accounts/ClientLogin", "credential attempt rate limited");
        return Err(AppError::TooManyRequests);
    }

    let user = user::find_by_username(&state.db, &username)
        .await?
        .ok_or(AppError::InvalidCredentials)?;

    if !verify_password(&password, &user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    // Correct password: hand the reservation back before the disabled-account
    // check, same as the web login endpoint.
    state.login_rate_limiter.release(Bucket::Login, ip);

    if user.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    let user_agent = request_user_agent(&headers);
    let ip = ip.to_string();
    let new_session = session::create_session(&state.db, user.id, &user_agent, &ip).await?;

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
/// The token is `<timestamp>/<hmac_hex>`, keyed off the shared root secret.
pub async fn get_post_token(auth: GReaderUser, State(state): State<AppState>) -> AppResult<String> {
    let token = generate_post_token(&state.config.secret, &auth.session.session_token);
    Ok(token)
}

/// The MAC input for a post token: the session token, a separator, and the
/// timestamp. Session tokens use the `A-Za-z0-9-_` alphabet and so never
/// contain `/`, which makes the concatenation unambiguous — without the
/// separator `("a", 12)` and `("a1", 2)` would sign the same bytes.
fn post_token_parts<'a>(session_token: &'a str, timestamp: &'a str) -> [&'a [u8]; 3] {
    [session_token.as_bytes(), b"/", timestamp.as_bytes()]
}

/// Generate a POST token: `<timestamp>/<hmac_hex>`.
pub fn generate_post_token(secret: &[u8], session_token: &str) -> String {
    let timestamp = Utc::now().timestamp().to_string();
    let hex = hex::encode(tag(
        secret,
        DOMAIN_GREADER_TOKEN,
        &post_token_parts(session_token, &timestamp),
    ));
    format!("{timestamp}/{hex}")
}

/// Verify a POST token. Returns `Ok(())` if valid, `Err` if invalid or expired.
pub fn verify_post_token(secret: &[u8], session_token: &str, post_token: &str) -> AppResult<()> {
    let (ts_str, sig_hex) = post_token.split_once('/').ok_or(AppError::Unauthorized)?;

    let timestamp: i64 = ts_str.parse().map_err(|_e| AppError::Unauthorized)?;
    let now = Utc::now().timestamp();
    if now - timestamp > POST_TOKEN_VALIDITY_SECS {
        return Err(AppError::Unauthorized);
    }

    let sig = hex::decode(sig_hex).map_err(|_e| AppError::Unauthorized)?;
    if !verify_tag(
        secret,
        DOMAIN_GREADER_TOKEN,
        &post_token_parts(session_token, ts_str),
        &sig,
    ) {
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
