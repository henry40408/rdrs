use ammonia::Builder;
use scraper::{Html, Selector};
use std::collections::HashSet;
use url::Url;

use super::image_proxy::create_proxy_url;

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

/// Remove tracking pixels (1x1 images, zero-dimension images, and images from tracking domains)
fn remove_tracking_pixels(html: &str) -> String {
    // Build a list of src URLs to remove based on tracking criteria
    let document = Html::parse_fragment(html);
    let img_selector = Selector::parse("img").unwrap();

    let mut urls_to_remove: Vec<String> = Vec::new();

    for element in document.select(&img_selector) {
        let attrs = element.value();

        // Check for 1x1 pixel or zero dimensions
        let width = attrs.attr("width");
        let height = attrs.attr("height");

        let is_tracking_size = match (width, height) {
            (Some(w), Some(h)) => w == "1" && h == "1",
            (Some(w), None) => w == "0",
            (None, Some(h)) => h == "0",
            _ => false,
        };

        // Check for tracking domain or path in src
        let is_tracking_url = if let Some(src) = attrs.attr("src") {
            let src_lower = src.to_lowercase();
            TRACKING_DOMAINS.iter().any(|d| src_lower.contains(d))
                || TRACKING_PATHS.iter().any(|p| src_lower.contains(p))
        } else {
            false
        };

        if is_tracking_size || is_tracking_url {
            if let Some(src) = attrs.attr("src") {
                urls_to_remove.push(src.to_string());
            }
        }
    }

    // Remove img tags with matching src URLs
    let mut result = html.to_string();
    for url in &urls_to_remove {
        // Find img tags containing this URL and remove them
        result = remove_img_tag_with_src(&result, url);
    }

    result
}

/// Remove an img tag that contains the specified src URL
fn remove_img_tag_with_src(html: &str, src_url: &str) -> String {
    let mut result = String::new();
    let mut i = 0;

    while i < html.len() {
        // Check if we're at the start of an img tag
        if html[i..].starts_with("<img") {
            // Find the end of this tag
            if let Some(end_pos) = html[i..].find('>') {
                let tag_end = i + end_pos + 1;
                let tag = &html[i..tag_end];

                // Check if this img tag contains our target src URL
                let src_patterns = [format!("src=\"{}\"", src_url), format!("src='{}'", src_url)];

                let should_remove = src_patterns.iter().any(|p| tag.contains(p));

                if should_remove {
                    // Skip this tag entirely
                    i = tag_end;
                    continue;
                }
            }
        }

        // Add current character to result
        if let Some(c) = html[i..].chars().next() {
            result.push(c);
            i += c.len_utf8();
        } else {
            break;
        }
    }

    result
}

/// Check if a parameter name is a tracking parameter
fn is_tracking_param(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    TRACKING_PARAMS.iter().any(|&p| name_lower == p)
        || TRACKING_PARAM_PREFIXES
            .iter()
            .any(|p| name_lower.starts_with(p))
}

/// Strip tracking parameters from all URLs in anchor tags
fn strip_tracking_params(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let a_selector = Selector::parse("a[href]").unwrap();

    let mut result = html.to_string();

    for element in document.select(&a_selector) {
        if let Some(href) = element.value().attr("href") {
            // Only process http/https URLs
            if !href.starts_with("http://") && !href.starts_with("https://") {
                continue;
            }

            if let Ok(mut url) = Url::parse(href) {
                let original_query: Vec<(String, String)> = url
                    .query_pairs()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();

                // Filter out tracking parameters
                let filtered_query: Vec<(String, String)> = original_query
                    .iter()
                    .filter(|(k, _)| !is_tracking_param(k))
                    .cloned()
                    .collect();

                // Only modify if we actually removed something
                if filtered_query.len() < original_query.len() {
                    // Clear and rebuild query string
                    url.set_query(None);
                    if !filtered_query.is_empty() {
                        let query_string: String = filtered_query
                            .iter()
                            .map(|(k, v)| {
                                format!(
                                    "{}={}",
                                    url::form_urlencoded::byte_serialize(k.as_bytes())
                                        .collect::<String>(),
                                    url::form_urlencoded::byte_serialize(v.as_bytes())
                                        .collect::<String>()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("&");
                        url.set_query(Some(&query_string));
                    }

                    // Build new URL with &amp; for HTML context
                    let new_url = url.as_str().replace('&', "&amp;");

                    // Try both & and &amp; versions for matching (ammonia encodes & to &amp;)
                    let old_attr_amp = format!("href=\"{}\"", href.replace('&', "&amp;"));
                    let old_attr_raw = format!("href=\"{}\"", href);
                    let new_attr = format!("href=\"{}\"", new_url);

                    // Try the &amp; version first (more common after ammonia)
                    if result.contains(&old_attr_amp) {
                        result = result.replacen(&old_attr_amp, &new_attr, 1);
                    } else {
                        result = result.replacen(&old_attr_raw, &new_attr, 1);
                    }
                }
            }
        }
    }

    result
}

pub fn sanitize_html(content: &str, secret: &[u8]) -> String {
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

    // Step 1: Ammonia sanitization (already adds rel="noopener noreferrer")
    let sanitized = Builder::default()
        .tags(allowed_tags)
        .link_rel(Some("noopener noreferrer"))
        .url_schemes(url_schemes)
        .clean(content)
        .to_string();

    // Step 2: Remove tracking pixels
    let without_pixels = remove_tracking_pixels(&sanitized);

    // Step 3: Strip tracking parameters from URLs
    let without_tracking = strip_tracking_params(&without_pixels);

    // Step 4: Rewrite image URLs to proxy
    let with_images = rewrite_image_urls(&without_tracking, secret);

    // Step 5: Add privacy attributes to links
    add_privacy_attrs_to_links(&with_images)
}

pub fn rewrite_image_urls(html: &str, secret: &[u8]) -> String {
    let document = Html::parse_fragment(html);
    let img_selector = Selector::parse("img[src]").unwrap();

    let mut result = html.to_string();

    for element in document.select(&img_selector) {
        if let Some(src) = element.value().attr("src") {
            // Only rewrite http/https URLs, skip data: URLs
            if src.starts_with("http://") || src.starts_with("https://") {
                let proxy_url = create_proxy_url(src, secret);

                // Replace the original src with the proxy URL and add lazy loading
                let old_attr = format!("src=\"{}\"", src);
                let new_attr = format!("src=\"{}\" loading=\"lazy\" decoding=\"async\"", proxy_url);
                result = result.replacen(&old_attr, &new_attr, 1);
            }
        }
    }

    result
}

/// Add target="_blank" and referrerpolicy="no-referrer" to all external links
fn add_privacy_attrs_to_links(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let a_selector = Selector::parse("a[href]").unwrap();

    let mut result = html.to_string();

    for element in document.select(&a_selector) {
        if let Some(href) = element.value().attr("href") {
            // Only process http/https links (external links)
            if href.starts_with("http://") || href.starts_with("https://") {
                let old_attr = format!("href=\"{}\"", href);
                let new_attr = format!(
                    "href=\"{}\" target=\"_blank\" referrerpolicy=\"no-referrer\"",
                    href
                );
                result = result.replacen(&old_attr, &new_attr, 1);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &[u8] = b"test_secret_key_32_bytes_long!!!";

    #[test]
    fn test_sanitize_basic_html() {
        let input = "<p>Hello <strong>world</strong></p>";
        let output = sanitize_html(input, TEST_SECRET);
        assert_eq!(output, "<p>Hello <strong>world</strong></p>");
    }

    #[test]
    fn test_remove_script_tags() {
        let input = "<p>Hello</p><script>alert('xss')</script>";
        let output = sanitize_html(input, TEST_SECRET);
        assert!(!output.contains("script"));
        assert!(output.contains("<p>Hello</p>"));
    }

    #[test]
    fn test_preserve_links() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(output.contains("href=\"https://example.com\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_remove_javascript_urls() {
        let input = r#"<a href="javascript:alert('xss')">Click</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(!output.contains("javascript"));
    }

    #[test]
    fn test_preserve_images() {
        let input = r#"<img src="https://example.com/image.jpg" alt="Image">"#;
        let output = sanitize_html(input, TEST_SECRET);
        // Image URLs should be rewritten to proxy URLs with signature
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("&s="));
        assert!(!output.contains("src=\"https://example.com/image.jpg\""));
    }

    #[test]
    fn test_rewrite_image_urls() {
        let input = r#"<p>Text</p><img src="https://example.com/image.jpg" alt="Image">"#;
        let output = rewrite_image_urls(input, TEST_SECRET);
        assert!(output.contains("/api/proxy/image?url="));
        assert!(output.contains("&s="));
        assert!(!output.contains("src=\"https://example.com/image.jpg\""));
    }

    #[test]
    fn test_rewrite_preserves_data_urls() {
        let input = r#"<img src="data:image/png;base64,abc123" alt="Data URL">"#;
        let output = rewrite_image_urls(input, TEST_SECRET);
        assert!(output.contains("data:image/png;base64,abc123"));
        assert!(!output.contains("/api/proxy/image"));
    }

    #[test]
    fn test_rewrite_multiple_images() {
        let input = r#"<img src="https://a.com/1.jpg"><img src="https://b.com/2.jpg">"#;
        let output = rewrite_image_urls(input, TEST_SECRET);
        assert!(!output.contains("src=\"https://a.com/1.jpg\""));
        assert!(!output.contains("src=\"https://b.com/2.jpg\""));
        // Both should be rewritten with signatures
        let proxy_count = output.matches("/api/proxy/image?url=").count();
        assert_eq!(proxy_count, 2);
        let sig_count = output.matches("&s=").count();
        assert_eq!(sig_count, 2);
    }

    #[test]
    fn test_links_have_target_blank() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(output.contains("target=\"_blank\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_multiple_links_have_target_blank() {
        let input = r#"<a href="https://a.com">A</a><a href="https://b.com">B</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        let target_count = output.matches("target=\"_blank\"").count();
        assert_eq!(target_count, 2);
    }

    #[test]
    fn test_relative_links_no_target_blank() {
        let input = r#"<a href="/local/path">Local</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(!output.contains("target=\"_blank\""));
    }

    // ============ Tracking Pixel Removal Tests ============

    #[test]
    fn test_remove_1x1_pixel_images() {
        let input = r#"<p>Text</p><img src="https://example.com/pixel.gif" width="1" height="1">"#;
        let output = remove_tracking_pixels(input);
        assert!(!output.contains("<img"));
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_remove_zero_dimension_images() {
        let input = r#"<img src="https://example.com/hidden.gif" width="0">"#;
        let output = remove_tracking_pixels(input);
        assert!(!output.contains("<img"));

        let input2 = r#"<img src="https://example.com/hidden.gif" height="0">"#;
        let output2 = remove_tracking_pixels(input2);
        assert!(!output2.contains("<img"));
    }

    #[test]
    fn test_remove_tracking_domain_images() {
        let input1 = r#"<img src="https://pixel.example.com/track.gif">"#;
        let output1 = remove_tracking_pixels(input1);
        assert!(!output1.contains("<img"));

        let input2 = r#"<img src="https://beacon.example.com/img.gif">"#;
        let output2 = remove_tracking_pixels(input2);
        assert!(!output2.contains("<img"));

        let input3 = r#"<img src="https://track.example.com/img.gif">"#;
        let output3 = remove_tracking_pixels(input3);
        assert!(!output3.contains("<img"));

        let input4 = r#"<img src="https://analytics.example.com/img.gif">"#;
        let output4 = remove_tracking_pixels(input4);
        assert!(!output4.contains("<img"));
    }

    #[test]
    fn test_remove_tracking_path_images() {
        let input1 = r#"<img src="https://example.com/pixel/tracker.gif">"#;
        let output1 = remove_tracking_pixels(input1);
        assert!(!output1.contains("<img"));

        let input2 = r#"<img src="https://example.com/beacon/img.gif">"#;
        let output2 = remove_tracking_pixels(input2);
        assert!(!output2.contains("<img"));

        let input3 = r#"<img src="https://example.com/1x1.gif">"#;
        let output3 = remove_tracking_pixels(input3);
        assert!(!output3.contains("<img"));
    }

    #[test]
    fn test_preserve_normal_images() {
        let input = r#"<img src="https://example.com/photo.jpg" width="800" height="600">"#;
        let output = remove_tracking_pixels(input);
        assert!(output.contains("<img"));
        assert!(output.contains("photo.jpg"));
    }

    // ============ URL Tracking Parameter Tests ============

    #[test]
    fn test_strip_utm_parameters() {
        let input = r#"<a href="https://example.com/page?utm_source=twitter&utm_medium=social&utm_campaign=test">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("utm_source"));
        assert!(!output.contains("utm_medium"));
        assert!(!output.contains("utm_campaign"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_facebook_click_id() {
        let input = r#"<a href="https://example.com/page?fbclid=ABC123">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("fbclid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_google_click_id() {
        let input = r#"<a href="https://example.com/page?gclid=XYZ789">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("gclid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_strip_microsoft_click_id() {
        let input = r#"<a href="https://example.com/page?msclkid=MSC456">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("msclkid"));
        assert!(output.contains("href=\"https://example.com/page\""));
    }

    #[test]
    fn test_preserve_non_tracking_parameters() {
        let input = r#"<a href="https://example.com/search?q=rust&page=2">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(output.contains("q=rust"));
        assert!(output.contains("page=2"));
    }

    #[test]
    fn test_strip_multiple_tracking_params() {
        let input = r#"<a href="https://example.com/page?id=123&fbclid=FB1&gclid=GC1&utm_source=test&valid=yes">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("fbclid"));
        assert!(!output.contains("gclid"));
        assert!(!output.contains("utm_source"));
        assert!(output.contains("id=123"));
        assert!(output.contains("valid=yes"));
    }

    #[test]
    fn test_preserve_url_without_params() {
        let input = r#"<a href="https://example.com/page">Link</a>"#;
        let output = strip_tracking_params(input);
        assert_eq!(input, output);
    }

    #[test]
    fn test_strip_matomo_params() {
        let input =
            r#"<a href="https://example.com/page?mtm_campaign=test&mtm_source=email">Link</a>"#;
        let output = strip_tracking_params(input);
        assert!(!output.contains("mtm_campaign"));
        assert!(!output.contains("mtm_source"));
    }

    // ============ Referrer Policy Tests ============

    #[test]
    fn test_links_have_referrerpolicy() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(output.contains("referrerpolicy=\"no-referrer\""));
        assert!(output.contains("target=\"_blank\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_multiple_links_have_referrerpolicy() {
        let input = r#"<a href="https://a.com">A</a><a href="https://b.com">B</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        let policy_count = output.matches("referrerpolicy=\"no-referrer\"").count();
        assert_eq!(policy_count, 2);
    }

    // ============ Integration Tests ============

    #[test]
    fn test_sanitize_removes_tracking_pixels() {
        let input =
            r#"<p>Text</p><img src="https://pixel.tracker.com/img.gif" width="1" height="1">"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(!output.contains("pixel.tracker.com"));
        assert!(output.contains("<p>Text</p>"));
    }

    #[test]
    fn test_sanitize_strips_tracking_params() {
        let input = r#"<a href="https://example.com/page?utm_source=test&id=123">Link</a>"#;
        let output = sanitize_html(input, TEST_SECRET);
        assert!(!output.contains("utm_source"));
        assert!(output.contains("id=123"));
    }
}
