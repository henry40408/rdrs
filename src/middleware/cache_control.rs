//! `Cache-Control: no-store` for session-bearing responses (OWASP *Web Content
//! Caching*): a previous user's pages must not reappear via back button or cache.
//!
//! Only fills in `Cache-Control` when absent, so static assets, feed icons and
//! the image proxy keep their own; requests without a session cookie are left
//! cacheable. `no-store` alone subsumes `no-cache`/`max-age=0` — don't append them.
//! `Vary: Cookie` is merged into any existing `Vary`.
//!
//! - Anonymous HTML also gets `no-store`, since
//!   [`anonymous_session`](super::csrf::anonymous_session) sets a cookie there.
//! - **`no-store` makes `ETag` useless for authenticated responses.** Deliberate
//!   trade-off; the dead `ETag` is not a bug to clean up.
//!
//! Layered inside `ETagLayer`, so it misses outer short-circuit responses and
//! `/events` — fine, those are 302/403s without freshness info.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::CookieJar;

use crate::middleware::{SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST};

/// Set `no-store` + `Vary: Cookie` when a session cookie is present and the
/// handler set no `Cache-Control`. Cookie detection is name-presence only, to
/// stay near-free; a false positive just costs one extra `no-store`.
pub async fn no_store_for_authenticated(req: Request, next: Next) -> Response {
    let session_cookie_present = has_session_cookie(req.headers());
    let response = next.run(req).await;
    apply(session_cookie_present, response)
}

/// Either session cookie name counts; which one is used depends on `cookie_secure`.
fn has_session_cookie(headers: &HeaderMap) -> bool {
    let jar = CookieJar::from_headers(headers);
    jar.get(SESSION_COOKIE_NAME).is_some() || jar.get(SESSION_COOKIE_NAME_HOST).is_some()
}

/// Header mutation, split out for unit tests.
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

/// `Cookie` merged into all existing `Vary` lines (one HTTP list), or `None` to
/// leave the response as is (already covers `Cookie`, is `*`, or unreadable).
fn merged_vary(existing: &[HeaderValue]) -> Option<HeaderValue> {
    if existing.is_empty() {
        return Some(HeaderValue::from_static("Cookie"));
    }

    let mut parts: Vec<&str> = Vec::new();
    for value in existing {
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
        // Deliberate public-caching directives must survive.
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
        // Clobbering compression's Vary would let caches serve gzip wrongly.
        let response = response_with_header(header::VARY, "Accept-Encoding");

        let response = apply(true, response);

        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding, Cookie"
        );
    }

    #[test]
    fn leaves_a_vary_that_already_covers_cookie_alone() {
        for vary in [
            "Accept-Encoding, Cookie",
            // Case-insensitive.
            "cookie",
            "*",
        ] {
            let response = apply(true, response_with_header(header::VARY, vary));
            assert_eq!(response.headers().get(header::VARY).unwrap(), vary);
        }
    }

    #[test]
    fn merges_multiple_vary_header_lines() {
        // Regression: `get` + `insert` dropped all but the first `Vary` line.
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
        // Missing the __Host- name would leave Secure deployments cacheable.
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
