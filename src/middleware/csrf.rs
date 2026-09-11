//! CSRF defence, in two independent lines so a bypass of one is not a bypass of
//! both.
//!
//! The first line is `tower_http::csrf::CsrfLayer`, a header-only, stateless
//! check layered over the whole router. On a state-changing method it allows a
//! `Sec-Fetch-Site` of `same-origin` or `none` and rejects any other value —
//! `same-site` included, which a browser sends for a sibling subdomain or
//! another port on the same host, and which `SameSite=Lax` still hands the
//! session cookie. Only where a browser omits that header does it fall back to
//! comparing the `Origin`'s full authority, port included, with the request's
//! own. A request carrying neither header is a non-browser client, which
//! authenticates by bearer token rather than an ambient cookie and so is not
//! exposed to CSRF. What this module adds to that layer is
//! [`log_cross_site_rejection`].
//!
//! The second line is the synchronizer token: [`csrf_guard`], fed by the
//! cookies [`anonymous_session`] mints.

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

/// Readable (non-`HttpOnly`) cookie carrying the CSRF token, so page JavaScript
/// can echo it back. It is *not* the credential the guard trusts — [`csrf_guard`]
/// always re-derives the expected token from the signed session cookie — so
/// exposing it to script is safe: a cross-origin page can neither read this
/// cookie nor compute its value.
pub const CSRF_COOKIE_NAME: &str = "csrf_token";

/// `__Host-`-prefixed CSRF cookie name, used only when `Secure` is in effect.
/// Mirrors [`crate::middleware::auth::SESSION_COOKIE_NAME_HOST`] for the same
/// reasoning — see that constant for why this is defence in depth and why the
/// prefix cannot be used unconditionally.
///
/// `__Host-` does not require `HttpOnly`, so it does not conflict with this
/// cookie needing to stay script-readable.
pub const CSRF_COOKIE_NAME_HOST: &str = "__Host-csrf_token";

/// Which cookie name to *write* for the CSRF cookie, given whether `Secure`
/// is in effect. See [`CSRF_COOKIE_NAME_HOST`].
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

/// The one route still exempt from the synchronizer-token guard. `ClientLogin`
/// exchanges a username and password for an API token, so a forged call has to
/// carry credentials the attacker already knows, and its response is unreadable
/// cross-origin. What is left — logging a victim's client into the *attacker's*
/// account — is login-CSRF, which the first-line origin guard already stops. Gating it
/// would only break a logged-in operator minting a client token by hand.
///
/// Registered under both the bare path and the FreshRSS-compatible
/// `/api/greader.php` prefix, hence the suffix match rather than a prefix one.
const CSRF_SKIP_SUFFIX: &str = "/accounts/ClientLogin";

/// Path prefixes for which no anonymous session is minted: static assets, the
/// service worker and health must stay cacheable, and the machine APIs get their
/// cookie from a real page load. Every HTML page a form is rendered on lives
/// outside these — `/offline` is the one HTML page listed, and it carries a link
/// rather than a form precisely so it can.
const ANON_SKIP_PREFIXES: &[&str] = &[
    "/api",
    "/reader",
    "/accounts",
    "/static",
    "/favicon",
    "/health",
    "/sw.js",
    "/offline",
    // The open-tracking pixel, fetched by clients that hold no session and
    // would discard the cookie anyway — minting one per image request would put
    // a pointless `Set-Cookie` on every entry a client syncs.
    "/p/",
];

/// Upper bound on a buffered request body when reading the `_csrf` field. Browser
/// form POSTs are small; 1 MiB caps what a malicious client could force us to
/// hold in memory.
const CSRF_MAX_BODY_BYTES: usize = 1 << 20;

/// Build the readable CSRF cookie for a session token. Mirrors the session
/// cookie's `Path`, `SameSite`, `Secure`, and `Max-Age` so the two travel
/// together, but is deliberately **not** `HttpOnly` — script must read it.
pub fn build_csrf_cookie(session_token: &str, secret: &[u8], secure: bool) -> Cookie<'static> {
    Cookie::build((csrf_cookie_name(secure), derive_csrf(secret, session_token)))
        .path("/")
        .http_only(false)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
        .build()
}

/// The synchronizer token to render into a server-side form, for the session
/// this request carries. Empty when the request holds no readable session cookie
/// at all, which for a browser only happens on the routes
/// [`ANON_SKIP_PREFIXES`] excludes — none of which render a form.
///
/// Derived the same way [`csrf_guard`] derives the value it *expects*: from the
/// token in the session cookie, not from a `Session` row. The two diverge for
/// the grace interval after a rotation, and it is the cookie the browser will
/// send back with the form.
///
/// Logged-in pages get this via [`PageAuthUser::csrf_token`][pau] instead; this
/// is for the anonymous pages that render a form.
///
/// [pau]: crate::middleware::auth::PageAuthUser::csrf_token
pub fn csrf_token_from_jar(jar: &CookieJar, secret: &[u8]) -> String {
    session_token_from_jar(jar, secret)
        .map(|token| derive_csrf(secret, &token))
        .unwrap_or_default()
}

/// Removal cookie for a CSRF cookie carried under `name`, used to evict a
/// leftover generation written under the name this deployment no longer uses.
///
/// The `__Host-` removal carries `Secure` unconditionally, regardless of the
/// current setting: a browser silently discards a `__Host-` cookie that lacks
/// it, which would make the removal a no-op and let the stale cookie survive.
fn csrf_removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .secure(name == CSRF_COOKIE_NAME_HOST)
        .max_age(Duration::ZERO)
        .build()
}

/// Log each rejection by the first-line origin guard, `tower_http`'s
/// `CsrfLayer`, which must be layered directly inside this one.
///
/// Both CSRF layers answer with a bodyless 403, indistinguishable in an access
/// log, so without this line telling a cross-site rejection apart from a token
/// mismatch means reading the source. The layer's own rejection builder is
/// handed the [`ProtectionError`] and nothing else, so the method, path and
/// headers worth logging are captured here on the way in and paired with the
/// error the layer attaches to its response on the way out. The *response*
/// stays bodyless on purpose — an attacker's page cannot read it anyway, and
/// naming the failed check only helps someone probing the guard.
pub async fn log_cross_site_rejection(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let sec_fetch_site = req.headers().get("sec-fetch-site").cloned();
    let origin = req.headers().get(header::ORIGIN).cloned();

    let res = next.run(req).await;
    if let Some(err) = res.extensions().get::<ProtectionError>() {
        tracing::warn!(
            event = "csrf.cross_site",
            check = %rejected_by(err.kind()),
            method = %method,
            path = %uri.path(),
            sec_fetch_site = header_str(sec_fetch_site.as_ref()),
            origin = header_str(origin.as_ref()),
            "rejected a state-changing request the browser reported as cross-site"
        );
    }
    res
}

/// Which of the origin guard's checks fired. The `Origin` fallback is the one
/// only an old browser — or a proxy that rewrites `Host` — can trip, so it is
/// worth telling apart in the log.
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

/// Whether `method` cannot change server state and so needs no CSRF check.
fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Give a logged-out visitor a signed session cookie so every page can carry a
/// CSRF token, and make sure a readable [`CSRF_COOKIE_NAME`] cookie is present
/// for whatever session the request ends up with.
///
/// The token is signed but backs no `session` row, so the visitor stays
/// unauthenticated while still holding a token the guard can verify. Both
/// cookies are injected into *this* request as well as set on the response, so
/// the guard and the handler see them on the same round trip.
///
/// Layered *inside* `forward_auth`: when both would establish a session,
/// forward-auth must win, so its `Set-Cookie` has to be last. A request that
/// already carries a valid session cookie keeps it — only its CSRF cookie is
/// rewritten, and only when missing or no longer derived from the session, which
/// is what carries existing sessions across the upgrade that introduced this
/// cookie and heals a browser whose cookie has drifted.
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
        // A real (or already-anonymous) session: leave the session cookie alone,
        // but make sure the readable CSRF cookie the page echoes back actually
        // *matches* it.
        //
        // Validating rather than merely detecting presence is what makes a
        // diverged browser heal. A present-but-stale cookie — left by a rotated
        // token, or by an upgrade that switched which name is written — used to
        // satisfy the old presence check forever, so every unsafe request 403'd
        // until it expired, with no in-app way out: logout is itself behind
        // `csrf_guard`. Overwriting is safe by construction, since the CSRF
        // cookie is never the credential.
        let name = csrf_cookie_name(secure);
        let matches_session = jar
            .get(name)
            .is_some_and(|c| verify_csrf(secret, &token, c.value()));

        // A cookie under the name this deployment does *not* write is a leftover
        // generation. Nothing refreshes it, so its value drifts away from the
        // session, and `csrf.js` on an older page — or any reader scanning
        // `document.cookie` in order — can pick it over the live one.
        let stale_name = csrf_cookie_name(!secure);
        let stale = jar.get(stale_name).is_some();

        if matches_session && !stale {
            return next.run(req).await;
        }

        // The fresh cookie is written even when the held one already matches,
        // whenever a removal rides along: `slide_session_cookie` skips its own
        // reissue once it sees a `Set-Cookie` under *either* CSRF name, so a
        // lone removal would leave this response carrying no live token.
        let mut cookies = Vec::with_capacity(2);
        let csrf = build_csrf_cookie(&token, secret, secure);
        set_request_cookie(&mut req, &csrf);
        cookies.push(csrf);
        if stale {
            cookies.push(csrf_removal_cookie(stale_name));
        }
        return with_set_cookies(next.run(req).await, &cookies);
    }

    // No session at all: mint an anonymous one plus its CSRF cookie.
    let token = generate_token();
    let session = build_session_cookie(&token, secret, secure);
    let csrf = build_csrf_cookie(&token, secret, secure);
    set_request_cookie(&mut req, &session);
    set_request_cookie(&mut req, &csrf);
    with_set_cookies(next.run(req).await, &[session, csrf])
}

/// Synchronizer-token CSRF guard, the second line behind the origin guard (see
/// the module docs).
///
/// On every state-changing method it requires the request to prove it holds the
/// session's token, from the `X-CSRF-Token` header or the `_csrf` form field
/// (the body is buffered and rebuilt so the handler still reads it). The
/// expected token is re-derived from the signed session cookie via
/// [`verify_csrf`] — a MAC over a known input — so this costs no database round
/// trip and the readable cookie is never trusted as the credential.
///
/// `multipart/form-data` bodies are passed through unread: the one multipart
/// route validates the field itself, since re-streaming a file upload here would
/// be wasteful.
///
/// The Google Reader surface is *not* exempt, despite its own clients being
/// unable to carry a token: they authenticate by bearer header and so hold no
/// session cookie, which the cookie-less pass-through below already lets
/// through. Exempting the paths instead would have skipped the one case that
/// does need checking — a browser calling `/reader/api/0/*` with an ambient
/// cookie, which `GReaderUser` accepts as a credential and for which
/// `verify_post_token_if_needed` then waives the `GReader` `T` token, leaving
/// nothing but the origin guard in front of it. `static/js/csrf.js`
/// already puts `X-CSRF-Token` on those `fetch` calls and no template posts a
/// form to those paths, so requiring it costs the app nothing. The cost is a
/// client sending *both* a bearer header and a live session cookie: it now
/// needs the token too. [`CSRF_SKIP_SUFFIX`] carves out the sole exception.
pub async fn csrf_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if is_safe(req.method()) || req.uri().path().ends_with(CSRF_SKIP_SUFFIX) {
        return next.run(req).await;
    }

    let secret = &state.config.secret;
    // An unsigned or tampered session cookie never resolves, so no submitted
    // token could match it.
    //
    // A request with *no* session cookie is passed through rather than rejected.
    // A CSRF attack necessarily rides the victim's cookie, which the browser
    // attaches automatically, so a cookie-less request cannot be a forged
    // authenticated action — it reaches a handler that rejects it on its own
    // `AuthUser` check. Login-CSRF, the one cookie-less case worth guarding, is
    // already stopped by the origin guard.
    let jar = CookieJar::from_headers(req.headers());
    let Some(session_token) = session_token_from_jar(&jar, secret) else {
        return next.run(req).await;
    };

    // Header path — no body to buffer.
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

    // A multipart handler validates the field itself; see the OPML import route.
    if is_multipart(&req) {
        return next.run(req).await;
    }

    // Body path: buffer, read `_csrf`, then rebuild the request unchanged.
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

/// Log a synchronizer-token rejection.
///
/// The session is identified only by its salted [`crate::secret::audit_id`]
/// hash, as the `rdrs::audit` events are — enough to see that one browser is
/// failing every unsafe request, the signature of a CSRF cookie that has drifted
/// out of step, without putting a live session token in the log.
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

/// Whether the request body is `multipart/form-data`.
fn is_multipart(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.trim_start().starts_with("multipart/form-data"))
}

/// Rewrite the request's `Cookie` header so downstream extractors see `cookie`
/// in place of any prior entry of the same name. Replacing rather than appending
/// matters: `CookieJar::get` returns the first match, so a stale value left in
/// front would shadow the one just set.
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

/// Append each cookie as a `Set-Cookie` header on the response.
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
        // A browser silently discards a `__Host-` cookie that lacks `Secure`,
        // so a non-Secure removal would be a no-op and the stale cookie would
        // outlive the eviction it was meant to trigger.
        assert_eq!(
            csrf_removal_cookie(CSRF_COOKIE_NAME_HOST).secure(),
            Some(true)
        );
        assert_ne!(csrf_removal_cookie(CSRF_COOKIE_NAME).secure(), Some(true));
    }
}
