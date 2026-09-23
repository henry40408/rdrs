use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts, OptionalFromRequestParts, Request, State},
    http::{HeaderValue, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::Utc;
use time::Duration;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::flash::FlashRedirect;
use crate::models::session::{self, Session};
use crate::models::user::{self, User};
use crate::services::audit;

pub const SESSION_COOKIE_NAME: &str = "session_token";

/// `__Host-`-prefixed session cookie name (OWASP): stops a sibling subdomain
/// from shadowing our cookie. Defence in depth only, since values are HMAC-signed.
/// Must never be written without `Secure` (the browser discards it, breaking
/// plain-HTTP login), so select it only via [`session_cookie_name`].
pub const SESSION_COOKIE_NAME_HOST: &str = "__Host-session_token";

/// Session cookie name to *write*: prefixed only when `Secure` is in effect.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE_NAME_HOST
    } else {
        SESSION_COOKIE_NAME
    }
}

/// Build the session cookie for `token`; every login path must use this. The
/// value is HMAC-signed, so forgeries fail before any DB lookup and a leaked
/// `session.session_token` is useless without the root key.
pub fn build_session_cookie(token: &str, secret: &[u8], secure: bool) -> Cookie<'static> {
    Cookie::build((
        session_cookie_name(secure),
        crate::secret::sign_session(secret, token),
    ))
    .path("/")
    .http_only(true)
    .secure(secure)
    .same_site(SameSite::Lax)
    .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
    .build()
}

/// Return the verified session token from `jar`, or `None`.
///
/// Tries [`SESSION_COOKIE_NAME_HOST`], then [`SESSION_COOKIE_NAME`] even if the
/// prefixed one is present but invalid: flipping `RDRS_COOKIE_SECURE` must not log
/// everyone out, and a stale empty `__Host-` cookie must not shadow a valid one.
/// Safe because forgery resistance comes from the HMAC alone.
pub fn session_token_from_jar(jar: &CookieJar, secret: &[u8]) -> Option<String> {
    if let Some(token) = jar
        .get(SESSION_COOKIE_NAME_HOST)
        .and_then(|c| crate::secret::verify_session(secret, c.value()))
    {
        return Some(token);
    }
    let value = jar.get(SESSION_COOKIE_NAME)?.value().to_string();
    crate::secret::verify_session(secret, &value)
}

/// Lets an extractor ask [`slide_session_cookie`] to rotate the session token.
/// Rotation is deferred to the middleware because only it knows the response can
/// carry the new cookie; an undelivered rotation would sign the client out.
#[derive(Clone, Default)]
pub struct RotationSlot(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl RotationSlot {
    /// Request rotation on the way out; no-op on routes outside the middleware
    /// (e.g. `/events`).
    pub fn request(parts: &Parts) {
        if let Some(slot) = parts.extensions.get::<Self>() {
            slot.0.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn requested(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Cacheable paths that must never get a `Set-Cookie`. Narrower than
/// `ANON_SKIP_PREFIXES` in `csrf.rs` so API clients still get `Max-Age` renewed;
/// other public responses are caught by [`response_is_publicly_cacheable`].
const SLIDE_SKIP_PREFIXES: &[&str] = &["/static", "/favicon", "/health", "/sw.js", "/offline"];

/// Reissue the session and CSRF cookies on every request with a verified session
/// cookie so `Max-Age` slides with use, and perform any rotation requested via
/// [`RotationSlot`].
///
/// Must never clobber inner layers' `Set-Cookie`s: if either name of a cookie
/// purpose is already set, it is left alone — otherwise logout's removal cookies
/// would be undone by a live one.
pub async fn slide_session_cookie(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if SLIDE_SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    let jar = CookieJar::from_headers(req.headers());
    let Some(token) = session_token_from_jar(&jar, &state.config.secret) else {
        return next.run(req).await;
    };

    let secret = &state.config.secret;
    let secure = state.config.cookie_secure;

    let slot = RotationSlot::default();
    req.extensions_mut().insert(slot.clone());

    let mut resp = next.run(req).await;

    // A session cookie on a shared-cacheable response would leak the session to
    // the next visitor. Checked before rotating, since this response can't carry it.
    if response_is_publicly_cacheable(&resp) {
        return resp;
    }

    // `None` means a concurrent request rotated first; our token stays valid as
    // the grace token.
    let token = if slot.requested() {
        match session::rotate_token(&state.db, &token).await {
            Ok(Some(rotated)) => {
                audit::session_token_rotated(secret, &token, &rotated);
                rotated
            }
            Ok(None) => token,
            Err(e) => {
                tracing::warn!(
                    event = "session.rotation_failed",
                    error = %e,
                    "session token rotation failed; keeping current token"
                );
                token
            }
        }
    } else {
        token
    };

    if !response_has_set_cookie_for_any(&resp, &[SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST]) {
        append_set_cookie(&mut resp, &build_session_cookie(&token, secret, secure));
    }
    if !response_has_set_cookie_for_any(
        &resp,
        &[
            crate::middleware::CSRF_COOKIE_NAME,
            crate::middleware::CSRF_COOKIE_NAME_HOST,
        ],
    ) {
        append_set_cookie(
            &mut resp,
            &crate::middleware::build_csrf_cookie(&token, secret, secure),
        );
    }

    resp
}

/// Whether `resp` sets cookie `name` (exact name match, not a substring search).
fn response_has_set_cookie_for(resp: &Response, name: &str) -> bool {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.split_once('=').is_some_and(|(n, _)| n.trim() == name))
}

/// Whether `resp` sets any of `names` (a purpose's prefixed and unprefixed names).
fn response_has_set_cookie_for_any(resp: &Response, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| response_has_set_cookie_for(resp, name))
}

/// Whether `resp` is shared-cacheable (`Cache-Control` without `no-store` or
/// `private`), so no `Set-Cookie` may be attached. Missing header counts as not.
fn response_is_publicly_cacheable(resp: &Response) -> bool {
    let Some(value) = resp.headers().get(header::CACHE_CONTROL) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value.split(',').all(|directive| {
        let name = directive.split_once('=').map_or(directive, |(n, _)| n);
        let name = name.trim();
        !name.eq_ignore_ascii_case("no-store") && !name.eq_ignore_ascii_case("private")
    })
}

fn append_set_cookie(resp: &mut Response, cookie: &Cookie<'static>) {
    if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}

/// An [`AuthUser`] authenticated within [`session::REAUTH_WINDOW_MINUTES`]
/// (OWASP reauthentication), guarding passkey add/remove so a hijacked session
/// can't mint a credential that survives a password change.
///
/// Forward-auth sessions are exempt: the proxy asserts identity per request and
/// the account may have no password. Rejects with
/// [`AppError::ReauthenticationRequired`].
#[derive(Debug, Clone)]
pub struct RecentlyAuthenticated {
    pub user: User,
    pub session: Session,
}

impl FromRequestParts<AppState> for RecentlyAuthenticated {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_user = AuthUser::from_request_parts(parts, state).await?;
        if !auth_user.via_forward_auth && !auth_user.session.authenticated_recently(Utc::now()) {
            return Err(AppError::ReauthenticationRequired);
        }
        Ok(Self {
            user: auth_user.user,
            session: auth_user.session,
        })
    }
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
            audit::session_destroyed(&state.config.secret, &token, session.user_id, "expired");
            true
        } else {
            if let Some(new_expires_at) = session::refresh_if_needed(&state.db, &session).await? {
                audit::session_renewed(
                    &state.config.secret,
                    &token,
                    session.user_id,
                    new_expires_at,
                );
                session.expires_at = new_expires_at;
                // Also due for token rotation, done by `slide_session_cookie`.
                RotationSlot::request(parts);
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
    /// CSRF token for no-JS forms. Derived from the *cookie* token, not
    /// `session.session_token`, which differs during a rotation's grace interval.
    pub csrf_token: String,
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
            audit::session_destroyed(&state.config.secret, &token, session.user_id, "expired");
            return Err(LoginRedirect);
        }
        if let Ok(Some(new_expires_at)) = session::refresh_if_needed(&state.db, &session).await {
            audit::session_renewed(
                &state.config.secret,
                &token,
                session.user_id,
                new_expires_at,
            );
            session.expires_at = new_expires_at;
            RotationSlot::request(parts);
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
            csrf_token: crate::secret::derive_csrf(&state.config.secret, &token),
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
    /// Lets handlers exempt forward-auth sessions from password reconfirmation,
    /// as [`RecentlyAuthenticated`] does.
    pub via_forward_auth: bool,
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
            via_forward_auth: auth_user.via_forward_auth,
        })
    }
}

/// Admin extractor for page routes that redirects to login on unauthorized
#[derive(Debug, Clone)]
pub struct PageAdminUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
    /// See [`PageAuthUser::csrf_token`].
    pub csrf_token: String,
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
            csrf_token: page_auth_user.csrf_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Response as HttpResponse;

    fn resp_with_set_cookies(cookies: &[&str]) -> Response {
        let mut builder = HttpResponse::builder();
        for c in cookies {
            builder = builder.header(header::SET_COOKIE, *c);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn response_has_set_cookie_for_matches_exact_name() {
        let resp = resp_with_set_cookies(&["session_token=abc123; Path=/; HttpOnly"]);
        assert!(response_has_set_cookie_for(&resp, "session_token"));
        assert!(!response_has_set_cookie_for(&resp, "csrf_token"));
    }

    #[test]
    fn response_has_set_cookie_for_is_not_fooled_by_value_containing_name() {
        let resp = resp_with_set_cookies(&["other=session_token_lookalike; Path=/"]);
        assert!(!response_has_set_cookie_for(&resp, "session_token"));
    }

    #[test]
    fn response_has_set_cookie_for_handles_attribute_laden_value() {
        let resp = resp_with_set_cookies(&[
            "csrf_token=xyz; Path=/; SameSite=Lax; Max-Age=604800; Secure",
        ]);
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
        assert!(!response_has_set_cookie_for(&resp, "session_token"));
    }

    #[test]
    fn response_has_set_cookie_for_checks_each_header_independently() {
        let resp = resp_with_set_cookies(&["csrf_token=abc; Path=/"]);
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
        assert!(!response_has_set_cookie_for(&resp, "session_token"));

        let resp = resp_with_set_cookies(&["session_token=; Path=/", "csrf_token=abc; Path=/"]);
        assert!(response_has_set_cookie_for(&resp, "session_token"));
        assert!(response_has_set_cookie_for(&resp, "csrf_token"));
    }

    #[test]
    fn response_has_set_cookie_for_any_matches_either_name() {
        // Logout's __Host- removal cookie must count as covered.
        let resp = resp_with_set_cookies(&["__Host-session_token=; Path=/; Secure"]);
        assert!(response_has_set_cookie_for_any(
            &resp,
            &[SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST]
        ));
        assert!(!response_has_set_cookie_for_any(
            &resp,
            &[crate::middleware::CSRF_COOKIE_NAME, "__Host-csrf_token"]
        ));
    }

    fn resp_with_cache_control(value: &str) -> Response {
        HttpResponse::builder()
            .header(header::CACHE_CONTROL, value)
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn response_is_publicly_cacheable_false_when_no_cache_control_header() {
        let resp = HttpResponse::builder().body(Body::empty()).unwrap();
        assert!(!response_is_publicly_cacheable(&resp));
    }

    #[test]
    fn response_is_publicly_cacheable_by_directive() {
        for (directive, cacheable) in [
            ("no-store", false),
            ("private, max-age=0", false),
            ("No-Store", false),
            ("public, max-age=86400", true),
            // `public` is not required.
            ("max-age=600", true),
        ] {
            assert_eq!(
                response_is_publicly_cacheable(&resp_with_cache_control(directive)),
                cacheable,
                "{directive}"
            );
        }
    }

    #[test]
    fn session_cookie_name_is_prefixed_only_when_secure() {
        assert_eq!(session_cookie_name(true), SESSION_COOKIE_NAME_HOST);
        assert_eq!(session_cookie_name(false), SESSION_COOKIE_NAME);
    }

    #[test]
    fn build_session_cookie_prefixed_variant_carries_secure_and_root_path() {
        let cookie = build_session_cookie("tok", b"01234567890123456789012345678901", true);
        assert_eq!(cookie.name(), SESSION_COOKIE_NAME_HOST);
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.path(), Some("/"));
        // The browser rejects a __Host- cookie with a Domain.
        assert_eq!(cookie.domain(), None);
    }

    #[test]
    fn build_session_cookie_unprefixed_variant_when_not_secure() {
        let cookie = build_session_cookie("tok", b"01234567890123456789012345678901", false);
        assert_eq!(cookie.name(), SESSION_COOKIE_NAME);
        assert_eq!(cookie.secure(), Some(false));
        assert_eq!(cookie.domain(), None);
    }
}
