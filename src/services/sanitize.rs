use ammonia::Builder;
use lol_html::{element, rewrite_str, RewriteStrSettings};
use scraper::{Html, Selector};
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

/// Check if a parameter name is a tracking parameter
fn is_tracking_param(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    TRACKING_PARAMS.iter().any(|&p| name_lower == p)
        || TRACKING_PARAM_PREFIXES
            .iter()
            .any(|p| name_lower.starts_with(p))
}

/// Attributes that carry the real image URL for lazy-loaded images, in priority order.
const LAZY_SRC_ATTRS: &[&str] = &["data-src", "data-lazy-src", "data-original"];

/// Parse a `width:NNpx` / `height:NNpx` integer out of an inline `style`.
fn style_dim(style: &str, prop: &str) -> Option<String> {
    for decl in style.split(';') {
        let mut kv = decl.splitn(2, ':');
        let key = kv.next()?.trim();
        if !key.eq_ignore_ascii_case(prop) {
            continue;
        }
        let val = kv.next()?.trim();
        let digits: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    None
}

/// Pre-ammonia pass: for any `<img>` lacking BOTH `width` and `height`, inject
/// them from `data-original-width`/`data-original-height` or an inline
/// `style="width:..px;height:..px"`. Ammonia strips those hint sources, so this
/// must run before it. Only injects when a usable integer PAIR is found.
fn harvest_image_dimensions(html: &str) -> String {
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
    .unwrap_or_else(|_| html.to_string())
}

/// Promote lazy-loaded image URLs into `src` before sanitization.
///
/// Many sites (e.g. WordPress with lazy-load plugins) ship a `data:` SVG
/// placeholder in `src` and keep the real URL in a `data-*` attribute. Ammonia
/// later drops both the `data:` src (disallowed scheme) and the unknown `data-*`
/// attribute, leaving an empty `<img>` and making images disappear. Running this
/// first moves the real URL into `src` so the rest of the pipeline can proxy it.
fn promote_lazy_images(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let img_selector = Selector::parse("img").expect("static CSS selector");

    let mut result = html.to_string();

    for element in document.select(&img_selector) {
        let el = element.value();

        // Keep a real (non-placeholder) src as-is.
        let current_src = el.attr("src");
        if let Some(src) = current_src {
            if !src.starts_with("data:") {
                continue;
            }
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

        let new_src = format!("src=\"{}\"", real);
        match current_src {
            // Replace the `data:` placeholder src with the real URL.
            Some(placeholder) => {
                let old_amp = format!("src=\"{}\"", placeholder.replace('&', "&amp;"));
                let old_raw = format!("src=\"{}\"", placeholder);
                if result.contains(&old_amp) {
                    result = result.replacen(&old_amp, &new_src, 1);
                } else {
                    result = result.replacen(&old_raw, &new_src, 1);
                }
            }
            // No src at all: convert the lazy attribute into `src`.
            None => {
                let old_amp = format!("{}=\"{}\"", attr_name, real.replace('&', "&amp;"));
                let old_raw = format!("{}=\"{}\"", attr_name, real);
                if result.contains(&old_amp) {
                    result = result.replacen(&old_amp, &new_src, 1);
                } else {
                    result = result.replacen(&old_raw, &new_src, 1);
                }
            }
        }
    }

    result
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

    // Step 0: Promote lazy-loaded image URLs into src before ammonia drops the
    // data: placeholder and the unknown data-* attributes.
    let unlazied = promote_lazy_images(content);
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
}
