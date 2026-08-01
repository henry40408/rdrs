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

// --- GReaderUser extractor ---

/// Where a `GReader` request's authority comes from. The two are not
/// interchangeable: a `Cookie` credential is a full web session (whatever it
/// is authorized to do, so is this request), while an `ApiToken` credential is
/// a narrower, independently-revocable GReader-only grant. Neither can forge
/// the other — see `handlers/greader/auth.rs`'s `FromRequestParts` for how
/// each is verified before this enum is ever constructed.
#[derive(Debug, Clone)]
pub enum GReaderCredential {
    /// Web UI cookie path — the signature was already verified by
    /// `session_token_from_jar`. This is the *only* way a `GReader` request
    /// can carry a full web session: an `Authorization` header token is never
    /// matched against `session`.
    Cookie(Session),
    /// Native client `ClientLogin` path — an independent `api_token` row.
    ApiToken(ApiToken),
}

impl GReaderCredential {
    /// The MAC subject for a post token. Both kinds share `DOMAIN_GREADER_TOKEN`,
    /// but `api_tokens` carry the `rdrs_gr_` prefix so the two subject spaces
    /// cannot overlap.
    pub fn post_token_subject(&self) -> &str {
        match self {
            GReaderCredential::Cookie(s) => &s.session_token,
            GReaderCredential::ApiToken(t) => &t.token,
        }
    }
}

/// Authentication extractor for Google Reader API endpoints.
/// Supports dual auth:
///   1. `Authorization: GoogleLogin auth=<token>` header (external clients) —
///      validated as an `api_token` row (`GReaderCredential::ApiToken`).
///   2. Session cookie fallback (Web UI) — `GReaderCredential::Cookie`.
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
        // 1. Try Authorization header: "GoogleLogin auth=<token>"
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
                // A header token that is not an `api_token` row is simply
                // rejected. It is deliberately *not* retried against
                // `session`: that coupling is what this table exists to
                // remove, and a migration window for it would in practice
                // just be left switched on forever.
                Err(e) => return Err(e),
            }
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

/// Validate an `Authorization: GoogleLogin auth=<token>` value as an
/// independent `api_token` row (not the raw `session.session_token` — that
/// coupling is exactly what this table exists to remove). Returns
/// `AppError::Unauthorized` when no row matches; there is no fallback to
/// `session`, so a pre-cutover client must re-run `ClientLogin`.
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

    // Best-effort: a failure here must not fail the request the token is
    // authenticating.
    let _ = api_token::touch_and_refresh(&state.db, &api_token).await;

    Ok((api_token, user))
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
///
/// `Auth` is an `api_token` row, not the caller's web session: a token leaked
/// from an RSS reader app must not be equivalent to a full session takeover
/// (see `models::api_token` and the module doc on `secret.rs`).
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

    // Per-account budget as well, mirroring the web login endpoint — a
    // distributed spray would otherwise simply pick whichever of the two
    // login protocols was not watching the account.
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
        // Equalise the "no such account" path with the "wrong password" one;
        // see the web login handler for why the clock, not the message, is
        // what leaks here.
        verify_dummy_password(&password);
        audit::login_failed(username.len(), "unknown_user", &ip.to_string(), &user_agent);
        return Err(AppError::InvalidCredentials);
    };

    if !verify_password(&password, &user.password_hash) {
        audit::login_failed(username.len(), "bad_password", &ip.to_string(), &user_agent);
        return Err(AppError::InvalidCredentials);
    }

    // Correct password: hand both reservations back before the
    // disabled-account check, same as the web login endpoint.
    state.login_rate_limiter.release(Bucket::Login, ip);
    state
        .login_rate_limiter
        .release_account(Bucket::Login, &username);

    if user.is_disabled() {
        audit::login_failed(username.len(), "disabled", &ip.to_string(), &user_agent);
        return Err(AppError::UserDisabled);
    }

    let ip = ip.to_string();
    // The client's own reported User-Agent doubles as the label shown on the
    // /user-settings revocation list — GReader clients don't send anything
    // more identifying than that.
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

// --- POST Token endpoint ---

/// `GET /reader/api/0/token`
///
/// Returns a short-lived POST token for CSRF protection.
/// The token is `<timestamp>/<hmac_hex>`, keyed off the shared root secret.
pub async fn get_post_token(auth: GReaderUser, State(state): State<AppState>) -> AppResult<String> {
    let token = generate_post_token(&state.config.secret, auth.credential.post_token_subject());
    Ok(token)
}

/// The MAC input for a post token: the credential subject, a separator, and
/// the timestamp. Session tokens and `api_tokens` both use the `A-Za-z0-9-_`
/// alphabet and so never contain `/`, which makes the concatenation
/// unambiguous — without the separator `("a", 12)` and `("a1", 2)` would sign
/// the same bytes.
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
