//! First-line CSRF defence: reject state-changing requests that a browser
//! reports, or reveals, to be cross-site.
//!
//! This is a header-only check with no token and no state — the synchronizer
//! token layered on top (see [`crate::secret::derive_csrf`]) is the second line.
//! It runs on every unsafe-method request across the whole router, but only
//! ever *rejects* a request that is provably cross-site; anything it cannot
//! classify is passed through, so it never breaks a legitimate caller:
//!
//! - **`Sec-Fetch-Site`** (sent by every current browser) is authoritative when
//!   present. `same-origin`, `same-site`, and `none` (a direct navigation or a
//!   user-typed URL) are allowed; only `cross-site` is rejected.
//! - **`Origin`** is the fallback for the rare browser that omits
//!   `Sec-Fetch-Site`. Its host is compared against the request's own `Host`;
//!   a mismatch — or an opaque `Origin: null` — is rejected.
//! - **Neither header** means a non-browser client (a native Google Reader app,
//!   `curl`, a server-to-server call). Those authenticate by bearer token, not
//!   an ambient cookie, so they are not exposed to CSRF and are allowed through.
//!
//! Scheme and port are deliberately ignored in the `Origin`/`Host` comparison:
//! behind a TLS-terminating reverse proxy the browser's `Origin` is `https://`
//! while the forwarded `Host` carries no scheme, and the proxy commonly strips
//! the port. Matching on host alone is what keeps the check working in that
//! standard deployment without a configured public URL.

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

use crate::AppState;
use crate::middleware::auth::{build_session_cookie, session_token_from_jar};
use crate::models::session::{self, generate_token};
use crate::secret::{derive_csrf, verify_csrf};

/// Readable (non-`HttpOnly`) cookie carrying the CSRF token, so page JavaScript
/// can echo it back as the `X-CSRF-Token` header or inject it into a form. It is
/// *not* the credential the guard trusts — [`csrf_guard`] always re-derives the
/// expected token from the signed session cookie — so exposing it to script is
/// safe: a cross-origin page can neither read this cookie nor compute its value.
pub const CSRF_COOKIE_NAME: &str = "csrf_token";

/// `__Host-`-prefixed CSRF cookie name, used only when `Secure` is in effect
/// (see [`csrf_cookie_name`]). Mirrors [`crate::middleware::auth::SESSION_COOKIE_NAME_HOST`]
/// for the same OWASP *Cookies* reasoning — see that constant's doc comment
/// for why this is defence in depth rather than a fix for an exploitable gap,
/// and why the prefix cannot be used unconditionally.
///
/// `__Host-` does not require `HttpOnly`, so prefixing this cookie does not
/// conflict with it needing to stay script-readable.
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

/// Path prefixes exempt from the synchronizer-token guard: the Google Reader
/// surface authenticates by bearer token or its own `T` post token, not by an
/// ambient cookie, so a browser CSRF cannot forge those calls.
const CSRF_SKIP_PREFIXES: &[&str] = &["/reader", "/accounts", "/api/greader.php"];

/// Path prefixes for which no anonymous session is minted: static assets and
/// health must stay cacheable (a `Set-Cookie` would poison shared caches), and
/// the machine APIs get their cookie, if any, from a real page load. Every HTML
/// page a form is rendered on lives outside these, so the token is always
/// available where it is needed.
const ANON_SKIP_PREFIXES: &[&str] = &[
    "/api",
    "/reader",
    "/accounts",
    "/static",
    "/favicon",
    "/health",
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

/// Removal cookie for a CSRF cookie carried under `name`, used to evict a
/// leftover generation written under the name this deployment no longer uses.
///
/// The `__Host-` removal carries `Secure` unconditionally — regardless of the
/// current `cookie_secure` setting — because a browser silently discards a
/// `__Host-` cookie that lacks it, which would make the removal a no-op and
/// let the stale cookie survive. Same reasoning as `handlers::auth::logout`.
fn csrf_removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .path("/")
        .secure(name == CSRF_COOKIE_NAME_HOST)
        .max_age(Duration::ZERO)
        .build()
}

/// Reject a state-changing request that is provably cross-site. See the module
/// docs for the classification. Safe methods (GET/HEAD/OPTIONS/TRACE) never
/// change state and pass through untouched.
pub async fn csrf_origin_guard(req: Request, next: Next) -> Response {
    if is_safe(req.method()) || !is_cross_site(&req) {
        return next.run(req).await;
    }
    // Both CSRF layers answer with a bodyless 403, indistinguishable from one
    // another in an access log; without these lines, telling a cross-site
    // rejection apart from a token mismatch means reading the source and
    // reasoning about headers by hand. The *response* stays bodyless on
    // purpose — an attacker's page cannot read a cross-origin response anyway,
    // and naming the failed check only helps someone probing the guard.
    tracing::warn!(
        event = "csrf.cross_site",
        method = %req.method(),
        path = %req.uri().path(),
        sec_fetch_site = header_str(&req, "sec-fetch-site"),
        origin = header_str(&req, "origin"),
        "rejected a state-changing request the browser reported as cross-site"
    );
    StatusCode::FORBIDDEN.into_response()
}

/// A request header as a string for logging, or `"-"` when absent or non-ASCII.
fn header_str<'a>(req: &'a Request, name: &'static str) -> &'a str {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
}

/// Whether `method` cannot change server state and so needs no CSRF check.
fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Whether the request is one a browser has told us — via `Sec-Fetch-Site` or a
/// mismatched `Origin` — is cross-site. A request a browser did not mark, and
/// that carries no `Origin`, is treated as not-cross-site (a non-browser
/// client); see the module docs.
fn is_cross_site(req: &Request) -> bool {
    let headers = req.headers();

    // `Sec-Fetch-Site` is authoritative where the browser sends it.
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return site.eq_ignore_ascii_case("cross-site");
    }

    // Fall back to comparing the Origin's host with the request's own Host.
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // No Sec-Fetch-Site and no Origin → a non-browser client.
        return false;
    };
    // `Origin: null` is opaque (a sandboxed iframe, a cross-origin redirect) and
    // never legitimate for a state-changing request here.
    if origin.eq_ignore_ascii_case("null") {
        return true;
    }
    let Some(origin_host) = host_of(origin) else {
        return true;
    };
    let request_host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(strip_port);
    // A missing/garbled Host with a present Origin cannot be confirmed
    // same-origin, so treat it as cross-site.
    request_host != Some(origin_host)
}

/// The host of an `Origin` value (`scheme://host[:port]`), lower-cased and with
/// any port removed. `None` when there is no `://` authority to read.
fn host_of(origin: &str) -> Option<String> {
    let authority = origin.split_once("://").map(|(_, rest)| rest)?;
    Some(strip_port(authority).to_ascii_lowercase())
}

/// Strip a trailing `:port` from a host authority, leaving the host. Handles
/// bracketed IPv6 literals (`[::1]:8080` → `[::1]`).
fn strip_port(authority: &str) -> String {
    if let Some(end) = authority
        .strip_prefix('[')
        .and_then(|_| authority.find(']'))
    {
        // Bracketed IPv6: keep through the closing bracket, drop any `:port`.
        return authority[..=end].to_ascii_lowercase();
    }
    authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host)
        .to_ascii_lowercase()
}

/// Give a logged-out visitor a signed session cookie so every page — the login
/// and register forms included — can carry a CSRF token, and make sure a
/// readable [`CSRF_COOKIE_NAME`] cookie is present for whatever session the
/// request ends up with.
///
/// The token is signed but backs no `session` row, so the visitor stays
/// unauthenticated (`find_by_token` finds nothing) while still holding a token
/// the guard can verify. Both cookies are injected into *this* request as well
/// as set on the response, so [`csrf_guard`] and the handler see them on the
/// same round trip.
///
/// Layered *inside* `forward_auth` (see `crate::app`): when both would establish
/// a session, forward-auth must win, so its `Set-Cookie` has to be the last one
/// emitted. A request that already carries a valid session cookie keeps it —
/// only its CSRF cookie is (re)written, and only when that cookie is missing or
/// no longer derives from the session. That is what carries existing sessions
/// across the upgrade that introduced this cookie, and what heals a browser
/// whose CSRF cookie has drifted out of step.
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
        // A real (or already-anonymous) session: leave the session cookie
        // untouched, but make sure the readable CSRF cookie the page will echo
        // back actually *matches* it.
        //
        // This validates rather than merely detecting presence, and that is
        // what makes a diverged browser heal itself. A cookie that is present
        // but stale — left behind by a rotated session token, or by an upgrade
        // that switched which name is written — used to satisfy the old
        // presence check forever, so every unsafe request 403'd until the
        // cookie expired (up to `SESSION_EXPIRY_DAYS`) with no in-app way out:
        // logout is itself behind `csrf_guard`. Overwriting it is safe by
        // construction — the CSRF cookie is never the credential, the guard
        // always re-derives the expected token from the signed session cookie
        // (see the note on `CSRF_COOKIE_NAME`), so re-minting hands an attacker
        // nothing they could not already compute for their own session.
        let name = csrf_cookie_name(secure);
        let matches_session = jar
            .get(name)
            .is_some_and(|c| verify_csrf(secret, &token, c.value()));

        // A cookie under the name this deployment does *not* write is a
        // leftover generation (from before the `__Host-` names arrived, or
        // before an operator flipped `RDRS_COOKIE_SECURE`). Nothing refreshes
        // it, so its value drifts away from the session, and `csrf.js` on an
        // older page — or any reader that scans `document.cookie` in order —
        // can pick it over the live one. Evict it instead of leaving it to
        // expire on its own schedule.
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

/// Synchronizer-token CSRF guard, the second line behind [`csrf_origin_guard`].
///
/// On every state-changing method it requires the request to prove it holds the
/// session's token, taken from the `X-CSRF-Token` header or, failing that, the
/// `_csrf` urlencoded form field (the body is buffered and rebuilt so the
/// downstream handler still reads it). The expected token is re-derived from the
/// signed session cookie via [`verify_csrf`] — a MAC over a known input — so this
/// costs no database round trip and the readable CSRF cookie is never trusted as
/// the credential.
///
/// `multipart/form-data` bodies are passed through unread: the one multipart
/// route (OPML import) validates the field itself, since buffering and
/// re-streaming a file upload here would be wasteful. The Google Reader prefixes
/// are skipped entirely — they authenticate by bearer token, not a cookie.
pub async fn csrf_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if is_safe(req.method()) {
        return next.run(req).await;
    }
    if CSRF_SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    let secret = &state.config.secret;
    // An unsigned or tampered session cookie never resolves, so no submitted
    // token could match it.
    //
    // A request with *no* session cookie is passed through, not rejected. A CSRF
    // attack necessarily rides the victim's session cookie — the browser attaches
    // it automatically — so a cookie-less request cannot be a forged
    // authenticated action: it reaches a handler that will reject it on its own
    // `AuthUser` check. Login-CSRF, the one cookie-less case worth guarding, is
    // already stopped by `csrf_origin_guard`. In the browser, `anonymous_session`
    // means a page-driven POST always has a cookie by the time it is submitted,
    // so this only ever relaxes direct, unauthenticated API calls.
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
/// hash, the same way the `rdrs::audit` events do — enough to see that one
/// browser is failing every unsafe request (the signature of a CSRF cookie that
/// has drifted out of step with its session), without putting a live session
/// token in the log.
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
    use axum::body::Body;

    fn req(method: Method, headers: &[(&str, &str)]) -> Request {
        let mut b = Request::builder().method(method).uri("/anything");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn safe_methods_are_never_cross_site_checked() {
        // Even an obviously cross-site GET passes — GET must not change state.
        let r = req(Method::GET, &[("sec-fetch-site", "cross-site")]);
        assert!(is_safe(r.method()));
    }

    #[test]
    fn sec_fetch_site_is_authoritative() {
        for allowed in ["same-origin", "same-site", "none", "SAME-ORIGIN"] {
            assert!(
                !is_cross_site(&req(Method::POST, &[("sec-fetch-site", allowed)])),
                "{allowed} must be allowed"
            );
        }
        assert!(is_cross_site(&req(
            Method::POST,
            &[("sec-fetch-site", "cross-site")]
        )));
        // It wins over a same-looking Origin/Host, in both directions.
        assert!(is_cross_site(&req(
            Method::POST,
            &[
                ("sec-fetch-site", "cross-site"),
                ("origin", "https://app.example.com"),
                ("host", "app.example.com"),
            ]
        )));
    }

    #[test]
    fn origin_fallback_compares_host_ignoring_scheme_and_port() {
        // TLS-terminating proxy: Origin is https://, Host has no scheme/port.
        assert!(!is_cross_site(&req(
            Method::POST,
            &[
                ("origin", "https://app.example.com"),
                ("host", "app.example.com"),
            ]
        )));
        // Port on the Origin, none on Host → still same host.
        assert!(!is_cross_site(&req(
            Method::POST,
            &[("origin", "http://localhost:8080"), ("host", "localhost"),]
        )));
        // Genuine cross-origin.
        assert!(is_cross_site(&req(
            Method::POST,
            &[
                ("origin", "https://evil.example.com"),
                ("host", "app.example.com"),
            ]
        )));
        // Opaque origin.
        assert!(is_cross_site(&req(
            Method::POST,
            &[("origin", "null"), ("host", "app.example.com")]
        )));
    }

    #[test]
    fn ipv6_literal_host_is_compared_without_its_port() {
        assert!(!is_cross_site(&req(
            Method::POST,
            &[("origin", "http://[::1]:8080"), ("host", "[::1]")]
        )));
    }

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

    #[test]
    fn non_browser_client_without_headers_passes() {
        // A native GReader client / curl sends neither header and authenticates
        // by bearer token, so it is not a CSRF vector.
        assert!(!is_cross_site(&req(Method::POST, &[])));
    }
}
