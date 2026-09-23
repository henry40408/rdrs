use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, FromRequestParts, State},
    http::{HeaderMap, request::Parts},
};
use axum_extra::extract::CookieJar;
use chrono::Utc;

use crate::AppState;
use crate::auth::{verify_dummy_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::Bucket;
use crate::models::api_token::{self, ApiToken};
use crate::models::session::{self, Session};
use crate::models::user::{self, User};
use crate::secret::{DOMAIN_GREADER_TOKEN, tag, verify_tag};
use crate::services::audit;
use crate::utils::http::request_user_agent;

/// POST token validity duration in seconds (30 minutes).
const POST_TOKEN_VALIDITY_SECS: i64 = 30 * 60;

/// Where a `GReader` request's authority comes from: `Cookie` is a full web
/// session; `ApiToken` is a narrower, independently revocable grant.
#[derive(Debug, Clone)]
pub enum GReaderCredential {
    /// Web UI cookie path (signature verified by `session_token_from_jar`); the
    /// only way to carry a full web session.
    Cookie(Session),
    /// Native client `ClientLogin` path — an independent `api_token` row.
    ApiToken(ApiToken),
}

impl GReaderCredential {
    /// The MAC subject for a post token; the `rdrs_gr_` prefix keeps
    /// `api_tokens` from overlapping session tokens.
    pub fn post_token_subject(&self) -> &str {
        match self {
            GReaderCredential::Cookie(s) => &s.session_token,
            GReaderCredential::ApiToken(t) => &t.token,
        }
    }
}

/// Auth extractor: `Authorization: GoogleLogin auth=<token>` (an `api_token`
/// row), falling back to the session cookie.
#[derive(Debug, Clone)]
pub struct GReaderUser {
    pub user: User,
    pub credential: GReaderCredential,
    /// Whether the user was authenticated via cookie (skip POST token check).
    pub via_cookie: bool,
}

impl FromRequestParts<AppState> for GReaderUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(auth_header) = parts.headers.get("authorization")
            && let Ok(auth_str) = auth_header.to_str()
            && let Some(token) = auth_str
                .strip_prefix("GoogleLogin auth=")
                .map(|s| s.trim().to_string())
            && !token.is_empty()
        {
            match validate_api_token(state, &token).await {
                Ok((api_token, user)) => {
                    return Ok(GReaderUser {
                        user,
                        credential: GReaderCredential::ApiToken(api_token),
                        via_cookie: false,
                    });
                }
                // Deliberately not retried against `session`.
                Err(e) => return Err(e),
            }
        }

        let jar = CookieJar::from_request_parts(parts, state)
            .await
            .map_err(|_e| AppError::Unauthorized)?;

        let token = crate::middleware::auth::session_token_from_jar(&jar, &state.config.secret)
            .ok_or(AppError::Unauthorized)?;

        let (session, user) = validate_token(state, &token).await?;
        Ok(GReaderUser {
            user,
            credential: GReaderCredential::Cookie(session),
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
        audit::session_destroyed(&state.config.secret, token, session.user_id, "expired");
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

/// Validate a `GoogleLogin auth=<token>` value as an `api_token` row, with
/// no fallback to `session`.
async fn validate_api_token(state: &AppState, token: &str) -> AppResult<(ApiToken, User)> {
    let api_token = api_token::find_by_token(&state.db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if api_token.is_expired() {
        // Lazy delete, mirroring the session equivalent above.
        api_token::delete_token(&state.db, api_token.id, api_token.user_id).await?;
        return Err(AppError::Unauthorized);
    }

    let user = user::find_by_id(&state.db, api_token.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if user.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    // Best-effort: must not fail the request being authenticated.
    let _ = api_token::touch_and_refresh(&state.db, &api_token).await;

    Ok((api_token, user))
}

#[derive(Debug, serde::Deserialize)]
pub struct ClientLoginForm {
    #[serde(rename = "Email")]
    pub email: String,
    #[serde(rename = "Passwd")]
    pub passwd: String,
}

/// `POST /accounts/ClientLogin`
///
/// Returns `SID`, `LSID`, `Auth` as text/plain. `Auth` is an `api_token`, so a
/// leaked client token is not a full session takeover.
pub async fn client_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Form(form): Form<ClientLoginForm>,
) -> AppResult<String> {
    let username = form.email.clone();
    let password = form.passwd.clone();

    // Reserve an attempt before any lookup or verification, as web login does.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    let user_agent = request_user_agent(&headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::Login, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::Login, endpoint = "POST /accounts/ClientLogin", "credential attempt rate limited");
        audit::login_rate_limited("POST /accounts/ClientLogin", "login", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    // Per-account budget too, so a spray can't pick the unwatched protocol.
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire_account(Bucket::Login, &username)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::Login, subject = "account", endpoint = "POST /accounts/ClientLogin", "credential attempt rate limited");
        audit::login_rate_limited(
            "POST /accounts/ClientLogin",
            "login_account",
            &ip.to_string(),
        );
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    let Some(user) = user::find_by_username(&state.db, &username).await? else {
        // Equalise timing with the wrong-password path.
        verify_dummy_password(&password);
        audit::login_failed(username.len(), "unknown_user", &ip.to_string(), &user_agent);
        return Err(AppError::InvalidCredentials);
    };

    if !verify_password(&password, &user.password_hash) {
        audit::login_failed(username.len(), "bad_password", &ip.to_string(), &user_agent);
        return Err(AppError::InvalidCredentials);
    }

    // Correct password: release reservations before the disabled check.
    state.login_rate_limiter.release(Bucket::Login, ip);
    state
        .login_rate_limiter
        .release_account(Bucket::Login, &username);

    if user.is_disabled() {
        audit::login_failed(username.len(), "disabled", &ip.to_string(), &user_agent);
        return Err(AppError::UserDisabled);
    }

    let ip = ip.to_string();
    // The client's User-Agent labels the token on /user-settings.
    let label = user_agent.clone();
    let t = api_token::create_api_token(&state.db, user.id, "greader", &label, &user_agent, &ip)
        .await?;
    audit::api_token_created(
        &state.config.secret,
        &t.token,
        user.id,
        "client_login",
        &ip,
        &user_agent,
    );

    let _ = user; // user info not needed in response

    Ok(format!("SID=unused\nLSID=unused\nAuth={}", t.token))
}

/// `GET /reader/api/0/token`
///
/// Short-lived CSRF token `<timestamp>/<hmac_hex>`, keyed off the root secret.
pub async fn get_post_token(auth: GReaderUser, State(state): State<AppState>) -> AppResult<String> {
    let token = generate_post_token(&state.config.secret, auth.credential.post_token_subject());
    Ok(token)
}

/// The MAC input `<subject>/<timestamp>`; subjects never contain `/`, so the
/// concatenation is unambiguous.
fn post_token_parts<'a>(subject: &'a str, timestamp: &'a str) -> [&'a [u8]; 3] {
    [subject.as_bytes(), b"/", timestamp.as_bytes()]
}

/// Generate a POST token: `<timestamp>/<hmac_hex>`.
pub fn generate_post_token(secret: &[u8], subject: &str) -> String {
    let timestamp = Utc::now().timestamp().to_string();
    let hex = hex::encode(tag(
        secret,
        DOMAIN_GREADER_TOKEN,
        &post_token_parts(subject, &timestamp),
    ));
    format!("{timestamp}/{hex}")
}

/// Verify a POST token. Returns `Ok(())` if valid, `Err` if invalid or expired.
pub fn verify_post_token(secret: &[u8], subject: &str, post_token: &str) -> AppResult<()> {
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
        &post_token_parts(subject, ts_str),
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
