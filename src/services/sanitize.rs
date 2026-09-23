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

/// Matched case-insensitively; `utm_`/`mtm_` suffixes are open-ended.
fn is_tracking_param(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    TRACKING_PARAMS.iter().any(|&p| name_lower == p)
        || TRACKING_PARAM_PREFIXES
            .iter()
            .any(|p| name_lower.starts_with(p))
}

/// Attributes that carry the real image URL for lazy-loaded images, in priority order.
const LAZY_SRC_ATTRS: &[&str] = &["data-src", "data-lazy-src", "data-original"];

/// ASCII-case-insensitive substring test for the pre-pass gates, since HTML
/// names are case-insensitive. `needle` must be lowercase.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    debug_assert!(needle.bytes().all(|b| !b.is_ascii_uppercase()));
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    // `windows(0)` panics.
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
/// Ammonia strips the `class`/`style` that kept such markup (e.g. Shiki's
/// line-number gutter) off-screen, so it would otherwise render as bare text.
fn drop_aria_hidden(html: &str) -> Cow<'_, str> {
    if aria_hidden_gate(html) {
        drop_aria_hidden_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`drop_aria_hidden`]: attribute names are never entity-decoded, so
/// the literal bytes must be present.
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
    // A wholly `aria-hidden` article would otherwise render blank.
    if stripped.trim().is_empty() && !html.trim().is_empty() {
        return Cow::Borrowed(html);
    }
    Cow::Owned(stripped)
}

/// Pre-ammonia pass: give an `<img>` lacking both `width` and `height` a
/// positive integer pair from `data-original-*` or inline `style`, which ammonia
/// strips.
fn harvest_image_dimensions(html: &str) -> Cow<'_, str> {
    if harvest_gate(html) {
        harvest_image_dimensions_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`harvest_image_dimensions`]. Accepts `<image` too, since HTML
/// parsers may rewrite it to `img`.
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
        // A harvested 0 would collapse the box and suppress the 16/9 fallback.
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

/// Promote lazy-loaded image URLs into `src` before ammonia drops the `data:`
/// placeholder and the `data-*` attribute holding the real URL.
fn promote_lazy_images(html: &str) -> Cow<'_, str> {
    if lazy_gate(html) {
        promote_lazy_images_inner(html)
    } else {
        Cow::Borrowed(html)
    }
}

/// Gate for [`promote_lazy_images`], the costliest pass (full DOM plus a
/// selector); keyed on the attribute, not `<img`, to sidestep `<image>`.
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

        // First usable lazy URL (non-empty, not another placeholder).
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
            let old_amp = format!("src=\"{}\"", placeholder.replace('&', "&amp;"));
            let old_raw = format!("src=\"{placeholder}\"");
            if result.contains(&old_amp) {
                result = result.replacen(&old_amp, &new_src, 1);
            } else {
                result = result.replacen(&old_raw, &new_src, 1);
            }
        } else {
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

/// Strip tracking params from an http(s) URL; `None` if nothing was removed or
/// the URL is not http(s).
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

/// Like `strip_tracking_params_from_url` but always returns the (possibly
/// unchanged) URL. Used for entry links shown and handed to external services.
pub fn strip_tracking_params(url: &str) -> String {
    strip_tracking_params_from_url(url).unwrap_or_else(|| url.to_string())
}

/// Post-ammonia rewrite in one `lol_html` pass: remove tracking pixels, strip
/// tracking params, proxy images, add link privacy attributes.
///
/// Attribute values are verbatim, so ammonia's `&amp;` is normalized to `&`
/// before `Url::parse`.
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
        let src = el.get_attribute("src").map(|s| s.replace("&amp;", "&"));

        if is_tracking_pixel(width.as_deref(), height.as_deref(), src.as_deref()) {
            el.remove();
            return Ok(());
        }

        // Rewrite to the signed proxy URL (skip data: URLs).
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
            // Fail closed: an unresolvable src would 404 on our origin or be fetched
            // from the author's host outside the proxy, leaking the reader's IP. Every
            // image must go through the proxy.
            let Some(url) = absolute_url else {
                el.remove();
                return Ok(());
            };
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
        Ok(())
    });

    let a_handler = element!("a[href]", |el| {
        let Some(href) = el.get_attribute("href") else {
            return Ok(());
        };
        let href = href.replace("&amp;", "&");
        if !href.starts_with("http://") && !href.starts_with("https://") {
            return Ok(());
        }
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

/// Markup an AI summary may keep: inline prose only, no images, tables,
/// headings or links. See [`sanitize_summary`].
const SUMMARY_TAGS: &[&str] = &[
    "p", "br", "strong", "em", "b", "i", "ul", "ol", "li", "code",
];

/// Reduce a model-written summary to inline prose markup.
///
/// The summary is attacker-influenced like feed content and rendered via
/// `|safe`. `a` is excluded: a link in a box the UI presents as trustworthy is
/// a phishing primitive. Stray `<` is escaped rather than opening a tag.
pub fn sanitize_summary(summary: &str) -> String {
    Builder::default()
        .tags(SUMMARY_TAGS.iter().copied().collect())
        .clean(summary)
        .to_string()
}

/// Sanitize untrusted feed markup to the whitelist, then remove tracking
/// pixels/params, add link privacy attributes, and route **every image through
/// the signed image proxy**.
///
/// An image `src` that cannot resolve to absolute `http(s)` is dropped, so pass
/// [`crate::models::entry::EntryWithFeed::content_base_url`]; `None` is for
/// callers with no document base, and tests.
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

    // Step 0: pre-passes read attributes ammonia is about to strip.
    let visible = drop_aria_hidden(content);
    let unlazied = promote_lazy_images(&visible);
    let unlazied = harvest_image_dimensions(&unlazied);

    // Step 1: ammonia (adds rel="noopener noreferrer").
    let sanitized = Builder::default()
        .tags(allowed_tags)
        .link_rel(Some("noopener noreferrer"))
        .url_schemes(url_schemes)
        .clean(&unlazied)
        .to_string();

    // Steps 2-5: single lol_html rewrite pass.
    rewrite_post_ammonia(&sanitized, secret, base_url, referrer, proxy_base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &[u8] = b"test_secret_key_32_bytes_long!!!";

    fn sanitize_with_base(input: &str, base: &str) -> String {
        sanitize_html(input, TEST_SECRET, Some(base), None, None)
    }

    // ============ AI summary sanitization ============

    #[test]
    fn summary_keeps_prose_markup() {
        let input = "<p>A <strong>bold</strong> and <em>italic</em> line.</p><ul><li>one</li></ul>";
        assert_eq!(sanitize_summary(input), input);
    }

    #[test]
    fn summary_drops_style_and_script_with_their_content() {
        let input =
            "<style>@import url(//evil.tld/x.css);</style><p>Body</p><script>alert(1)</script>";
        let output = sanitize_summary(input);
        assert!(!output.contains("evil.tld"), "{output}");
        assert!(!output.contains("alert"), "{output}");
        assert_eq!(output, "<p>Body</p>");
    }

    #[test]
    fn summary_drops_images_and_event_handlers() {
        // An `<img>` would be an un-proxied external fetch.
        let input = r#"<p onclick="x()">Body <img src="//evil.tld/beacon.gif"></p>"#;
        let output = sanitize_summary(input);
        assert!(!output.contains("evil.tld"), "{output}");
        assert!(!output.contains("onclick"), "{output}");
        assert!(output.contains("Body"), "{output}");
    }

    #[test]
    fn summary_unlinks_anchors_but_keeps_their_text() {
        let input = r#"<p>See <a href="https://evil.tld/login">your account</a>.</p>"#;
        let output = sanitize_summary(input);
        assert!(!output.contains("<a"), "{output}");
        assert!(!output.contains("evil.tld"), "{output}");
        assert!(output.contains("your account"), "{output}");
    }

    #[test]
    fn summary_escapes_plain_text_rather_than_parsing_it() {
        let output = sanitize_summary("Latency held at 5 < 10 ms & stayed there.");
        assert_eq!(output, "Latency held at 5 &lt; 10 ms &amp; stayed there.");
    }

    #[test]
    fn summary_leaves_markdown_untouched_as_text() {
        // Kagi returns markdown; sanitizing must not disturb it.
        let input = "**Bold** and _italic_ with a [link](https://example.com).";
        assert_eq!(sanitize_summary(input), input);
    }

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
    fn test_remove_javascript_urls() {
        let input = r#"<a href="javascript:alert('xss')">Click</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("javascript"));
    }

    #[test]
    fn test_preserve_images() {
        let input = r#"<img src="https://example.com/image.jpg" alt="Image">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("&s="));
        assert!(!output.contains("src=\"https://example.com/image.jpg\""));
    }

    #[test]
    fn test_rewrite_preserves_data_urls() {
        // Targets the rewrite pass directly: the full pipeline drops `data:` in
        // ammonia first.
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
        let proxy_count = output.matches("/api/proxy/image?url=").count();
        assert_eq!(proxy_count, 2);
        let sig_count = output.matches("&s=").count();
        assert_eq!(sig_count, 2);
    }

    #[test]
    fn test_promote_lazy_image_data_lazy_src() {
        // WordPress-style lazy load: data: placeholder in src, real URL in
        // data-lazy-src. It must be promoted and proxied.
        let input = r#"<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-lazy-src="https://example.com/real.jpg" alt="Photo">"#;
        let output = sanitize_with_base(input, "https://example.com/post");
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
        let output = sanitize_with_base(input, "https://example.com/post");
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
        let output = sanitize_with_base(input, "https://example.com/post");
        assert!(
            output.contains("/api/proxy/image?url="),
            "expected relative lazy image to be proxied, got: {output}"
        );
    }

    #[test]
    fn test_real_src_not_overridden_by_lazy_attr() {
        let input =
            r#"<img src="https://example.com/real.jpg" data-src="https://example.com/other.jpg">"#;
        let output = sanitize_with_base(input, "https://example.com/post");
        assert!(output.contains("/api/proxy/image?url="));
        assert!(
            !output.contains("other.jpg"),
            "lazy attr must not override a real src, got: {output}"
        );
    }

    #[test]
    fn test_links_keep_href_and_gain_privacy_attributes() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        for attr in [
            "href=\"https://example.com\"",
            "target=\"_blank\"",
            "rel=\"noopener noreferrer\"",
            "referrerpolicy=\"no-referrer\"",
        ] {
            assert!(output.contains(attr), "{attr} missing: {output}");
        }
    }

    #[test]
    fn test_every_link_gains_privacy_attributes() {
        let input = r#"<a href="https://a.com">A</a><a href="https://b.com">B</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert_eq!(output.matches("target=\"_blank\"").count(), 2);
        assert_eq!(output.matches("referrerpolicy=\"no-referrer\"").count(), 2);
    }

    #[test]
    fn test_relative_links_no_target_blank() {
        let input = r#"<a href="/local/path">Local</a>"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("target=\"_blank\""));
    }

    // ============ Tracking Pixel Removal Tests ============

    #[test]
    fn test_remove_tracking_images_but_keep_the_text() {
        for img in [
            // 1x1 and zero-sized
            r#"<img src="https://example.com/pixel.gif" width="1" height="1">"#,
            r#"<img src="https://pixel.tracker.com/img.gif" width="1" height="1">"#,
            r#"<img src="https://example.com/hidden.gif" width="0">"#,
            r#"<img src="https://example.com/hidden.gif" height="0">"#,
            // tracking hosts
            r#"<img src="https://pixel.example.com/track.gif">"#,
            r#"<img src="https://beacon.example.com/img.gif">"#,
            r#"<img src="https://track.example.com/img.gif">"#,
            r#"<img src="https://analytics.example.com/img.gif">"#,
            // tracking paths
            r#"<img src="https://example.com/pixel/tracker.gif">"#,
            r#"<img src="https://example.com/beacon/img.gif">"#,
            r#"<img src="https://example.com/1x1.gif">"#,
        ] {
            let output = sanitize_html(&format!("<p>Text</p>{img}"), TEST_SECRET, None, None, None);
            assert!(!output.contains("<img"), "{img} survived: {output}");
            assert!(output.contains("<p>Text</p>"), "{output}");
        }
    }

    #[test]
    fn test_remove_tracking_pixel_with_data_src_attr() {
        // The real `src` is a tracking URL, so the tag is removed despite `data-src`.
        let input = r#"<p>Text</p><img data-src="https://example.com/real.jpg" src="https://pixel.tracker.com/p.gif" width="1" height="1">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!output.contains("<img"), "tracking pixel should be removed");
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_keep_normal_image_with_data_src_when_only_data_src_flagged() {
        // Only the real `src` counts: a tracking-ish `data-src` does not drop it.
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
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("<p>a</p>"));
        assert!(output.contains("<p>b</p>"));
    }

    #[test]
    fn test_preserve_normal_images() {
        let input = r#"<img src="https://example.com/photo.jpg" width="800" height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("<img"));
        assert!(output.contains("/api/proxy/image?url="));
    }

    // ============ URL Tracking Parameter Tests ============

    #[test]
    fn test_strip_tracking_parameters() {
        for query in [
            "utm_source=twitter&utm_medium=social&utm_campaign=test",
            "fbclid=ABC123",
            "gclid=XYZ789",
            "msclkid=MSC456",
            "mtm_campaign=test&mtm_source=email",
        ] {
            let input = format!(r#"<a href="https://example.com/page?{query}">Link</a>"#);
            let output = sanitize_html(&input, TEST_SECRET, None, None, None);
            assert!(
                output.contains("href=\"https://example.com/page\""),
                "{query} not stripped: {output}"
            );
        }
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
        // Strips utm_*/click IDs, keeps genuine params.
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
        assert_eq!(
            strip_tracking_params("https://example.com/article?id=42&page=2"),
            "https://example.com/article?id=42&page=2"
        );
        assert_eq!(
            strip_tracking_params("https://example.com/article"),
            "https://example.com/article"
        );
    }

    #[test]
    fn test_strip_tracking_params_passes_through_non_http() {
        // Non-http(s) inputs are returned verbatim.
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
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    // ============ Relative URL Tests ============

    #[test]
    fn test_rewrite_relative_image_urls_against_the_base() {
        for (src, base) in [
            ("/images/photo.jpg", "https://example.com/article/123"),
            ("images/photo.jpg", "https://example.com/article/123"),
            ("../images/photo.jpg", "https://example.com/article/123"),
            ("/images/photo.jpg", "https://example.com/article"),
        ] {
            let input = format!(r#"<p>Text</p><img src="{src}" alt="Photo">"#);
            let output = sanitize_with_base(&input, base);
            assert!(output.contains("/api/proxy/image?url="), "{src}: {output}");
            assert!(
                !output.contains(&format!("src=\"{src}\"")),
                "{src}: {output}"
            );
        }
    }

    #[test]
    fn test_relative_images_without_base_url_are_dropped() {
        let input = r#"<p>Text</p><img src="/images/photo.jpg" alt="Photo">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        // Fail closed: the src would resolve against the wrong origin.
        assert!(!output.contains("<img"), "{output}");
        assert!(!output.contains("/images/photo.jpg"), "{output}");
        assert!(output.contains("<p>Text</p>"), "{output}");
    }

    #[test]
    fn protocol_relative_image_never_escapes_the_proxy() {
        // `//evil.tld/x.gif` is absolute to a browser; unrewritten it leaks the
        // reader's IP to the author's host.
        let input = r#"<img src="//evil.tld/x.gif">"#;

        let no_base = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(!no_base.contains("evil.tld"), "{no_base}");

        let with_base = sanitize_with_base(input, "https://feed.example/post");
        assert!(with_base.contains("/api/proxy/image?url="), "{with_base}");
        assert!(!with_base.contains(r#"src="//evil.tld"#), "{with_base}");
    }

    /// Every `src="..."` value in `html`; only `<img>` keeps `src` after
    /// sanitization.
    fn image_srcs(html: &str) -> Vec<&str> {
        html.match_indices(r#"src=""#)
            .map(|(i, m)| {
                let rest = &html[i + m.len()..];
                &rest[..rest.find('"').unwrap_or(rest.len())]
            })
            .collect()
    }

    #[test]
    fn every_emitted_image_src_is_proxied_or_inline_data() {
        // Invariant: anything that cannot become a signed proxy URL must not reach
        // the page.
        let inputs = [
            r#"<img src="//evil.tld/x.gif">"#,
            r#"<img src="/\/evil.tld/x.gif">"#,
            r#"<img src="/images/photo.jpg">"#,
            r#"<img src="images/photo.jpg">"#,
            r#"<img src="../images/photo.jpg">"#,
            r#"<img src="https://cdn.example.com/a.jpg">"#,
            r#"<img src="data:image/svg+xml,%3Csvg%3E%3C/svg%3E" data-src="/img/pic.jpg">"#,
            r#"<img src="">"#,
            r#"<img alt="no src">"#,
            r#"<figure><img src="//evil.tld/a.png"><figcaption>c</figcaption></figure>"#,
        ];
        for base in [None, Some("https://feed.example/post")] {
            for input in inputs {
                let output = sanitize_html(input, TEST_SECRET, base, None, None);
                for src in image_srcs(&output) {
                    assert!(
                        src.starts_with("/api/proxy/image?url=") || src.starts_with("data:"),
                        "unproxied src {src:?} escaped from {input:?} with base {base:?}: {output}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_rewrite_image_url_with_query_params_containing_ampersand() {
        // lol_html decodes attribute values and re-encodes on write.
        for src in [
            "https://example.com/image.jpg?size=800&format=webp",
            "https://cdn.example.com/photo?w=800&h=600",
        ] {
            let input = format!(r#"<img src="{src}" alt="Photo">"#);
            let output = sanitize_html(&input, TEST_SECRET, None, None, None);
            assert!(output.contains("/api/proxy/image?url="), "{src}: {output}");
            assert!(
                !output.contains("example.com"),
                "{src} not replaced: {output}"
            );
        }
    }

    #[test]
    fn test_mixed_absolute_and_relative_images() {
        let input = r#"<img src="https://cdn.example.com/abs.jpg"><img src="/images/rel.jpg">"#;
        let output = sanitize_with_base(input, "https://example.com/page");
        let proxy_count = output.matches("/api/proxy/image?url=").count();
        assert_eq!(proxy_count, 2);
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
        // A literal `>` in src must not mis-bound the tag.
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
    fn test_harvest_dims_from_data_original_and_style() {
        for (hints, width, height) in [
            (
                r#"data-original-width="800" data-original-height="600""#,
                800,
                600,
            ),
            (r#"style="width:320px;height:240px""#, 320, 240),
        ] {
            let input = format!(r#"<img src="https://e.com/a.jpg" {hints}>"#);
            let output = sanitize_html(&input, TEST_SECRET, None, None, None);
            assert!(output.contains(&format!("width=\"{width}\"")), "{output}");
            assert!(output.contains(&format!("height=\"{height}\"")), "{output}");
        }
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
        // VitePress/Shiki shape: gutter sibling of <pre>, hidden by a stripped class.
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

    /// Documents near the gates' edges: casing, `<image>`, hints without images,
    /// images without hints, comments, `data-original` vs `data-original-width`.
    const GATE_CORPUS: &[&str] = &[
        "",
        "<p>plain</p>",
        // Visible content included: a pure `aria-hidden` document hits the blank
        // fallback and would mask a wrongly skipped gate.
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

    /// Each gate must be a superset of its pass: when it says "skip", running the
    /// pass anyway must be a no-op.
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

    // HTML names are case-insensitive, so each gate must be too.

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
        let output = sanitize_with_base(input, "https://example.com/post");
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
        // Nothing here triggers a pre-pass; output must equal ammonia + rewrite alone.
        let input = r"<p>Plain <strong>body</strong> text.</p>";
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert_eq!(output, "<p>Plain <strong>body</strong> text.</p>");
    }
}
