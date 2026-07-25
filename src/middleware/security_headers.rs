//! `Strict-Transport-Security` (HSTS) — OWASP Session Management Cheat
//! Sheet, *Transport Layer Security*: once a browser has been told a host is
//! HTTPS-only, it refuses plain HTTP to that host for the whole `max-age`,
//! closing the downgrade channel that would otherwise put a session cookie on
//! the wire in cleartext (e.g. via a stripped link or a captive-portal
//! redirect).
//!
//! Unlike [`super::cache_control::no_store_for_authenticated`], this
//! middleware is **not** conditional on the request at all: HSTS is a
//! declaration about the *host*, not about any one response, so it goes on
//! every response — `/static`, `/health`, favicons included. There is no skip
//! list, and there must not be one: a browser that never saw the header on
//! `/health` would have no reason to upgrade a plain-HTTP request to it.
//!
//! **Whether the layer exists at all is decided once**, in
//! [`crate::create_router`], from [`crate::Config::hsts_header_value`] — a
//! plain-HTTP deployment (the default; see that method and
//! [`crate::config::parse_hsts`]) adds no layer and pays literally nothing
//! per request. When it does exist, the header value is a [`HeaderValue`]
//! built once at router-construction time and cloned per response, so the
//! allocation in `Config::hsts_header_value` (which returns a `String`) never
//! runs on the hot path.
//!
//! If a response **already carries** `Strict-Transport-Security` — most
//! likely because a TLS-terminating reverse proxy added it — this layer
//! leaves it alone rather than overwriting it.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// Per-layer state for [`set_hsts`]: just the precomputed header value. A
/// dedicated state type (rather than a field on [`crate::AppState`]) keeps
/// this middleware self-contained — it needs nothing else, and every other
/// consumer of `AppState` would otherwise gain a field it never reads.
#[derive(Clone)]
pub struct HstsState(HeaderValue);

impl HstsState {
    pub fn new(value: HeaderValue) -> Self {
        Self(value)
    }
}

/// Add `Strict-Transport-Security: <value>` to every response, unless one is
/// already present.
pub async fn set_hsts(
    State(HstsState(value)): State<HstsState>,
    req: Request,
    next: Next,
) -> Response {
    let response = next.run(req).await;
    apply(&value, response)
}

/// The actual header mutation, factored out of the async middleware body so
/// it can be unit tested directly against a hand-built [`Response`] instead
/// of a real [`Next`]. `HeaderMap::entry` is what gives us "do not overwrite"
/// for free: `or_insert` only runs when the header is absent.
fn apply(value: &HeaderValue, mut response: Response) -> Response {
    response
        .headers_mut()
        .entry(header::STRICT_TRANSPORT_SECURITY)
        .or_insert_with(|| value.clone());
    response
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

    #[test]
    fn sets_the_header_when_absent() {
        let value = HeaderValue::from_static("max-age=31536000; includeSubDomains");
        let response = apply(&value, plain_response());

        assert_eq!(
            response
                .headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .unwrap(),
            "max-age=31536000; includeSubDomains"
        );
    }

    #[test]
    fn does_not_overwrite_an_existing_header() {
        // A TLS-terminating reverse proxy may already have added its own
        // declaration; ours must not clobber it.
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::STRICT_TRANSPORT_SECURITY, "max-age=1")
            .body(Body::empty())
            .unwrap();

        let value = HeaderValue::from_static("max-age=31536000; includeSubDomains");
        let response = apply(&value, response);

        assert_eq!(
            response
                .headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .unwrap(),
            "max-age=1"
        );
    }
}
