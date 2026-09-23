//! CSRF defence in two independent lines, so bypassing one does not bypass both.
//!
//! 1. `tower_http::csrf::CsrfLayer` over the whole router: an unsafe method
//!    needs `Sec-Fetch-Site` of `same-origin` or `none` (`same-site` is rejected:
//!    sibling subdomains and other ports still get `SameSite=Lax` cookies), else
//!    falls back to an exact `Origin` authority match. Requests with neither are
//!    non-browser bearer clients. This module adds [`log_cross_site_rejection`].
//! 2. The synchronizer token: [`csrf_guard`], fed by cookies
//!    [`anonymous_session`] mints.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use time::Duration;
use tower_http::csrf::{ProtectionError, ProtectionErrorKind};

use crate::AppState;
use crate::middleware::auth::{build_session_cookie, session_token_from_jar};
use crate::models::session::{self, generate_token};
use crate::secret::{derive_csrf, verify_csrf};

/// Script-readable cookie carrying the CSRF token. Safe to expose: it is never
/// the credential — [`csrf_guard`] re-derives the expected token from the signed
/// session cookie.
pub const CSRF_COOKIE_NAME: &str = "csrf_token";

/// `__Host-` CSRF cookie name, used only when `Secure` is in effect; see
/// [`crate::middleware::auth::SESSION_COOKIE_NAME_HOST`].
pub const CSRF_COOKIE_NAME_HOST: &str = "__Host-csrf_token";

/// Which CSRF cookie name to *write*, given whether `Secure` is in effect.
pub fn csrf_cookie_name(secure: bool) -> &'static str {
    if secure {
        CSRF_COOKIE_NAME_HOST
    } else {
        CSRF_COOKIE_NAME
    }
}

/// Header a browser echoes the token back in for `fetch`-driven mutations.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// The form field carrying the token on a body-submitted POST.
const CSRF_FIELD: &str = "_csrf";

/// The only route exempt from the synchronizer-token guard. A forged
/// `ClientLogin` needs credentials the attacker already knows, its response is
/// unreadable cross-origin, and login-CSRF is stopped by the origin guard.
/// Suffix-matched because it is also mounted under `/api/greader.php`.
const CSRF_SKIP_SUFFIX: &str = "/accounts/ClientLogin";

/// Paths that never get an anonymous session: cacheable assets and machine
/// APIs. None renders a form (`/offline` deliberately uses a link).
const ANON_SKIP_PREFIXES: &[&str] = &[
    "/api",
    "/reader",
    "/accounts",
    "/static",
    "/favicon",
    "/health",
    "/sw.js",
    "/offline",
    // Tracking pixel: sessionless clients would just discard the cookie.
    "/p/",
];

/// Cap on the body buffered to read `_csrf` (1 MiB).
const CSRF_MAX_BODY_BYTES: usize = 1 << 20;

/// Build the CSRF cookie for a session token, matching the session cookie's
/// attributes except that it is **not** `HttpOnly`.
pub fn build_csrf_cookie(session_token: &str, secret: &[u8], secure: bool) -> Cookie<'static> {
    Cookie::build((csrf_cookie_name(secure), derive_csrf(secret, session_token)))
        .path("/")
        .http_only(false)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
        .build()
}

/// The token to render into an anonymous page's form (logged-in pages use
/// [`PageAuthUser::csrf_token`][pau]); empty without a session cookie.
///
/// Derived from the session *cookie*, as [`csrf_guard`] does, not the `Session`
/// row: they diverge during the post-rotation grace interval.
///
/// [pau]: crate::middleware::auth::PageAuthUser::csrf_token
pub fn csrf_token_from_jar(jar: &CookieJar, secret: &[u8]) -> String {
    session_token_from_jar(jar, secret)
        .map(|token| derive_csrf(secret, &token))
        .unwrap_or_default()
}

/// Removal cookie evicting a CSRF cookie under the name this deployment no
/// longer writes. The `__Host-` one always carries `Secure`, or browsers ignore it.
fn csrf_removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .secure(name == CSRF_COOKIE_NAME_HOST)
        .max_age(Duration::ZERO)
        .build()
}

/// Log rejections by `tower_http`'s `CsrfLayer`, which must be layered directly
/// inside this one, so they are distinguishable from token mismatches. The 403
/// stays bodyless on purpose: naming the failed check only helps a prober.
pub async fn log_cross_site_rejection(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let sec_fetch_site = req.headers().get("sec-fetch-site").cloned();
    let origin = req.headers().get(header::ORIGIN).cloned();

    let res = next.run(req).await;
    if let Some(err) = res.extensions().get::<ProtectionError>() {
        // Bound here rather than written inline: llvm-cov reports a call inside
        // a `tracing` field list as never executed, even when the event fires.
        let check = rejected_by(err.kind());
        let path = uri.path();
        let sec_fetch_site = header_str(sec_fetch_site.as_ref());
        let origin = header_str(origin.as_ref());
        tracing::warn!(
            event = "csrf.cross_site",
            check = %check,
            method = %method,
            path = %path,
            sec_fetch_site,
            origin,
            "rejected a state-changing request the browser reported as cross-site"
        );
    }
    res
}

/// Which origin-guard check fired; the `Origin` fallback flags an old browser
/// or a `Host`-rewriting proxy.
fn rejected_by(kind: ProtectionErrorKind) -> &'static str {
    match kind {
        ProtectionErrorKind::CrossOriginRequest => "sec_fetch_site",
        ProtectionErrorKind::CrossOriginRequestFromOldBrowser => "origin_fallback",
        // `ProtectionErrorKind` is `#[non_exhaustive]`.
        _ => "other",
    }
}

/// A header value as a string for logging, or `"-"` when absent or non-ASCII.
fn header_str(value: Option<&HeaderValue>) -> &str {
    value.and_then(|v| v.to_str().ok()).unwrap_or("-")
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Give a logged-out visitor a signed session cookie backing no `session` row,
/// so every page can carry a CSRF token, and ensure the CSRF cookie matches
/// whatever session the request has. Cookies are injected into this request
/// too, so the guard and handler see them on the same round trip.
///
/// Layered *inside* `forward_auth` so forward-auth's `Set-Cookie` wins.
pub async fn anonymous_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if ANON_SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }
    let secret = &state.config.secret;
    let secure = state.config.cookie_secure;
    let jar = CookieJar::from_headers(req.headers());

    if let Some(token) = session_token_from_jar(&jar, secret) {
        // Keep the session; verify (not just detect) the CSRF cookie, so a
        // stale one heals instead of 403ing every unsafe request — including
        // logout. Overwriting is safe: the CSRF cookie is never the credential.
        let name = csrf_cookie_name(secure);
        let matches_session = jar
            .get(name)
            .is_some_and(|c| verify_csrf(secret, &token, c.value()));

        // A cookie under the other name is a leftover that drifts and can be
        // picked over the live one by `document.cookie` readers.
        let stale_name = csrf_cookie_name(!secure);
        let stale = jar.get(stale_name).is_some();

        if matches_session && !stale {
            return next.run(req).await;
        }

        // Always reissue alongside a removal: `slide_session_cookie` skips its
        // reissue on seeing either CSRF name, leaving no live token otherwise.
        let mut cookies = Vec::with_capacity(2);
        let csrf = build_csrf_cookie(&token, secret, secure);
        set_request_cookie(&mut req, &csrf);
        cookies.push(csrf);
        if stale {
            cookies.push(csrf_removal_cookie(stale_name));
        }
        return with_set_cookies(next.run(req).await, &cookies);
    }

    let token = generate_token();
    let session = build_session_cookie(&token, secret, secure);
    let csrf = build_csrf_cookie(&token, secret, secure);
    set_request_cookie(&mut req, &session);
    set_request_cookie(&mut req, &csrf);
    with_set_cookies(next.run(req).await, &[session, csrf])
}

/// Synchronizer-token guard, the second CSRF line.
///
/// Unsafe methods must carry the session's token in `X-CSRF-Token` or the
/// `_csrf` form field (body buffered and rebuilt). The expected token is a MAC
/// re-derived from the signed session cookie via [`verify_csrf`]: no DB trip,
/// and the readable cookie is never trusted. Multipart bodies pass through; that
/// route checks the field itself.
///
/// The `GReader` API is deliberately *not* exempt: bearer clients carry no
/// cookie and pass anyway, but a browser with an ambient cookie there has its
/// `T` token waived, leaving only the origin guard. `CSRF_SKIP_SUFFIX` is the
/// sole exception.
pub async fn csrf_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if is_safe(req.method()) || req.uri().path().ends_with(CSRF_SKIP_SUFFIX) {
        return next.run(req).await;
    }

    let secret = &state.config.secret;
    // No (valid) session cookie passes through: CSRF rides the victim's cookie,
    // so this cannot be a forged authenticated action, and `AuthUser` rejects
    // it downstream. Login-CSRF is stopped by the origin guard.
    let jar = CookieJar::from_headers(req.headers());
    let Some(session_token) = session_token_from_jar(&jar, secret) else {
        return next.run(req).await;
    };

    if let Some(submitted) = req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) {
        if verify_csrf(secret, &session_token, submitted) {
            return next.run(req).await;
        }
        warn_token_mismatch(
            secret,
            &session_token,
            req.method(),
            req.uri().path(),
            "header token does not derive from this session",
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    // The OPML import route validates the field itself.
    if is_multipart(&req) {
        return next.run(req).await;
    }

    let (parts, body) = req.into_parts();
    let path = parts.uri.path();
    let Ok(bytes) = axum::body::to_bytes(body, CSRF_MAX_BODY_BYTES).await else {
        warn_token_mismatch(
            secret,
            &session_token,
            &parts.method,
            path,
            "body unreadable or over the buffering limit",
        );
        return StatusCode::FORBIDDEN.into_response();
    };
    let ok = url::form_urlencoded::parse(&bytes)
        .find(|(k, _)| k == CSRF_FIELD)
        .is_some_and(|(_, v)| verify_csrf(secret, &session_token, &v));
    if !ok {
        warn_token_mismatch(
            secret,
            &session_token,
            &parts.method,
            path,
            "no _csrf field, or it does not derive from this session",
        );
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// Log a token rejection, identifying the session only by its salted
/// [`crate::secret::audit_id`] — never the live token.
fn warn_token_mismatch(
    secret: &[u8],
    session_token: &str,
    method: &Method,
    path: &str,
    reason: &'static str,
) {
    tracing::warn!(
        event = "csrf.mismatch",
        reason,
        method = %method,
        path = %path,
        session = %crate::secret::audit_id(secret, session_token),
        "rejected a state-changing request whose CSRF token did not match its session"
    );
}

fn is_multipart(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.trim_start().starts_with("multipart/form-data"))
}

/// Replace (not append) `cookie` in the request's `Cookie` header:
/// `CookieJar::get` returns the first match, so a stale value would shadow it.
fn set_request_cookie(req: &mut Request, cookie: &Cookie<'static>) {
    let prefix = format!("{}=", cookie.name());
    let kept = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|pair| !pair.is_empty() && !pair.starts_with(&prefix))
        .map(str::to_owned)
        .chain(std::iter::once(format!(
            "{}={}",
            cookie.name(),
            cookie.value()
        )))
        .collect::<Vec<_>>()
        .join("; ");
    if let Ok(value) = HeaderValue::from_str(&kept) {
        req.headers_mut().insert(header::COOKIE, value);
    }
}

fn with_set_cookies(mut resp: Response, cookies: &[Cookie<'static>]) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
            resp.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_prefixed_removal_carries_secure_unconditionally() {
        // Browsers discard a `__Host-` cookie lacking `Secure`.
        assert_eq!(
            csrf_removal_cookie(CSRF_COOKIE_NAME_HOST).secure(),
            Some(true)
        );
        assert_ne!(csrf_removal_cookie(CSRF_COOKIE_NAME).secure(), Some(true));
    }
}
