use ammonia::Builder;
use scraper::{Html, Selector};
use std::collections::HashSet;

use super::image_proxy::create_proxy_url;

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

    let sanitized = Builder::default()
        .tags(allowed_tags)
        .link_rel(Some("noopener noreferrer"))
        .url_schemes(url_schemes)
        .clean(content)
        .to_string();

    rewrite_image_urls(&sanitized, secret)
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

                // Replace the original src with the proxy URL
                let old_attr = format!("src=\"{}\"", src);
                let new_attr = format!("src=\"{}\"", proxy_url);
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
}
