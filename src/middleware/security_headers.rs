//! Response security headers: [`set_security_headers`] (CSP and friends, always
//! installed) and [`set_hsts`] (only on HTTPS deployments).
//!
//! - **No skip list, and there must not be one.** These describe the host, so
//!   every response gets them — `/static`, `/health` and the image proxy included.
//! - **Applied outermost** in [`crate::create_router`], because `forward_auth` and
//!   the CSRF guards return early without calling `next`.
//! - A header already present (e.g. from a reverse proxy) is left alone.
//!
//! ## The Content-Security-Policy
//!
//! `script-src 'self'` and `style-src 'self'` forbid inline `<script>`, `on*=`
//! handlers and `style` attributes in any markup, including HTML assigned to
//! `innerHTML`; violations fail silently in the browser, not the build.
//! Writing `element.style` from script is fine (CSP polices markup, not CSSOM).
//! `img-src 'self' data:` assumes `RDRS_PUBLIC_BASE_URL` is the browser-facing
//! origin. `frame-ancestors 'none'` plus `X-Frame-Options: DENY` covers pre-CSP3
//! browsers.
//!
//! ## Deliberately absent
//!
//! - `Cross-Origin-Resource-Policy`: `same-origin` would break proxied images in
//!   third-party Google Reader clients' webviews.
//! - `publickey-credentials-*` in `Permissions-Policy`: naming them would drop
//!   their `self` default and break passkeys.

use std::sync::LazyLock;

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     script-src 'self'; \
     style-src 'self'; \
     img-src 'self' data:; \
     font-src 'self'; \
     connect-src 'self'; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'";

/// Unused features, denied. Passkey features omitted on purpose (module docs).
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

/// Not `no-referrer`: entry-action redirects recover the originating list from
/// the same-origin `Referer`.
const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";

/// Built once so non-`http`-constant header names aren't re-parsed per response.
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

/// Add the fixed security headers, keeping any already present.
pub async fn set_security_headers(req: Request, next: Next) -> Response {
    let response = next.run(req).await;
    apply_static(response)
}

/// Header mutation for [`set_security_headers`], split out for unit tests.
fn apply_static(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in STATIC_HEADERS.iter() {
        headers.entry(name.clone()).or_insert_with(|| value.clone());
    }
    response
}

/// Precomputed HSTS value for [`set_hsts`]. The layer is only added (in
/// [`crate::create_router`]) when [`crate::Config::hsts_header_value`] is set.
#[derive(Clone)]
pub struct HstsState(HeaderValue);

impl HstsState {
    pub fn new(value: HeaderValue) -> Self {
        Self(value)
    }
}

/// Add `Strict-Transport-Security` unless already present.
pub async fn set_hsts(
    State(HstsState(value)): State<HstsState>,
    req: Request,
    next: Next,
) -> Response {
    let response = next.run(req).await;
    apply(&value, response)
}

/// Header mutation for [`set_hsts`], split out for unit tests.
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
        // A TLS-terminating proxy's own header must win.
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

    /// An injected `<script>` must have no way to run.
    #[test]
    fn script_src_is_strict() {
        assert!(CONTENT_SECURITY_POLICY.contains("script-src 'self';"));
        assert!(
            !CONTENT_SECURITY_POLICY.contains("script-src 'self' 'unsafe-inline'"),
            "script-src must not allow inline scripts"
        );
        assert!(!CONTENT_SECURITY_POLICY.contains("unsafe-eval"));
    }

    /// Passkeys need these features to keep their `self` default.
    #[test]
    fn permissions_policy_leaves_webauthn_alone() {
        assert!(!PERMISSIONS_POLICY.contains("publickey-credentials"));
    }

    /// Every inline `on*=` handler attribute in a template (any event name;
    /// requiring `=` excludes prose words starting with "on").
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

    /// True for a `<script>` tag with executable inline code (no `src`, not a
    /// JSON data block).
    fn is_inline_script_tag(tag: &str) -> bool {
        !tag.contains("src=") && !tag.contains("application/json")
    }

    /// Every file under `dir`, recursively, whose extension is in `extensions`.
    fn source_files(dir: &str, extensions: &[&str]) -> Vec<std::path::PathBuf> {
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.contains(&e))
                {
                    files.push(path);
                }
            }
        }
        files
    }

    /// Keeps templates and JS-built markup in line with the strict CSP, whose
    /// violations fail silently. `element.style` (CSSOM) stays allowed, hence
    /// matching `style="`.
    #[test]
    fn no_markup_ships_inline_script_handler_or_style() {
        let templates = source_files(concat!(env!("CARGO_MANIFEST_DIR"), "/templates"), &["html"]);
        let scripts = source_files(concat!(env!("CARGO_MANIFEST_DIR"), "/static/js"), &["js"]);
        let scanned = templates.len() + scripts.len();

        for path in templates.iter().chain(scripts.iter()) {
            let source = std::fs::read_to_string(path).unwrap();

            assert!(
                !source.contains("style=\""),
                "{}: inline style attribute — `style-src 'self'` blocks these. Use a \
                 class in static/css/app.css, the `hidden` attribute, or assign to \
                 `element.style` from script (the CSSOM is not policed).",
                path.display()
            );

            assert!(
                !source.contains("<style"),
                "{}: inline <style> element — `style-src 'self'` blocks these, even \
                 inside a shadow root. Adopt a constructable stylesheet instead, as \
                 components/rdrs-kb-help.js does.",
                path.display()
            );

            // The rest only applies to markup; .js files are external modules.
            if path.extension().and_then(|e| e.to_str()) != Some("html") {
                continue;
            }

            let handlers = inline_handler_attributes(&source);
            assert!(
                handlers.is_empty(),
                "{}: inline handler attribute(s) {handlers:?} — CSP blocks these. \
                 Use a `data-` attribute plus a delegated listener in \
                 static/js/behaviors.js instead.",
                path.display()
            );

            for (i, _) in source.match_indices("<script") {
                let tag = &source[i..];
                let end = tag.find('>').unwrap_or(tag.len());
                assert!(
                    !is_inline_script_tag(&tag[..end]),
                    "{}: inline <script> block — CSP blocks these. Move it to a \
                     module under static/js/ and reference it with `src`.",
                    path.display()
                );
            }
        }

        assert!(scanned > 10, "sanity: expected to scan the source tree");
    }

    #[test]
    fn does_not_overwrite_existing_static_headers() {
        // A reverse proxy's own policy wins.
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
        // Headers the proxy didn't set are still added.
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
    }
}
