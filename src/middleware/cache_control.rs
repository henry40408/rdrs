//! Stop a browser disk cache or a shared proxy from retaining a session-bearing
//! response — OWASP's *Web Content Caching*: "the previous user's inbox" must not
//! reappear via the back button or a cache hit on a shared machine.
//!
//! [`no_store_for_authenticated`] is response-oriented rather than a path
//! allowlist: it only fills in `Cache-Control` when the response has none, so it
//! never fights the three places that set one on purpose — the immutable static
//! assets, the feed icon's `public, max-age=86400`, and the image proxy's
//! headers including its upstream pass-through. It also does nothing to a
//! request carrying no session cookie, so `/static`, `/health`, favicons and
//! anonymous proxy fetches stay cacheable.
//!
//! **Header value:** `no-store` alone. It already subsumes `no-cache` and
//! `max-age=0`, so do not "complete the OWASP list" by appending them.
//! `Pragma: no-cache` is a fossil that only ever meant anything to an HTTP/1.0
//! cache.
//!
//! **`Vary: Cookie`** is appended to any existing `Vary` rather than overwriting
//! it. Without it, a shared proxy keying a cached variant by URL alone could
//! conflate the with-cookie and without-cookie responses for one path.
//!
//! Two consequences worth calling out, the second especially:
//!
//! - [`anonymous_session`](super::csrf::anonymous_session) mints a DB-less
//!   signed cookie for every logged-out HTML page, so anonymous HTML gets
//!   `no-store` too — fine for a login form, but it means this is effectively
//!   always on for HTML.
//! - **`no-store` makes `ETag` useless for authenticated responses.** A browser
//!   keeps no copy, so it never has anything to send `If-None-Match` against,
//!   and `ETagLayer`'s hashing becomes pure cost. A deliberate trade: rdrs is
//!   SSR-first with small pages and the server-side caches already absorb the
//!   repeated work, so the only loss is a few KB of conditional-request savings.
//!   Do not read the resulting dead `ETag` header as a bug to clean up.
//!
//! Layered *inside* `ETagLayer`, so it observes the handler's own
//! `Cache-Control` before `ETag` processing runs. It therefore never sees a
//! response returned by an outer layer that short-circuits without calling
//! `next`, nor `/events` — acceptable, since those are 302/403 responses and
//! neither is heuristically cacheable without explicit freshness information.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::CookieJar;

use crate::middleware::{SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST};

/// Fill in `Cache-Control: no-store` (+ `Vary: Cookie`) on a response to a
/// request carrying a session cookie, unless the handler already set its own —
/// see the module docs for the full rationale.
///
/// Cookie detection is a **name-presence check only**: no signature check, no
/// database. This runs on every request, so its cost must be near zero, and a
/// false positive is merely one extra `no-store` on a response belonging to an
/// expired or tampered cookie.
pub async fn no_store_for_authenticated(req: Request, next: Next) -> Response {
    let session_cookie_present = has_session_cookie(req.headers());
    let response = next.run(req).await;
    apply(session_cookie_present, response)
}

/// Whether `headers` carries a `Cookie` entry under either session cookie name.
/// Both must be checked: which one is in play depends on the deployment's
/// `cookie_secure` setting, which this middleware has no access to — nor needs,
/// since presence under either name is enough to mark the response `no-store`.
fn has_session_cookie(headers: &HeaderMap) -> bool {
    let jar = CookieJar::from_headers(headers);
    jar.get(SESSION_COOKIE_NAME).is_some() || jar.get(SESSION_COOKIE_NAME_HOST).is_some()
}

/// The actual header mutation, factored out of the async middleware body so
/// it can be unit tested directly against a hand-built [`Response`] instead
/// of a real [`Next`].
fn apply(session_cookie_present: bool, mut response: Response) -> Response {
    if !session_cookie_present || response.headers().contains_key(header::CACHE_CONTROL) {
        return response;
    }

    let existing_vary: Vec<HeaderValue> = response
        .headers()
        .get_all(header::VARY)
        .iter()
        .cloned()
        .collect();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(merged) = merged_vary(&existing_vary) {
        headers.insert(header::VARY, merged);
    }
    response
}

/// `Cookie` merged into whatever `Vary` the response already carries, or `None`
/// when the response should be left exactly as it is.
///
/// Three things the obvious `format!("{existing}, Cookie")` gets wrong:
///
/// * **Dropping sibling headers.** A response may carry several `Vary` lines,
///   which HTTP treats as one list. Reading only the first and then `insert`ing
///   replaces *all* of them.
/// * **Repeating itself.** A handler that already set `Vary: Cookie` would get
///   `Vary: Cookie, Cookie` — harmless to a cache, but noise.
/// * **Narrowing `*`.** `Vary: *` already means "never serve this from a shared
///   cache without revalidating", so it is left alone.
fn merged_vary(existing: &[HeaderValue]) -> Option<HeaderValue> {
    if existing.is_empty() {
        return Some(HeaderValue::from_static("Cookie"));
    }

    let mut parts: Vec<&str> = Vec::new();
    for value in existing {
        // A `Vary` we cannot even read is not one we can safely rewrite;
        // leaving it intact beats replacing it with a guess.
        let value = value.to_str().ok()?;
        parts.extend(value.split(',').map(str::trim).filter(|p| !p.is_empty()));
    }

    if parts
        .iter()
        .any(|p| *p == "*" || p.eq_ignore_ascii_case("cookie"))
    {
        return None;
    }

    parts.push("Cookie");
    HeaderValue::from_str(&parts.join(", ")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;

    fn plain_response() -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap()
    }

    fn response_with_header(name: header::HeaderName, value: &str) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header(name, value)
            .body(Body::empty())
            .unwrap()
    }

    fn headers_with_cookie(cookie_header: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie_header).unwrap(),
        );
        headers
    }

    #[test]
    fn skips_when_response_already_has_cache_control() {
        // The three deliberate public-caching call sites (static assets, the
        // feed endpoint, the image proxy — including its upstream
        // pass-through) all set their own directive; it must survive
        // untouched even though the request carries a session cookie.
        let response = response_with_header(header::CACHE_CONTROL, "public, max-age=86400");

        let response = apply(true, response);

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=86400"
        );
        assert!(
            response.headers().get(header::VARY).is_none(),
            "must not add Vary when Cache-Control is left alone"
        );
    }

    #[test]
    fn skips_when_request_has_no_session_cookie() {
        // /static, /health, favicons, anonymous image-proxy fetches: no
        // cookie means no reason to defeat their heuristic cacheability.
        let response = apply(false, plain_response());

        assert!(response.headers().get(header::CACHE_CONTROL).is_none());
        assert!(response.headers().get(header::VARY).is_none());
    }

    #[test]
    fn sets_no_store_and_vary_cookie_when_session_cookie_present() {
        let response = apply(true, plain_response());

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(response.headers().get(header::VARY).unwrap(), "Cookie");
    }

    #[test]
    fn appends_to_existing_vary() {
        // Compression sets `Vary: Accept-Encoding`; ours must join it, not
        // clobber it — otherwise a shared cache could serve a gzip response
        // to a client that only accepts identity encoding.
        let response = response_with_header(header::VARY, "Accept-Encoding");

        let response = apply(true, response);

        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding, Cookie"
        );
    }

    #[test]
    fn does_not_repeat_an_existing_cookie_vary() {
        let response = response_with_header(header::VARY, "Accept-Encoding, Cookie");

        let response = apply(true, response);

        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding, Cookie",
            "Cookie is already covered; appending it again is pure noise"
        );
    }

    #[test]
    fn matches_an_existing_cookie_vary_case_insensitively() {
        // Field values are compared case-insensitively by caches, so `cookie`
        // must count as already covered.
        let response = response_with_header(header::VARY, "cookie");

        let response = apply(true, response);

        assert_eq!(response.headers().get(header::VARY).unwrap(), "cookie");
    }

    #[test]
    fn leaves_a_wildcard_vary_alone() {
        // `Vary: *` is already the strongest possible statement; narrowing it
        // to `*, Cookie` adds nothing.
        let response = response_with_header(header::VARY, "*");

        let response = apply(true, response);

        assert_eq!(response.headers().get(header::VARY).unwrap(), "*");
    }

    #[test]
    fn merges_multiple_vary_header_lines() {
        // Regression: reading only the first `Vary` with `HeaderMap::get` and
        // then `insert`ing the result replaced *all* of them, so a response
        // carrying two `Vary` lines silently lost one — a shared cache would
        // then key on the wrong set of request headers.
        let mut response = plain_response();
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Accept-Language"));

        let response = apply(true, response);

        let values: Vec<_> = response
            .headers()
            .get_all(header::VARY)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(values, vec!["Accept-Encoding, Accept-Language, Cookie"]);
    }

    #[test]
    fn detects_the_host_prefixed_cookie_name() {
        // a33d1ee introduced the __Host- prefix for Secure deployments, so a
        // browser holding that cookie must trigger no-store exactly like the
        // unprefixed name does — missing this case would silently leave
        // Secure-deployment sessions cacheable.
        assert!(has_session_cookie(&headers_with_cookie(
            "__Host-session_token=abc123"
        )));
        assert!(has_session_cookie(&headers_with_cookie(
            "session_token=abc123"
        )));
        assert!(!has_session_cookie(&headers_with_cookie(
            "unrelated=abc123"
        )));
        assert!(!has_session_cookie(&HeaderMap::new()));
    }
}
