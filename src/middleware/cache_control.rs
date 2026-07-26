//! Stop a browser disk cache or a shared/intermediate proxy from retaining a
//! session-bearing response — OWASP Session Management Cheat Sheet, *Web
//! Content Caching*: "the previous user's inbox" must not be able to reappear
//! via the back button or a cache hit on a shared machine.
//!
//! [`no_store_for_authenticated`] is response-oriented rather than a path
//! allowlist: it only fills in `Cache-Control` when the response has none, so
//! it never fights the three places in this codebase that set one on purpose
//! (all of which must keep working unchanged) — `handlers/static_assets.rs`'s
//! long-lived immutable/`no-cache` asset headers, `handlers/feed.rs`'s
//! `public, max-age=86400`, and `handlers/proxy.rs`'s image cache headers,
//! including its pass-through of whatever `Cache-Control` the upstream image
//! server sent. It also does nothing to a request that carries no session
//! cookie at all, so `/static`, `/health`, favicons and anonymous
//! image-proxy fetches stay cacheable.
//!
//! **Header value:** `no-store` alone. `no-store` already subsumes
//! `no-cache` and `max-age=0` (a cache honouring `no-store` never stores the
//! response in the first place, so those two directives would add nothing);
//! do not "complete the OWASP list" by appending them. `Pragma: no-cache` is
//! also deliberately omitted — it is a fossil that only ever meant anything
//! to an HTTP/1.0 cache, and rdrs is served over HTTP/1.1+ by axum/hyper.
//!
//! **`Vary: Cookie`** is set alongside `no-store`, appended to any existing
//! `Vary` rather than overwriting it. Without it, a shared proxy that (against
//! the `no-store` instruction, or upstream of a cache that respects it) keys
//! a cached variant by URL alone could conflate the with-cookie and
//! without-cookie responses for the same path.
//!
//! Two consequences worth calling out, the second one especially:
//!
//! - [`anonymous_session`](super::csrf::anonymous_session) mints a DB-less
//!   signed session cookie for every logged-out HTML page, so anonymous HTML
//!   gets `no-store` too. That's fine — a login form shouldn't be cached
//!   either — but it means this middleware is effectively always on for HTML.
//! - **`no-store` makes `ETag` useless for authenticated responses.** A
//!   browser keeps no copy of a `no-store` response, so it never has anything
//!   to send `If-None-Match` against next time, and `ETagLayer`'s hashing work
//!   becomes pure cost for those responses. This is a deliberate trade: rdrs
//!   is SSR-first with small pages, and `services/page_cache.rs` /
//!   `sidebar_cache.rs` already absorb the repeated *server-side* work, so the
//!   only thing given up is a few KB of conditional-request savings on the
//!   wire — far cheaper than a logged-in page leaking on a shared machine.
//!   Do not read the resulting dead `ETag` header as a bug to clean up.
//!
//! This middleware is layered *inside* `ETagLayer` deliberately, so it
//! observes the handler's own `Cache-Control` (if any) before `ETag`
//! processing runs. One consequence: it never sees a response returned by
//! one of the outer layers that short-circuits without calling `next` — the
//! `forward_auth` redirects or the CSRF-guard 403s — nor `/events`, which
//! sits outside this stack entirely. That's acceptable here: those are
//! 302/403 responses, and neither is heuristically cacheable without
//! explicit freshness information, so there is nothing for this middleware
//! to correct.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::CookieJar;

use crate::middleware::{SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST};

/// Fill in `Cache-Control: no-store` (+ `Vary: Cookie`) on a response to a
/// request carrying a session cookie, unless the handler already set its own
/// `Cache-Control` — see the module docs for the full rationale.
///
/// Cookie detection is a **name-presence check only**: it does not verify the
/// HMAC signature or touch the database. This middleware runs on every
/// request, so its cost must be near zero; the cost of a false positive here
/// is merely one extra `no-store` on a response that turns out to belong to
/// an expired or tampered cookie, which is harmless.
pub async fn no_store_for_authenticated(req: Request, next: Next) -> Response {
    let session_cookie_present = has_session_cookie(req.headers());
    let response = next.run(req).await;
    apply(session_cookie_present, response)
}

/// Whether `headers` carries a `Cookie` entry under either session cookie
/// name — the unprefixed [`SESSION_COOKIE_NAME`] or the `__Host-`-prefixed
/// [`SESSION_COOKIE_NAME_HOST`] introduced alongside `Secure` deployments.
/// Both must be checked: which name is in play depends on the deployment's
/// `cookie_secure` setting, and this middleware has no access to that
/// decision (nor does it need it — presence under either name is enough to
/// mark the response `no-store`).
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

/// `Cookie` merged into whatever `Vary` the response already carries, or
/// `None` when the response should be left exactly as it is.
///
/// Three things this must not do, each of which the obvious one-line
/// `format!("{existing}, Cookie")` gets wrong:
///
/// * **Drop sibling headers.** A response may carry several `Vary` lines;
///   HTTP treats them as one comma-separated list. Reading only the first
///   (`HeaderMap::get`) and then `insert`ing the result replaces *all* of
///   them, silently losing every line but the first. Hence `get_all` at the
///   call site and the join here.
/// * **Repeat itself.** A handler that already set `Vary: Cookie` would
///   otherwise get `Vary: Cookie, Cookie`. Harmless to a cache, which reads
///   `Vary` as a set, but it is noise in every response we touch.
/// * **Narrow `*`.** `Vary: *` already means "never serve this from a shared
///   cache without revalidating"; rewriting it to `*, Cookie` says nothing
///   new, so it is left alone.
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
