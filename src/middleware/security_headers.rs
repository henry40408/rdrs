//! Response security headers, in two layers.
//!
//! [`set_security_headers`] carries the fixed set — CSP, `X-Content-Type-Options`,
//! `Referrer-Policy`, `Permissions-Policy`, `X-Frame-Options`,
//! `Cross-Origin-Opener-Policy` — and is always installed. [`set_hsts`] carries
//! `Strict-Transport-Security` alone, because that one is conditional on the
//! deployment being HTTPS.
//!
//! Both share the same two rules, for the same reasons:
//!
//! - **No skip list, and there must not be one.** These are declarations about
//!   the *host* and its resources, not about any one response, so they go on
//!   every response — `/static`, `/health`, favicons, the image proxy included.
//!   `nosniff` on a proxied image is exactly where it earns its keep, and a
//!   browser that never saw HSTS on `/health` would have no reason to upgrade a
//!   plain-HTTP request to it.
//! - **Applied outermost** in [`crate::create_router`], around the whole router
//!   and over both `core` and `/events`, rather than alongside the other
//!   response layers. `forward_auth` and the CSRF guards return a response
//!   without calling `next` on several paths (a redirect, a rejection); nested
//!   any further in, these would silently miss those responses.
//!
//! If a response **already carries** one of these headers — most likely because
//! a reverse proxy added it — that value is left alone rather than overwritten.
//!
//! ## The Content-Security-Policy
//!
//! `script-src 'self'` with no `'unsafe-inline'` is the point of the whole
//! policy, and it is only enforceable because no template ships an inline
//! `<script>` or an `on*=` handler attribute any more: the three inline blocks
//! became `static/js/{login,register,search}.js`, and every `onsubmit` /
//! `onclick` / `onchange` / `onerror` became a `data-` attribute driven by a
//! delegated listener in `static/js/behaviors.js` (banner dismiss in
//! `components/rdrs-flash.js`). Reintroducing either would not fail a build —
//! it would silently stop working in the browser, so don't.
//!
//! `style-src` keeps `'unsafe-inline'` on purpose. Two `style` attributes are
//! load-bearing and cannot become classes: the per-datum bar geometry on
//! /statistics (`height: {percent}%` over an `f64`, so not a finite class set)
//! and the sprite-hiding rule in `_icon_sprite.html` (the UA stylesheet's
//! `[hidden]` rule is XHTML-namespaced and never matches an SVG element — see
//! that file's comment). The concession is narrow: with `default-src 'self'`
//! and `img-src 'self' data:` there is no external origin for injected CSS to
//! exfiltrate to.
//!
//! `img-src 'self' data:` covers the three real sources — same-origin feed
//! icons, remote article images rewritten to the same-origin `/api/proxy/image`
//! (see [`crate::services::image_proxy`]), and the `data:` SVG chevron that
//! `app.css` uses as a `<select>` background. It assumes `RDRS_PUBLIC_BASE_URL`
//! names the origin the browser actually uses, since that is what the reading
//! pane stamps into proxy URLs — already a hard requirement for the session
//! cookie's `Secure` flag and for HSTS.
//!
//! `frame-ancestors 'none'` is the modern half of the clickjacking defence and
//! `X-Frame-Options: DENY` the legacy half; both are sent because the older
//! header is the only one pre-CSP3 browsers honour.
//!
//! ## What is deliberately absent
//!
//! **`Cross-Origin-Resource-Policy`.** `same-origin` would break third-party
//! Google Reader clients: the `GReader` item feed hands out absolute
//! `/api/proxy/image` URLs (see [`crate::handlers::greader`]), and a native
//! client rendering that HTML in a webview fetches them as cross-origin
//! no-cors requests, which CORP would reject. Article images would silently
//! vanish in every such client.
//!
//! **`publickey-credentials-get` / `-create` in `Permissions-Policy`.** A
//! `Permissions-Policy` header only overrides the features it names; anything
//! unlisted keeps its default allowlist, which for both of these is `self`.
//! Naming them to deny would break passkey sign-in and enrolment.

use std::sync::LazyLock;

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// See the module docs for why each directive reads the way it does.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; \
     font-src 'self'; \
     connect-src 'self'; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'";

/// Every feature the app has no use for, denied outright. Passkey features are
/// omitted on purpose so they keep their `self` default — see the module docs.
const PERMISSIONS_POLICY: &str = "accelerometer=(), \
     autoplay=(), \
     camera=(), \
     display-capture=(), \
     encrypted-media=(), \
     geolocation=(), \
     gyroscope=(), \
     magnetometer=(), \
     microphone=(), \
     midi=(), \
     payment=(), \
     usb=(), \
     xr-spatial-tracking=()";

/// `strict-origin-when-cross-origin` rather than `no-referrer`: the entry-action
/// redirect in [`crate::handlers::entries`] recovers which list the user came
/// from out of the same-origin `Referer`, and `no-referrer` would strip it and
/// send every action back to the default list. This value keeps the full URL
/// same-origin while cross-origin navigations leak only the bare origin — which
/// external article links already avoid entirely via `rel="noreferrer"`.
const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";

/// Built once. `HeaderName::from_static` is not a `const fn`, and the two
/// headers with no `http` constant would otherwise be re-parsed on every
/// response.
static STATIC_HEADERS: LazyLock<[(HeaderName, HeaderValue); 6]> = LazyLock::new(|| {
    [
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (
            header::REFERRER_POLICY,
            HeaderValue::from_static(REFERRER_POLICY),
        ),
        (
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(PERMISSIONS_POLICY),
        ),
        (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
        (
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ),
    ]
});

/// Add the fixed security headers to every response, leaving any the response
/// already carries untouched.
pub async fn set_security_headers(req: Request, next: Next) -> Response {
    let response = next.run(req).await;
    apply_static(response)
}

/// The header mutation for [`set_security_headers`], factored out of the async
/// body so it can be unit tested against a hand-built [`Response`] instead of a
/// real [`Next`].
fn apply_static(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in STATIC_HEADERS.iter() {
        headers.entry(name.clone()).or_insert_with(|| value.clone());
    }
    response
}

/// Per-layer state for [`set_hsts`]: just the precomputed header value. A
/// dedicated state type (rather than a field on [`crate::AppState`]) keeps
/// this middleware self-contained — it needs nothing else, and every other
/// consumer of `AppState` would otherwise gain a field it never reads.
///
/// **Whether the layer exists at all is decided once**, in
/// [`crate::create_router`], from [`crate::Config::hsts_header_value`] — a
/// plain-HTTP deployment (the default; see that method and
/// [`crate::config::parse_hsts`]) adds no layer and pays literally nothing
/// per request. When it does exist, the header value is a [`HeaderValue`]
/// built once at router-construction time and cloned per response, so the
/// allocation in `Config::hsts_header_value` (which returns a `String`) never
/// runs on the hot path.
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

    #[test]
    fn sets_every_static_header() {
        let response = apply_static(plain_response());
        let headers = response.headers();

        assert_eq!(
            headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            CONTENT_SECURITY_POLICY
        );
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert_eq!(
            headers.get(header::REFERRER_POLICY).unwrap(),
            REFERRER_POLICY
        );
        assert_eq!(
            headers.get("permissions-policy").unwrap(),
            PERMISSIONS_POLICY
        );
        assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(
            headers.get("cross-origin-opener-policy").unwrap(),
            "same-origin"
        );
    }

    /// The whole point of the policy: an injected `<script>` must have no way
    /// to run. A stray `'unsafe-inline'` in `script-src` would silently undo
    /// the template refactor that made the strict directive possible.
    #[test]
    fn script_src_is_strict() {
        assert!(CONTENT_SECURITY_POLICY.contains("script-src 'self';"));
        assert!(
            !CONTENT_SECURITY_POLICY.contains("script-src 'self' 'unsafe-inline'"),
            "script-src must not allow inline scripts"
        );
        // `'unsafe-eval'` has no legitimate use here either.
        assert!(!CONTENT_SECURITY_POLICY.contains("unsafe-eval"));
    }

    /// Passkey sign-in and enrolment rely on these two features keeping their
    /// `self` default, which only holds while the header stays silent on them.
    #[test]
    fn permissions_policy_leaves_webauthn_alone() {
        assert!(!PERMISSIONS_POLICY.contains("publickey-credentials"));
    }

    /// Collect every inline `on*="…"` handler attribute in a template.
    ///
    /// Matches generically rather than against a fixed list of event names, so
    /// an `onpointerdown` nobody thought of is caught too. A prose word that
    /// merely starts with "on" is excluded by requiring an `=` immediately
    /// after the run of lowercase letters.
    fn inline_handler_attributes(html: &str) -> Vec<String> {
        let mut found = Vec::new();
        for (i, _) in html.match_indices("on") {
            if i == 0 {
                continue;
            }
            if !html[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
            {
                continue;
            }
            let rest = &html[i..];
            let name: String = rest.chars().take_while(char::is_ascii_lowercase).collect();
            if name.len() > 2 && rest[name.len()..].starts_with('=') {
                found.push(name);
            }
        }
        found
    }

    /// True for a `<script …>` open tag that carries executable inline code —
    /// i.e. neither an external `src` nor a non-executing data block such as
    /// `type="application/json"` (which CSP's `script-src` does not police,
    /// since the browser never runs it).
    fn is_inline_script_tag(tag: &str) -> bool {
        !tag.contains("src=") && !tag.contains("application/json")
    }

    /// `script-src 'self'` is only a real defence while the templates hold up
    /// their end: an inline `<script>` or an `on*=` attribute is not a build
    /// error, it just silently stops working in the browser. This walks every
    /// template and fails on either, so the CSP and the markup cannot drift.
    #[test]
    fn no_template_ships_inline_script_or_handler_attributes() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/templates");
        let mut stack = vec![std::path::PathBuf::from(root)];
        let mut scanned = 0usize;

        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let html = std::fs::read_to_string(&path).unwrap();
                scanned += 1;

                let handlers = inline_handler_attributes(&html);
                assert!(
                    handlers.is_empty(),
                    "{}: inline handler attribute(s) {handlers:?} — CSP blocks these. \
                     Use a `data-` attribute plus a delegated listener in \
                     static/js/behaviors.js instead.",
                    path.display()
                );

                for (i, _) in html.match_indices("<script") {
                    let tag = &html[i..];
                    let end = tag.find('>').unwrap_or(tag.len());
                    assert!(
                        !is_inline_script_tag(&tag[..end]),
                        "{}: inline <script> block — CSP blocks these. Move it to a \
                         module under static/js/ and reference it with `src`.",
                        path.display()
                    );
                }
            }
        }

        assert!(scanned > 10, "sanity: expected to scan the template tree");
    }

    #[test]
    fn does_not_overwrite_existing_static_headers() {
        // A reverse proxy that already set its own policy wins, exactly as
        // with HSTS above.
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
            .body(Body::empty())
            .unwrap();

        let response = apply_static(response);

        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap(),
            "default-src 'none'"
        );
        // The ones the proxy did *not* set are still added.
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
    }
}
