use askama::Template;
// Not yet consumed by a handler — a later task renders `SummarizerCardTemplate`
// into an `Html<String>` response.
#[allow(unused_imports)]
use axum::response::Html;
use url::Url;

use crate::utils::url_validation::validate_url;

/// Maximum URLs accepted in a single summarizer run.
pub const MAX_URLS: usize = 30;

/// Parse the textarea into a validated, de-duplicated, order-preserving list of
/// URL strings. Rejects an empty list, more than `MAX_URLS`, and any line that
/// is not a fetchable http(s) URL (SSRF-validated).
#[allow(dead_code)]
pub(crate) fn parse_url_lines(input: &str) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in input.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = Url::parse(line).map_err(|_err| format!("Not a valid URL: {line}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("Only http(s) URLs are supported: {line}"));
        }
        validate_url(&parsed).map_err(|e| format!("URL not allowed ({line}): {e}"))?;
        if seen.insert(line.to_string()) {
            out.push(line.to_string());
        }
    }
    if out.is_empty() {
        return Err("Enter at least one URL.".to_string());
    }
    if out.len() > MAX_URLS {
        return Err(format!("Too many URLs — {} max per run.", MAX_URLS));
    }
    Ok(out)
}

/// Host for the card-title fallback; returns the input unchanged if unparseable.
#[allow(dead_code)]
pub(crate) fn url_host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// One URL's card. `state` selects the rendered branch; unused string fields are
/// empty. `summary` is trusted HTML/markdown from Kagi (rendered with `|safe`).
#[derive(Debug, Clone)]
pub(crate) struct SummarizerCard {
    pub index: usize,
    pub url: String,
    pub title: String,
    pub state: &'static str,
    pub summary: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "_summarizer_card_fragment.html")]
pub(crate) struct SummarizerCardTemplate {
    pub card: SummarizerCard,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trims_and_drops_blanks() {
        let out = parse_url_lines("  https://a.com/x \n\n https://b.com/y\n").unwrap();
        assert_eq!(out, vec!["https://a.com/x", "https://b.com/y"]);
    }

    #[test]
    fn dedupes_preserving_order() {
        let out = parse_url_lines("https://a.com\nhttps://a.com\nhttps://b.com").unwrap();
        assert_eq!(out, vec!["https://a.com", "https://b.com"]);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse_url_lines("   \n  ").is_err());
    }

    #[test]
    fn rejects_over_max() {
        let many = (0..(MAX_URLS + 1))
            .map(|i| format!("https://ex.com/{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_url_lines(&many).unwrap_err();
        assert!(err.contains("30"));
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(parse_url_lines("ftp://a.com/x").is_err());
        assert!(parse_url_lines("not a url").is_err());
    }

    #[test]
    fn host_fallback() {
        assert_eq!(url_host("https://news.example.net/a/b"), "news.example.net");
        assert_eq!(url_host("garbage"), "garbage");
    }
}
