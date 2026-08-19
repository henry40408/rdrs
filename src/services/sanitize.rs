use ammonia::Builder;
use lol_html::{RewriteStrSettings, element, rewrite_str};
use scraper::{Html, Selector};
use std::borrow::Cow;
use std::collections::HashSet;
use url::Url;

use super::image_proxy::{create_proxy_url, create_proxy_url_with_referrer};

/// Known tracking domains (subdomains that indicate tracking)
const TRACKING_DOMAINS: &[&str] = &["pixel.", "beacon.", "track.", "analytics."];

/// Known tracking URL paths
const TRACKING_PATHS: &[&str] = &["/pixel", "/beacon", "/track", "/1x1"];

/// Tracking query parameters that should be removed (exact match)
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "gclsrc",
    "srsltid",
    "yclid",
    "ysclid",
    "twclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "mc_tc",
    "_openstat",
    "fb_action_ids",
    "fb_action_types",
    "fb_ref",
    "fb_source",
    "fb_comment_id",
    "hmb_campaign",
    "hmb_medium",
    "hmb_source",
    "itm_campaign",
    "itm_medium",
    "itm_source",
    "campaign_id",
    "campaign_medium",
    "campaign_name",
    "campaign_source",
    "campaign_term",
    "campaign_content",
    "wickedid",
    "hsa_cam",
    "_hsenc",
    "__hssc",
    "__hstc",
    "__hsfp",
    "_hsmi",
    "hsctatracking",
    "rb_clickid",
    "oly_anon_id",
    "oly_enc_id",
    "vero_id",
    "vero_conv",
    "mkt_tok",
    "sc_cid",
    "_bhlid",
    "_branch_match_id",
    "_branch_referrer",
    "__readwiseLocation",
    "ref",
];

/// Tracking query parameter prefixes
const TRACKING_PARAM_PREFIXES: &[&str] = &["utm_", "mtm_"];

/// Matched case-insensitively, and by prefix for the `utm_`/`mtm_` families,
/// whose suffixes are open-ended.
fn is_tracking_param(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    TRACKING_PARAMS.iter().any(|&p| name_lower == p)
        || TRACKING_PARAM_PREFIXES
            .iter()
            .any(|p| name_lower.starts_with(p))
}

/// Attributes that carry the real image URL for lazy-loaded images, in priority order.
const LAZY_SRC_ATTRS: &[&str] = &["data-src", "data-lazy-src", "data-original"];

/// ASCII-case-insensitive substring test, for the pre-pass gates below.
///
/// They have to be case-insensitive: HTML tag and attribute names are, and so
/// are the parsers each gate stands in front of, so a feed that ships `<IMG
/// DATA-SRC=...>` must not slip past a lowercase-only check and silently lose
/// its pass. `needle` must already be lowercase.
///
/// Scanning a few KiB for a short needle costs microseconds against the
/// hundreds a parse costs, so this is worth paying on every document to skip
/// the parse on most of them.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    debug_assert!(needle.bytes().all(|b| !b.is_ascii_uppercase()));
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    // `windows(0)` panics, so the empty needle is answered before it is reached
    // — every caller passes a literal, but a panic is not a thing to leave lying
    // in a function this hot.
    if n.is_empty() {
        return true;
    }
    h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Parse a `width:NNpx` / `height:NNpx` integer out of an inline `style`.
fn style_dim(style: &str, prop: &str) -> Option<String> {
    for decl in style.split(';') {
        let mut kv = decl.splitn(2, ':');
        let key = kv.next()?.trim();
        if !key.eq_ignore_ascii_case(prop) {
            continue;
        }
        let val = kv.next()?.trim();
        let digits: String = val.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    None
}

/// Pre-ammonia pass: drop `aria-hidden="true"` subtrees, content included.
///
/// Ammonia strips `class`, `style` and `aria-hidden` alike, so any markup the
/// source site only kept off-screen through its own stylesheet lands in the
/// reading pane as literal text — no CSS of ours can hide it afterwards,
/// because the hook it was hidden by is gone. The loudest example is the
/// line-number gutter VitePress/Shiki emits beside every code block
/// (`<div class="line-numbers-wrapper" aria-hidden="true">`, one `<span>` and
/// one `<br>` per line, absolutely positioned over the block): stripped of its
/// class it renders as a column of bare numbers *below* the code, once per
/// block.
///
/// `aria-hidden="true"` is the author stating the subtree carries nothing a
/// reader needs — the same signal Readability uses to skip a node — so honour it
/// generically instead of blocklisting per-site class names.
fn drop_aria_hidden(html: &str) -> Cow<'_, str> {
    if aria_hidden_gate(html) {
        drop_aria_hidden_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`drop_aria_hidden`]: the selector cannot match without the
/// attribute name present, and attribute names are never entity-decoded, so
/// the literal bytes have to be there for the parser to see it too.
fn aria_hidden_gate(html: &str) -> bool {
    contains_ignore_ascii_case(html, "aria-hidden")
}

fn drop_aria_hidden_inner(html: &str) -> Cow<'_, str> {
    let handler = element!("[aria-hidden]", |el| {
        if el
            .get_attribute("aria-hidden")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        {
            el.remove();
        }
        Ok(())
    });
    let stripped = rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(handler),
    )
    .unwrap_or_else(|_| html.to_string());
    // A site that wraps its whole article in `aria-hidden="true"` (sloppy, but
    // it happens) would otherwise leave the entry blank; a gutter beats nothing.
    if stripped.trim().is_empty() && !html.trim().is_empty() {
        return Cow::Borrowed(html);
    }
    Cow::Owned(stripped)
}

/// Pre-ammonia pass: for any `<img>` lacking BOTH `width` and `height`, inject
/// them from `data-original-width`/`data-original-height` or an inline
/// `style="width:..px;height:..px"`. Ammonia strips those hint sources, so this
/// must run before it. Only injects when a usable integer PAIR is found.
fn harvest_image_dimensions(html: &str) -> Cow<'_, str> {
    if harvest_gate(html) {
        harvest_image_dimensions_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`harvest_image_dimensions`]: the handler fires only on an image
/// element, and only injects when it finds a hint in `data-original-*` or an
/// inline `style`.
///
/// `<image` is accepted alongside `<img` on purpose. HTML tree construction
/// rewrites a stray `<image>` start tag to `img`, and while `lol_html` tokenises
/// rather than building a tree, that is an implementation detail of a
/// dependency rather than a guarantee — cheaper to admit the tag than to
/// depend on it not being adjusted.
fn harvest_gate(html: &str) -> bool {
    (contains_ignore_ascii_case(html, "<img") || contains_ignore_ascii_case(html, "<image"))
        && (contains_ignore_ascii_case(html, "style")
            || contains_ignore_ascii_case(html, "data-original-"))
}

fn harvest_image_dimensions_inner(html: &str) -> Cow<'_, str> {
    let handler = element!("img", |el| {
        if el.get_attribute("width").is_some() || el.get_attribute("height").is_some() {
            return Ok(());
        }
        let style = el.get_attribute("style").unwrap_or_default();
        let w = el
            .get_attribute("data-original-width")
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .or_else(|| style_dim(&style, "width"));
        let h = el
            .get_attribute("data-original-height")
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .or_else(|| style_dim(&style, "height"));
        // Require a positive integer pair: a harvested 0 (e.g.
        // `style="width:0px"`) would inject `width="0"` and collapse the box to
        // zero height while suppressing the 16/9 loading fallback.
        let positive = |s: &Option<String>| {
            s.as_deref()
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|n| n > 0)
        };
        if positive(&w) && positive(&h) {
            el.set_attribute("width", &w.unwrap())?;
            el.set_attribute("height", &h.unwrap())?;
        }
        Ok(())
    });
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(handler),
    )
    .map_or(Cow::Borrowed(html), Cow::Owned)
}

/// Promote lazy-loaded image URLs into `src` before sanitization.
///
/// Many sites (e.g. `WordPress` with lazy-load plugins) ship a `data:` SVG
/// placeholder in `src` and keep the real URL in a `data-*` attribute. Ammonia
/// later drops both the `data:` src (disallowed scheme) and the unknown `data-*`
/// attribute, leaving an empty `<img>` and making images disappear. Running this
/// first moves the real URL into `src` so the rest of the pipeline can proxy it.
fn promote_lazy_images(html: &str) -> Cow<'_, str> {
    if lazy_gate(html) {
        promote_lazy_images_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`promote_lazy_images`]: a promotion needs one of
/// [`LAZY_SRC_ATTRS`] to be present, so without any of them the pass cannot
/// change a byte. Worth gating harder than the other two — this one builds a
/// full scraper DOM *and* compiles a CSS selector per call, and on a real
/// corpus fewer than 2% of entries carry a lazy attribute at all.
///
/// Note this gate does *not* mention `<img`: html5ever rewrites a stray
/// `<image>` start tag to `img`, so keying on the attribute rather than the
/// tag sidesteps the question entirely.
fn lazy_gate(html: &str) -> bool {
    LAZY_SRC_ATTRS
        .iter()
        .any(|a| contains_ignore_ascii_case(html, a))
}

fn promote_lazy_images_inner(html: &str) -> Cow<'_, str> {
    let document = Html::parse_fragment(html);
    let img_selector = Selector::parse("img").expect("static CSS selector");

    let mut result = html.to_string();

    for element in document.select(&img_selector) {
        let el = element.value();

        // Keep a real (non-placeholder) src as-is.
        let current_src = el.attr("src");
        if let Some(src) = current_src
            && !src.starts_with("data:")
        {
            continue;
        }

        // Find the first usable lazy URL (non-empty, not another placeholder).
        let lazy = LAZY_SRC_ATTRS.iter().find_map(|attr| {
            el.attr(attr)
                .filter(|u| !u.is_empty() && !u.starts_with("data:"))
                .map(|u| (*attr, u))
        });
        let Some((attr_name, real)) = lazy else {
            continue;
        };

        let new_src = format!("src=\"{real}\"");
        if let Some(placeholder) = current_src {
            // Replace the `data:` placeholder src with the real URL.
            let old_amp = format!("src=\"{}\"", placeholder.replace('&', "&amp;"));
            let old_raw = format!("src=\"{placeholder}\"");
            if result.contains(&old_amp) {
                result = result.replacen(&old_amp, &new_src, 1);
            } else {
                result = result.replacen(&old_raw, &new_src, 1);
            }
        } else {
            // No src at all: convert the lazy attribute into `src`.
            let old_amp = format!("{}=\"{}\"", attr_name, real.replace('&', "&amp;"));
            let old_raw = format!("{attr_name}=\"{real}\"");
            if result.contains(&old_amp) {
                result = result.replacen(&old_amp, &new_src, 1);
            } else {
                result = result.replacen(&old_raw, &new_src, 1);
            }
        }
    }

    Cow::Owned(result)
}

/// Decide whether an `<img>` is a tracking pixel, given its attributes.
fn is_tracking_pixel(width: Option<&str>, height: Option<&str>, src: Option<&str>) -> bool {
    let is_tracking_size = match (width, height) {
        (Some(w), Some(h)) => w == "1" && h == "1",
        (Some(w), None) => w == "0",
        (None, Some(h)) => h == "0",
        _ => false,
    };

    let is_tracking_url = if let Some(src) = src {
        let src_lower = src.to_lowercase();
        TRACKING_DOMAINS.iter().any(|d| src_lower.contains(d))
            || TRACKING_PATHS.iter().any(|p| src_lower.contains(p))
    } else {
        false
    };

    is_tracking_size || is_tracking_url
}

/// Strip tracking query parameters from an http(s) URL, returning the rewritten
/// URL only if something was removed. Non-http(s) inputs return `None`.
fn strip_tracking_params_from_url(href: &str) -> Option<String> {
    if !href.starts_with("http://") && !href.starts_with("https://") {
        return None;
    }
    let mut url = Url::parse(href).ok()?;
    let original_query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let filtered_query: Vec<(String, String)> = original_query
        .iter()
        .filter(|(k, _)| !is_tracking_param(k))
        .cloned()
        .collect();
    if filtered_query.len() == original_query.len() {
        return None;
    }
    url.set_query(None);
    if !filtered_query.is_empty() {
        let query_string: String = filtered_query
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(),
                    url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&query_string));
    }
    Some(url.to_string())
}

/// Strip tracking query parameters from an outbound entry/display URL, always
/// returning an owned `String`. Unlike [`strip_tracking_params_from_url`],
/// inputs with nothing to strip (or non-http(s) URLs) are returned unchanged
/// rather than `None`. Used to clean the entry `link` shown in the UI and handed
/// to the summarizer / bookmark services, mirroring the tracking-param removal
/// already applied to links inside article content.
pub fn strip_tracking_params(url: &str) -> String {
    strip_tracking_params_from_url(url).unwrap_or_else(|| url.to_string())
}

/// Consolidated post-ammonia rewrite: a single streaming `lol_html` pass that
/// folds the former four `parse_fragment`-based passes (remove tracking pixels,
/// strip tracking params, rewrite image URLs to the signed proxy, add link
/// privacy attributes) into one parse of the (already-sanitized) HTML.
///
/// `lol_html` exposes attribute values verbatim (it neither decodes `&amp;` on
/// read nor re-encodes `&` on write), so we normalize `&amp;` to `&` before
/// parsing a URL — ammonia emits `&amp;` for query separators, and `Url::parse`
/// needs the raw `&` to split query pairs correctly.
fn rewrite_post_ammonia(
    html: &str,
    secret: &[u8],
    base_url: Option<&str>,
    referrer: Option<&str>,
    proxy_base_url: Option<&str>,
) -> String {
    let parsed_base = base_url.and_then(|u| Url::parse(u).ok());

    let img_handler = element!("img", |el| {
        let width = el.get_attribute("width");
        let height = el.get_attribute("height");
        // Normalize ammonia's `&amp;` back to `&` so URL parsing and proxy
        // signing operate on the real URL.
        let src = el.get_attribute("src").map(|s| s.replace("&amp;", "&"));

        // Remove tracking pixels outright.
        if is_tracking_pixel(width.as_deref(), height.as_deref(), src.as_deref()) {
            el.remove();
            return Ok(());
        }

        // Rewrite the image src to the signed proxy URL (skip data: URLs).
        if let Some(src) = src {
            if src.starts_with("data:") {
                return Ok(());
            }
            let absolute_url = if src.starts_with("http://") || src.starts_with("https://") {
                Some(src.clone())
            } else if let Some(ref base) = parsed_base {
                base.join(&src).ok().map(|u| u.to_string())
            } else {
                None
            };
            if let Some(url) = absolute_url {
                let proxy_url = if let Some(ref_val) = referrer {
                    create_proxy_url_with_referrer(&url, ref_val, secret, proxy_base_url)
                } else {
                    create_proxy_url(&url, secret, proxy_base_url)
                };
                el.set_attribute("src", &proxy_url)?;
                el.set_attribute("loading", "lazy")?;
                el.set_attribute("decoding", "async")?;
                el.set_attribute("data-img-state", "loading")?;
            }
        }
        Ok(())
    });

    let a_handler = element!("a[href]", |el| {
        let Some(href) = el.get_attribute("href") else {
            return Ok(());
        };
        // Normalize ammonia's `&amp;` back to `&` so query pairs split correctly.
        let href = href.replace("&amp;", "&");
        if !href.starts_with("http://") && !href.starts_with("https://") {
            return Ok(());
        }
        // Strip tracking params first, then apply privacy attributes.
        if let Some(stripped) = strip_tracking_params_from_url(&href) {
            el.set_attribute("href", &stripped)?;
        }
        el.set_attribute("target", "_blank")?;
        el.set_attribute("referrerpolicy", "no-referrer")?;
        Ok(())
    });

    let settings = RewriteStrSettings::new()
        .append_element_content_handler(img_handler)
        .append_element_content_handler(a_handler);
    let rewritten = rewrite_str(html, settings);
    rewritten.unwrap_or_else(|_| html.to_string())
}

pub fn sanitize_html(
    content: &str,
    secret: &[u8],
    base_url: Option<&str>,
    referrer: Option<&str>,
    proxy_base_url: Option<&str>,
) -> String {
    let allowed_tags: HashSet<&str> = [
        "p",
        "br",
        "a",
        "strong",
        "em",
        "b",
        "i",
        "ul",
        "ol",
        "li",
        "blockquote",
        "pre",
        "code",
        "img",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "div",
        "span",
        "figure",
        "figcaption",
        "table",
        "thead",
        "tbody",
        "tr",
        "th",
        "td",
    ]
    .iter()
    .copied()
    .collect();

    let url_schemes: HashSet<&str> = ["http", "https"].iter().copied().collect();

    // Step 0: Drop author-hidden scaffolding, then promote lazy-loaded image
    // URLs into src before ammonia drops the data: placeholder and the unknown
    // data-* attributes. Both read attributes ammonia is about to strip.
    let visible = drop_aria_hidden(content);
    let unlazied = promote_lazy_images(&visible);
    let unlazied = harvest_image_dimensions(&unlazied);

    // Step 1: Ammonia sanitization (already adds rel="noopener noreferrer")
    let sanitized = Builder::default()
        .tags(allowed_tags)
        .link_rel(Some("noopener noreferrer"))
        .url_schemes(url_schemes)
        .clean(&unlazied)
        .to_string();

    // Steps 2-5 folded into a single streaming lol_html pass: remove tracking
    // pixels, strip tracking params, rewrite image URLs to the signed proxy, and
    // add link privacy attributes — all in one parse of the sanitized HTML.
    rewrite_post_ammonia(&sanitized, secret, base_url, referrer, proxy_base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &[u8] = b"test_secret_key_32_bytes_long!!!";

    #[test]
    fn test_sanitize_basic_html() {
        let input = "<p>Hello <strong>world</strong></p>";
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert_eq!(output, "<p>Hello <strong>world</strong></p>");
    }

    #[test]
    fn test_remove_script_tags() {
        let input = "<p>Hello</p><script>alert('xss')</script>";
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("script"));
        assert!(output.contains("<p>Hello</p>"));
    }

    #[test]
    fn test_preserve_links() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("href=\"https://example.com\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_remove_javascript_urls() {
        let input = r#"<a href="javascript:alert('xss')">Click</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("javascript"));
    }

    #[test]
    fn test_preserve_images() {
        let input = r#"<img src="https://example.com/image.jpg" alt="Image">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        // Image URLs should be rewritten to proxy URLs with signature
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("&s="));
        assert!(!output.contains("src=\"https://example.com/image.jpg\""));
    }

    #[test]
    fn test_rewrite_image_urls() {
        let input = r#"<p>Text</p><img src="https://example.com/image.jpg" alt="Image">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("&s="));
        assert!(!output.contains("src=\"https://example.com/image.jpg\""));
    }

    #[test]
    fn test_rewrite_preserves_data_urls() {
        // The post-ammonia rewrite must leave `data:` image sources untouched and
        // never proxy them. (The full `sanitize_html` pipeline runs ammonia first,
        // which drops the disallowed `data:` scheme outright — so this targets the
        // rewrite pass directly, where the data: skip lives.)
        let input = r#"<img src="data:image/png;base64,abc123" alt="Data URL">"#;
        let output = rewrite_post_ammonia(input, TEST_SECRET, None, None, None);
        assert!(output.contains("data:image/png;base64,abc123"));
        assert!(!output.contains("/api/proxy/image"));
    }

    #[test]
    fn test_rewrite_multiple_images() {
        let input = r#"<img src="https://a.com/1.jpg"><img src="https://b.com/2.jpg">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("src=\"https://a.com/1.jpg\""));
        assert!(!output.contains("src=\"https://b.com/2.jpg\""));
        // Both should be rewritten with signatures
        let proxy_count = output.matches("/api/proxy/image?url=").count();
        assert_eq!(proxy_count, 2);
        let sig_count = output.matches("&s=").count();
        assert_eq!(sig_count, 2);
    }

    #[test]
    fn test_promote_lazy_image_data_lazy_src() {
        // Lazy-loaded images (e.g. WordPress + lazy-load plugins) carry a data: SVG
        // placeholder in src and the real URL in data-lazy-src. The real image must
        // be promoted and proxied, not dropped.
        let input = r#"<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-lazy-src="https://example.com/real.jpg" alt="Photo">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/post"),
            None,
            None,
        );
        assert!(
            output.contains("/api/proxy/image?url="),
            "expected lazy image to be proxied, got: {output}"
        );
        assert!(output.contains("&s="));
        assert!(
            !output.contains("data:image/svg"),
            "placeholder should be replaced, got: {output}"
        );
    }

    #[test]
    fn test_promote_lazy_image_data_src() {
        let input =
            r#"<img src="data:image/gif;base64,R0lGOD" data-src="https://example.com/photo.png">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/post"),
            None,
            None,
        );
        assert!(
            output.contains("/api/proxy/image?url="),
            "expected lazy image to be proxied, got: {output}"
        );
        assert!(!output.contains("data:image/gif"));
    }

    #[test]
    fn test_promote_lazy_image_relative_data_src() {
        // Relative lazy URLs must be resolved against base_url before proxying.
        let input = r#"<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-src="/img/pic.jpg">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/post"),
            None,
            None,
        );
        assert!(
            output.contains("/api/proxy/image?url="),
            "expected relative lazy image to be proxied, got: {output}"
        );
    }

    #[test]
    fn test_real_src_not_overridden_by_lazy_attr() {
        // When src is already a real URL, it must win even if a lazy attr exists.
        let input =
            r#"<img src="https://example.com/real.jpg" data-src="https://example.com/other.jpg">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/post"),
            None,
            None,
        );
        // real.jpg must be the one proxied; other.jpg must not appear.
        assert!(output.contains("/api/proxy/image?url="));
        assert!(
            !output.contains("other.jpg"),
            "lazy attr must not override a real src, got: {output}"
        );
    }

    #[test]
    fn test_links_have_target_blank() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("target=\"_blank\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_multiple_links_have_target_blank() {
        let input = r#"<a href="https://a.com">A</a><a href="https://b.com">B</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        let target_count = output.matches("target=\"_blank\"").count();
        assert_eq!(target_count, 2);
    }

    #[test]
    fn test_relative_links_no_target_blank() {
        let input = r#"<a href="/local/path">Local</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("target=\"_blank\""));
    }

    // ============ Tracking Pixel Removal Tests ============

    #[test]
    fn test_remove_1x1_pixel_images() {
        let input = r#"<p>Text</p><img src="https://example.com/pixel.gif" width="1" height="1">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("<img"));
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_remove_zero_dimension_images() {
        let input = r#"<img src="https://example.com/hidden.gif" width="0">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("<img"));

        let input2 = r#"<img src="https://example.com/hidden.gif" height="0">"#;
        let output2 = sanitize_html(input2, TEST_SECRET, None, None, None);
        assert!(!output2.contains("<img"));
    }

    #[test]
    fn test_remove_tracking_domain_images() {
        let input1 = r#"<img src="https://pixel.example.com/track.gif">"#;
        let output1 = sanitize_html(input1, TEST_SECRET, None, None, None);
        assert!(!output1.contains("<img"));

        let input2 = r#"<img src="https://beacon.example.com/img.gif">"#;
        let output2 = sanitize_html(input2, TEST_SECRET, None, None, None);
        assert!(!output2.contains("<img"));

        let input3 = r#"<img src="https://track.example.com/img.gif">"#;
        let output3 = sanitize_html(input3, TEST_SECRET, None, None, None);
        assert!(!output3.contains("<img"));

        let input4 = r#"<img src="https://analytics.example.com/img.gif">"#;
        let output4 = sanitize_html(input4, TEST_SECRET, None, None, None);
        assert!(!output4.contains("<img"));
    }

    #[test]
    fn test_remove_tracking_path_images() {
        let input1 = r#"<img src="https://example.com/pixel/tracker.gif">"#;
        let output1 = sanitize_html(input1, TEST_SECRET, None, None, None);
        assert!(!output1.contains("<img"));

        let input2 = r#"<img src="https://example.com/beacon/img.gif">"#;
        let output2 = sanitize_html(input2, TEST_SECRET, None, None, None);
        assert!(!output2.contains("<img"));

        let input3 = r#"<img src="https://example.com/1x1.gif">"#;
        let output3 = sanitize_html(input3, TEST_SECRET, None, None, None);
        assert!(!output3.contains("<img"));
    }

    #[test]
    fn test_remove_tracking_pixel_with_data_src_attr() {
        // A tracking pixel may also carry a `data-src`; the real `src` is the
        // flagged tracking URL and the tag must still be removed. ammonia strips
        // the unknown `data-src` attribute, then the lol_html pass parses the tag
        // structurally and removes it based on the real `src`.
        let input = r#"<p>Text</p><img data-src="https://example.com/real.jpg" src="https://pixel.tracker.com/p.gif" width="1" height="1">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("<img"), "tracking pixel should be removed");
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_keep_normal_image_with_data_src_when_only_data_src_flagged() {
        // A normal image whose `src` is NOT a tracking URL must be kept (and
        // proxied) even if a `data-src` happens to look tracking-ish; only the
        // real `src` counts. `promote_lazy_images` leaves the real `src` alone, so
        // the kept image is the proxied photo, not the data-src URL.
        let input = r#"<img data-src="https://pixel.tracker.com/x.gif" src="https://example.com/photo.jpg" width="800" height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(
            output.contains("/api/proxy/image?url="),
            "real image should be kept and proxied, got: {output}"
        );
        assert!(
            !output.contains("pixel.tracker.com"),
            "the tracking data-src must not survive, got: {output}"
        );
    }

    #[test]
    fn test_remove_multiple_tracking_pixels_single_pass() {
        let input = r#"<img src="https://pixel.a.com/1.gif" width="1" height="1"><p>a</p><img src="https://beacon.b.com/2.gif"><p>b</p><img src="https://example.com/keep.jpg" width="800">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("pixel.a.com"));
        assert!(!output.contains("beacon.b.com"));
        // The non-tracking image survives as a proxied URL.
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("<p>a</p>"));
        assert!(output.contains("<p>b</p>"));
    }

    #[test]
    fn test_preserve_normal_images() {
        let input = r#"<img src="https://example.com/photo.jpg" width="800" height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("<img"));
        // The normal image is kept and proxied.
        assert!(output.contains("/api/proxy/image?url="));
    }

    // ============ URL Tracking Parameter Tests ============

    #[test]
    fn test_strip_utm_parameters() {
        let input = r#"<a href="https://example.com/page?utm_source=twitter&utm_medium=social&utm_campaign=test">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("utm_source"));
        assert!(!output.contains("utm_medium"));
        assert!(!output.contains("utm_campaign"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_facebook_click_id() {
        let input = r#"<a href="https://example.com/page?fbclid=ABC123">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("fbclid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_google_click_id() {
        let input = r#"<a href="https://example.com/page?gclid=XYZ789">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("gclid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_microsoft_click_id() {
        let input = r#"<a href="https://example.com/page?msclkid=MSC456">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("msclkid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_preserve_non_tracking_parameters() {
        let input = r#"<a href="https://example.com/search?q=rust&page=2">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("q=rust"));
        assert!(output.contains("page=2"));
    }

    #[test]
    fn test_strip_multiple_tracking_params() {
        let input = r#"<a href="https://example.com/page?id=123&fbclid=FB1&gclid=GC1&utm_source=test&valid=yes">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("fbclid"));
        assert!(!output.contains("gclid"));
        assert!(!output.contains("utm_source"));
        assert!(output.contains("id=123"));
        assert!(output.contains("valid=yes"));
    }

    #[test]
    fn test_strip_tracking_params_removes_trackers() {
        // The public helper (used for entry links) strips utm_*/click IDs while
        // keeping genuine query params, and always returns an owned String.
        let cleaned = strip_tracking_params(
            "https://example.com/article?id=42&utm_source=news&fbclid=FB1&page=2",
        );
        assert!(!cleaned.contains("utm_source"));
        assert!(!cleaned.contains("fbclid"));
        assert!(cleaned.contains("id=42"));
        assert!(cleaned.contains("page=2"));
    }

    #[test]
    fn test_strip_tracking_params_returns_input_when_clean() {
        // Nothing to strip → the URL is returned unchanged (not None).
        assert_eq!(
            strip_tracking_params("https://example.com/article?id=42&page=2"),
            "https://example.com/article?id=42&page=2"
        );
        // A param-free URL is likewise untouched.
        assert_eq!(
            strip_tracking_params("https://example.com/article"),
            "https://example.com/article"
        );
    }

    #[test]
    fn test_strip_tracking_params_passes_through_non_http() {
        // Non-http(s) inputs (e.g. relative or mailto) are returned verbatim.
        assert_eq!(
            strip_tracking_params("/relative/path?utm_source=x"),
            "/relative/path?utm_source=x"
        );
        assert_eq!(
            strip_tracking_params("mailto:someone@example.com"),
            "mailto:someone@example.com"
        );
    }

    #[test]
    fn test_preserve_url_without_params() {
        let input = r#"<a href="https://example.com/page">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        // A param-free URL is preserved verbatim in href (only privacy attrs added).
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_matomo_params() {
        let input =
            r#"<a href="https://example.com/page?mtm_campaign=test&mtm_source=email">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("mtm_campaign"));
        assert!(!output.contains("mtm_source"));
    }

    // ============ Referrer Policy Tests ============

    #[test]
    fn test_links_have_referrerpolicy() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("referrerpolicy=\"no-referrer\""));
        assert!(output.contains("target=\"_blank\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_multiple_links_have_referrerpolicy() {
        let input = r#"<a href="https://a.com">A</a><a href="https://b.com">B</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        let policy_count = output.matches("referrerpolicy=\"no-referrer\"").count();
        assert_eq!(policy_count, 2);
    }

    // ============ Integration Tests ============

    #[test]
    fn test_sanitize_removes_tracking_pixels() {
        let input =
            r#"<p>Text</p><img src="https://pixel.tracker.com/img.gif" width="1" height="1">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("pixel.tracker.com"));
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_sanitize_strips_tracking_params() {
        let input = r#"<a href="https://example.com/page?utm_source=test&id=123">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("utm_source"));
        assert!(output.contains("id=123"));
    }

    // ============ Relative URL Tests ============

    #[test]
    fn test_rewrite_relative_image_urls_with_base() {
        let input = r#"<img src="/images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/article/123"),
            None,
            None,
        );
        assert!(output.contains("/api/proxy/image?url="));
        assert!(!output.contains("src=\"/images/photo.jpg\""));
    }

    #[test]
    fn test_rewrite_relative_path_image_urls() {
        let input = r#"<img src="images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/article/123"),
            None,
            None,
        );
        assert!(output.contains("/api/proxy/image?url="));
        assert!(!output.contains("src=\"images/photo.jpg\""));
    }

    #[test]
    fn test_rewrite_parent_relative_image_urls() {
        let input = r#"<img src="../images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/article/123"),
            None,
            None,
        );
        assert!(output.contains("/api/proxy/image?url="));
        assert!(!output.contains("src=\"../images/photo.jpg\""));
    }

    #[test]
    fn test_relative_images_without_base_url_unchanged() {
        let input = r#"<img src="/images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        // Without base URL, relative paths should remain unchanged
        assert!(output.contains("src=\"/images/photo.jpg\""));
        assert!(!output.contains("/api/proxy/image"));
    }

    #[test]
    fn test_rewrite_image_url_with_query_params_containing_ampersand() {
        // The src carries a `&` in its query. lol_html decodes attribute values and
        // re-encodes on write, so the rewrite must still succeed and proxy the URL.
        let input = r#"<img src="https://example.com/image.jpg?size=800&format=webp" alt="Photo">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(
            output.contains("/api/proxy/image?url="),
            "image should be proxied"
        );
        assert!(
            !output.contains("src=\"https://example.com/image.jpg"),
            "original src should be replaced"
        );
    }

    #[test]
    fn test_sanitize_html_proxies_image_with_query_ampersand() {
        let input = r#"<img src="https://cdn.example.com/photo?w=800&h=600" alt="Photo">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(
            output.contains("/api/proxy/image?url="),
            "image should be proxied"
        );
        assert!(
            !output.contains("cdn.example.com"),
            "original domain should not appear in src"
        );
    }

    #[test]
    fn test_mixed_absolute_and_relative_images() {
        let input = r#"<img src="https://cdn.example.com/abs.jpg"><img src="/images/rel.jpg">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/page"),
            None,
            None,
        );
        // Both should be rewritten
        let proxy_count = output.matches("/api/proxy/image?url=").count();
        assert_eq!(proxy_count, 2);
    }

    #[test]
    fn test_sanitize_html_with_base_url() {
        let input = r#"<p>Text</p><img src="/images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/article"),
            None,
            None,
        );
        assert!(output.contains("/api/proxy/image?url="));
        assert!(!output.contains("src=\"/images/photo.jpg\""));
    }

    #[test]
    fn test_rewrite_image_urls_with_proxy_base() {
        let input = r#"<img src="https://example.com/image.jpg">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            None,
            None,
            Some("https://rdrs.example.com"),
        );
        assert!(output.contains("https://rdrs.example.com/api/proxy/image?url="));
        assert!(!output.contains("src=\"/api/proxy/image"));
    }

    #[test]
    fn test_sanitize_tracking_pixel_with_gt_in_src() {
        // A tracking pixel whose src contains a literal `>` (e.g. an encoded
        // query string) must still be removed cleanly. The previous substring-
        // scanning pass keyed off the first `>` and could mis-bound such a tag;
        // the lol_html pass parses the tag structurally, so the `>` inside the
        // attribute value is handled correctly.
        let input = r#"<p>keep</p><img src="https://pixel.tracker.com/p.gif?q=a>b" width="1" height="1"><p>tail</p>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(
            !output.contains("pixel.tracker.com"),
            "tracking pixel with > in src should be removed, got: {output}"
        );
        assert!(output.contains("<p>keep</p>"), "got: {output}");
        assert!(output.contains("<p>tail</p>"), "got: {output}");
    }

    #[test]
    fn test_sanitize_html_with_proxy_base() {
        let input = r#"<img src="https://cdn.example.com/photo.jpg">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            None,
            None,
            Some("https://rdrs.example.com"),
        );
        assert!(output.contains("https://rdrs.example.com/api/proxy/image?url="));
    }

    #[test]
    fn test_image_width_height_preserved() {
        let input = r#"<img src="https://example.com/a.jpg" width="640" height="480" alt="x">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(
            output.contains("width=\"640\""),
            "width must survive: {output}"
        );
        assert!(
            output.contains("height=\"480\""),
            "height must survive: {output}"
        );
    }

    #[test]
    fn test_harvest_dims_from_data_original() {
        let input = r#"<img src="https://e.com/a.jpg" data-original-width="800" data-original-height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"800\""), "{output}");
        assert!(output.contains("height=\"600\""), "{output}");
    }
    #[test]
    fn test_harvest_dims_from_style() {
        let input = r#"<img src="https://e.com/a.jpg" style="width:320px;height:240px">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"320\""), "{output}");
        assert!(output.contains("height=\"240\""), "{output}");
    }
    #[test]
    fn test_harvest_skips_when_dims_present() {
        let input = r#"<img src="https://e.com/a.jpg" width="100" height="50" data-original-width="800" data-original-height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"100\""), "{output}");
        assert!(!output.contains("width=\"800\""), "{output}");
    }

    #[test]
    fn test_harvest_skips_zero_dimensions() {
        // A harvested 0 would collapse the box to zero height — never inject it.
        let input = r#"<img src="https://e.com/a.jpg" style="width:0px;height:0px">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("width=\"0\""), "{output}");
        assert!(!output.contains("height=\"0\""), "{output}");
    }

    #[test]
    fn test_img_tagged_loading_state() {
        let input = r#"<img src="https://e.com/a.jpg" alt="x">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("data-img-state=\"loading\""), "{output}");
    }

    #[test]
    fn test_drops_code_block_line_number_gutter() {
        // VitePress/Shiki shape: the gutter is a sibling of <pre>, hidden by the
        // source site's CSS via a class ammonia strips. Left in, it renders as a
        // column of bare numbers under every code block.
        let input = concat!(
            r#"<div class="language-ts line-numbers-mode"><span class="lang">ts</span>"#,
            r#"<pre><code><span class="line">const a = 1;</span>"#,
            "\n",
            r#"<span class="line">const b = 2;</span></code></pre>"#,
            r#"<div class="line-numbers-wrapper" aria-hidden="true">"#,
            r#"<span class="line-number">1</span><br><span class="line-number">2</span><br></div></div>"#,
        );
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("const a = 1;"), "{output}");
        assert!(output.contains("const b = 2;"), "{output}");
        assert!(!output.contains("<br>"), "gutter survived: {output}");
        assert!(
            !output.contains("<span>1</span>"),
            "gutter survived: {output}"
        );
    }

    #[test]
    fn test_keeps_aria_hidden_false_and_absent() {
        let input = r#"<p aria-hidden="false">keep me</p><p>and me</p>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("keep me"), "{output}");
        assert!(output.contains("and me"), "{output}");
    }

    #[test]
    fn test_aria_hidden_matched_case_insensitively() {
        let input = r#"<p>body</p><span aria-hidden="TRUE">decor</span>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("body"), "{output}");
        assert!(!output.contains("decor"), "{output}");
    }

    #[test]
    fn test_wholly_aria_hidden_content_is_kept() {
        // Blanking the entry is worse than showing markup the author hid.
        let input = r#"<div aria-hidden="true"><p>the entire article</p></div>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("the entire article"), "{output}");
    }

    /// Documents chosen to sit near the edges of the three gates: tag- and
    /// attribute-name casing, an `<image>` start tag (which HTML tree
    /// construction rewrites to `img`), hint attributes without an image,
    /// images without hints, markup inside comments, and `data-original`
    /// against its `data-original-width` near-namesake.
    const GATE_CORPUS: &[&str] = &[
        "",
        "<p>plain</p>",
        // Paired with visible content on purpose: an `aria-hidden` document
        // with nothing else in it hits the pass's own "would blank the entry"
        // fallback, which returns the input unchanged and would mask a gate
        // that wrongly skipped it.
        r#"<p>body</p><span ARIA-HIDDEN="true">decor</span>"#,
        r#"<p aria-hidden="false">x</p>"#,
        r#"<IMG SRC="https://e.com/a.jpg">"#,
        r#"<img src="https://e.com/a.jpg" style="width:8px;height:6px">"#,
        r#"<img src="https://e.com/a.jpg" DATA-ORIGINAL-WIDTH="8" DATA-ORIGINAL-HEIGHT="6">"#,
        r#"<image src="https://e.com/a.jpg" data-original-width="8" data-original-height="6">"#,
        r#"<image src="data:image/gif;base64,R0lGOD" data-src="https://e.com/b.jpg">"#,
        r#"<img src="data:image/gif;base64,R0lGOD" DATA-LAZY-SRC="https://e.com/b.jpg">"#,
        r#"<img src="data:image/gif;base64,R0lGOD" data-original="https://e.com/b.jpg">"#,
        r#"<div style="color:red">no image here</div>"#,
        r#"<!-- <img src="x" style="width:8px"> -->"#,
        "<p>a &lt;img&gt; mention in text</p>",
    ];

    /// The gates are only sound if each is a *superset* of the pass it fronts:
    /// whenever a gate says "skip", running the pass anyway must be a no-op.
    /// This asserts exactly that, so a pass that grows a new trigger without
    /// its gate growing to match fails here instead of silently ceasing to
    /// fire — which is the failure mode a substring gate actually risks.
    #[test]
    fn gates_are_supersets_of_the_passes_they_front() {
        for doc in GATE_CORPUS {
            if !aria_hidden_gate(doc) {
                assert_eq!(
                    drop_aria_hidden_inner(doc).as_ref(),
                    *doc,
                    "aria_hidden_gate skipped a document the pass would rewrite: {doc}"
                );
            }
            if !lazy_gate(doc) {
                assert_eq!(
                    promote_lazy_images_inner(doc).as_ref(),
                    *doc,
                    "lazy_gate skipped a document the pass would rewrite: {doc}"
                );
            }
            if !harvest_gate(doc) {
                assert_eq!(
                    harvest_image_dimensions_inner(doc).as_ref(),
                    *doc,
                    "harvest_gate skipped a document the pass would rewrite: {doc}"
                );
            }
        }
    }

    #[test]
    fn test_contains_ignore_ascii_case() {
        assert!(contains_ignore_ascii_case("a <IMG> b", "<img"));
        assert!(contains_ignore_ascii_case("DATA-Src=", "data-src"));
        assert!(contains_ignore_ascii_case("xx", "xx"));
        assert!(!contains_ignore_ascii_case("x", "xx"));
        assert!(!contains_ignore_ascii_case("data_src", "data-src"));
        // `windows(0)` would panic rather than answer.
        assert!(contains_ignore_ascii_case("", ""));
    }

    // The three pre-passes are gated on a cheap substring test so the common
    // document skips their parse. HTML attribute names are case-insensitive, so
    // each gate has to be too — these pin that, since a lowercase-only gate
    // would make the pass silently vanish rather than fail loudly.

    #[test]
    fn test_uppercase_aria_hidden_attribute_still_dropped() {
        let input = r#"<p>body</p><span ARIA-HIDDEN="true">decor</span>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("body"), "{output}");
        assert!(!output.contains("decor"), "{output}");
    }

    #[test]
    fn test_uppercase_lazy_attribute_still_promoted() {
        let input =
            r#"<img src="data:image/gif;base64,R0lGOD" DATA-SRC="https://example.com/photo.png">"#;
        let output = sanitize_html(
            input,
            TEST_SECRET,
            Some("https://example.com/post"),
            None,
            None,
        );
        assert!(output.contains("/api/proxy/image?url="), "{output}");
        assert!(!output.contains("data:image/gif"), "{output}");
    }

    #[test]
    fn test_uppercase_dimension_hints_still_harvested() {
        let input = r#"<img src="https://example.com/a.jpg" DATA-ORIGINAL-WIDTH="800" DATA-ORIGINAL-HEIGHT="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains(r#"width="800""#), "{output}");
        assert!(output.contains(r#"height="600""#), "{output}");
    }

    #[test]
    fn test_document_without_pre_pass_triggers_is_unchanged() {
        // The gates' happy path: nothing here can trigger any of the three, so
        // the output must match what the ammonia + rewrite steps alone produce.
        let input = r"<p>Plain <strong>body</strong> text.</p>";
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert_eq!(output, "<p>Plain <strong>body</strong> text.</p>");
    }
}
