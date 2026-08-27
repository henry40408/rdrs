//! Response security headers, in two layers.
//!
//! [`set_security_headers`] carries the fixed set — CSP, `X-Content-Type-Options`,
//! `Referrer-Policy`, `Permissions-Policy`, `X-Frame-Options`,
//! `Cross-Origin-Opener-Policy` — and is always installed. [`set_hsts`] carries
//! `Strict-Transport-Security` alone, since that one is conditional on the
//! deployment being HTTPS.
//!
//! Both share the same two rules:
//!
//! - **No skip list, and there must not be one.** These are declarations about
//!   the *host*, not about any one response, so they go on every response —
//!   `/static`, `/health`, favicons and the image proxy included. `nosniff` on a
//!   proxied image is exactly where it earns its keep.
//! - **Applied outermost** in [`crate::create_router`], because `forward_auth`
//!   and the CSRF guards return a response without calling `next` on several
//!   paths; nested any further in, these would miss those responses.
//!
//! A header a response already carries — most likely from a reverse proxy — is
//! left alone rather than overwritten.
//!
//! ## The Content-Security-Policy
//!
//! `script-src 'self'` with no `'unsafe-inline'` is the point of the whole
//! policy, and it is only enforceable because no template ships an inline
//! `<script>` or an `on*=` handler any more: those became modules under
//! `static/js/` and `data-` attributes driven by delegated listeners.
//! Reintroducing either would not fail a build — it would silently stop working
//! in the browser.
//!
//! `style-src 'self'` is equally strict, so **no markup anywhere may carry a
//! `style` attribute** — not templates, and not HTML that JavaScript assigns to
//! `innerHTML`, which the parser checks the same way. Static declarations became
//! classes, `style="display:none"` became the `hidden` attribute, and the
//! per-datum bar geometry on /statistics became a `pct-N` class. Writing to
//! `element.style` *from script* is untouched, since CSP polices markup rather
//! than the CSSOM.
//!
//! `_icon_sprite.html` is the one place that needs collapsing without CSS — a
//! bare `<svg>` renders at 300x150 — and uses SVG presentation attributes, which
//! are not `style` attributes.
//!
//! `img-src 'self' data:` covers the three real sources: same-origin feed icons,
//! remote article images rewritten to the same-origin proxy, and the `data:` SVG
//! chevron `app.css` uses. It assumes `RDRS_PUBLIC_BASE_URL` names the origin the
//! browser actually uses, already a hard requirement for the cookie's `Secure`
//! flag and for HSTS.
//!
//! `frame-ancestors 'none'` and `X-Frame-Options: DENY` are the modern and
//! legacy halves of the clickjacking defence; both are sent because the older
//! header is the only one pre-CSP3 browsers honour.
//!
//! ## What is deliberately absent
//!
//! **`Cross-Origin-Resource-Policy`.** `same-origin` would break third-party
//! Google Reader clients: the item feed hands out absolute proxy URLs, and a
//! native client rendering that HTML in a webview fetches them as cross-origin
//! no-cors requests. Article images would silently vanish.
//!
//! **`publickey-credentials-get` / `-create` in `Permissions-Policy`.** The
//! header only overrides the features it names, and both default to `self`, so
//! naming them to deny would break passkey sign-in and enrolment.

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
     style-src 'self'; \
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
/// redirect recovers which list the reader came from out of the same-origin
/// `Referer`, and `no-referrer` would send every action back to the default
/// list. Cross-origin navigations still leak only the bare origin, and external
/// article links avoid it entirely via `rel="noreferrer"`.
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
/// dedicated state type keeps this middleware self-contained, rather than giving
/// every other consumer of [`crate::AppState`] a field it never reads.
///
/// **Whether the layer exists at all is decided once**, in
/// [`crate::create_router`], from [`crate::Config::hsts_header_value`]: a
/// plain-HTTP deployment adds no layer and pays nothing per request. When it
/// does exist the value is a [`HeaderValue`] built at router-construction time
/// and cloned per response, so the allocation never runs on the hot path.
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
    /// Matches generically rather than against a fixed list of event names, so an
    /// `onpointerdown` nobody thought of is caught too. A prose word merely
    /// starting with "on" is excluded by requiring an `=` right after.
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

    /// `script-src 'self'` and `style-src 'self'` are only a real defence while
    /// the markup holds up its end: an inline `<script>`, an `on*=` or a `style=`
    /// attribute is not a build error, it just silently stops working. This walks
    /// every template and every file that builds markup for `innerHTML`, so the
    /// policy and the markup cannot drift.
    ///
    /// Markup a script assigns to `innerHTML` is parsed and policed the same way,
    /// including inside a shadow root. Writing to `element.style` is a CSSOM
    /// operation and stays allowed, which is why this looks for `style="`.
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

            // The remaining two only apply to markup, and every .js file here is
            // an ES module served with `src`, not an inline block.
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
