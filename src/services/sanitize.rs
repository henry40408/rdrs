use ammonia::Builder;
use std::collections::HashSet;

pub fn sanitize_html(content: &str) -> String {
    let allowed_tags: HashSet<&str> = [
        "p", "br", "a", "strong", "em", "b", "i", "ul", "ol", "li", "blockquote", "pre", "code",
        "img", "h1", "h2", "h3", "h4", "h5", "h6", "div", "span", "figure", "figcaption",
        "table", "thead", "tbody", "tr", "th", "td",
    ]
    .iter()
    .copied()
    .collect();

    let url_schemes: HashSet<&str> = ["http", "https"].iter().copied().collect();

    Builder::default()
        .tags(allowed_tags)
        .link_rel(Some("noopener noreferrer"))
        .url_schemes(url_schemes)
        .clean(content)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_basic_html() {
        let input = "<p>Hello <strong>world</strong></p>";
        let output = sanitize_html(input);
        assert_eq!(output, "<p>Hello <strong>world</strong></p>");
    }

    #[test]
    fn test_remove_script_tags() {
        let input = "<p>Hello</p><script>alert('xss')</script>";
        let output = sanitize_html(input);
        assert!(!output.contains("script"));
        assert!(output.contains("<p>Hello</p>"));
    }

    #[test]
    fn test_preserve_links() {
        let input = r#"<a href="https://example.com">Link</a>"#;
        let output = sanitize_html(input);
        assert!(output.contains("href=\"https://example.com\""));
        assert!(output.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn test_remove_javascript_urls() {
        let input = r#"<a href="javascript:alert('xss')">Click</a>"#;
        let output = sanitize_html(input);
        assert!(!output.contains("javascript"));
    }

    #[test]
    fn test_preserve_images() {
        let input = r#"<img src="https://example.com/image.jpg" alt="Image">"#;
        let output = sanitize_html(input);
        assert!(output.contains("src=\"https://example.com/image.jpg\""));
    }
}
