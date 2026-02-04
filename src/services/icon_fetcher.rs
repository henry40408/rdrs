use reqwest::header::CONTENT_TYPE;
use tracing::debug;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::services::http::{send_with_retry, RetryConfig, ICON_TIMEOUT};

const MAX_ICON_SIZE: usize = 256 * 1024; // 256KB

pub struct FetchedImage {
    pub data: Vec<u8>,
    pub content_type: String,
    pub source_url: String,
}

pub async fn fetch_feed_icon(
    icon_url: Option<&str>,
    logo_url: Option<&str>,
    site_url: Option<&str>,
    user_agent: &str,
) -> AppResult<Option<FetchedImage>> {
    // Try icon_url first
    if let Some(url) = icon_url {
        if let Ok(Some(img)) = fetch_image(url, user_agent).await {
            debug!("Fetched icon from feed icon_url: {}", url);
            return Ok(Some(img));
        }
    }

    // Try logo_url
    if let Some(url) = logo_url {
        if let Ok(Some(img)) = fetch_image(url, user_agent).await {
            debug!("Fetched icon from feed logo_url: {}", url);
            return Ok(Some(img));
        }
    }

    // Fallback to favicon
    if let Some(url) = site_url {
        if let Ok(Some(img)) = fetch_favicon(url, user_agent).await {
            return Ok(Some(img));
        }
    }

    Ok(None)
}

async fn fetch_image(url: &str, user_agent: &str) -> AppResult<Option<FetchedImage>> {
    let client = reqwest::Client::builder()
        .timeout(ICON_TIMEOUT)
        .user_agent(user_agent)
        .build()
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    let retry_config = RetryConfig::icon();
    let url_owned = url.to_string();

    let response = match send_with_retry(&retry_config, || client.get(&url_owned)).await {
        Ok(r) => r,
        Err(e) => {
            debug!("Failed to fetch image from {}: {}", url, e);
            return Ok(None);
        }
    };

    if !response.status().is_success() {
        debug!("Non-success status {} for {}", response.status(), url);
        return Ok(None);
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .unwrap_or_default();

    // Validate content type is an image
    if !content_type.starts_with("image/") {
        debug!("Invalid content type {} for {}", content_type, url);
        return Ok(None);
    }

    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            debug!("Failed to read bytes from {}: {}", url, e);
            return Ok(None);
        }
    };

    if bytes.len() > MAX_ICON_SIZE {
        debug!("Image too large ({} bytes) from {}", bytes.len(), url);
        return Ok(None);
    }

    if bytes.is_empty() {
        debug!("Empty image from {}", url);
        return Ok(None);
    }

    Ok(Some(FetchedImage {
        data: bytes.to_vec(),
        content_type,
        source_url: url.to_string(),
    }))
}

async fn fetch_favicon(site_url: &str, user_agent: &str) -> AppResult<Option<FetchedImage>> {
    let base_url = match Url::parse(site_url) {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    // Try /favicon.ico first
    let favicon_url = format!(
        "{}://{}/favicon.ico",
        base_url.scheme(),
        base_url.host_str().unwrap_or("")
    );
    if let Ok(Some(img)) = fetch_image(&favicon_url, user_agent).await {
        debug!("Fetched favicon from {}", favicon_url);
        return Ok(Some(img));
    }

    // Try parsing HTML for link rel="icon"
    let client = reqwest::Client::builder()
        .timeout(ICON_TIMEOUT)
        .user_agent(user_agent)
        .build()
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    let retry_config = RetryConfig::icon();
    let site_url_owned = site_url.to_string();

    let html = match send_with_retry(&retry_config, || client.get(&site_url_owned)).await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(t) => t,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };

    if let Some(icon_url) = extract_favicon_from_html(&html, &base_url) {
        if let Ok(Some(img)) = fetch_image(&icon_url, user_agent).await {
            debug!("Fetched favicon from HTML link: {}", icon_url);
            return Ok(Some(img));
        }
    }

    Ok(None)
}

fn extract_favicon_from_html(html: &str, base_url: &Url) -> Option<String> {
    let html_lower = html.to_lowercase();

    // Look for <link rel="icon" or <link rel="shortcut icon"
    for pattern in &[
        "rel=\"icon\"",
        "rel='icon'",
        "rel=\"shortcut icon\"",
        "rel='shortcut icon'",
    ] {
        if let Some(link_pos) = html_lower.find(pattern) {
            // Find the start of this <link> tag
            let tag_start = html_lower[..link_pos].rfind("<link")?;
            // Find the end of this tag
            let tag_end = html_lower[tag_start..].find('>')? + tag_start;
            let tag = &html[tag_start..=tag_end];

            // Extract href
            if let Some(href) = extract_href(tag) {
                return resolve_url(&href, base_url);
            }
        }
    }

    None
}

fn extract_href(tag: &str) -> Option<String> {
    let tag_lower = tag.to_lowercase();

    for prefix in &["href=\"", "href='"] {
        if let Some(start) = tag_lower.find(prefix) {
            let quote = if prefix.ends_with('"') { '"' } else { '\'' };
            let value_start = start + prefix.len();
            let value_end = tag[value_start..].find(quote)?;
            return Some(tag[value_start..value_start + value_end].to_string());
        }
    }

    None
}

fn resolve_url(href: &str, base_url: &Url) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }

    if href.starts_with("//") {
        return Some(format!("{}:{}", base_url.scheme(), href));
    }

    base_url.join(href).ok().map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_href() {
        assert_eq!(
            extract_href(r#"<link rel="icon" href="/favicon.ico">"#),
            Some("/favicon.ico".to_string())
        );
        assert_eq!(
            extract_href(r#"<link href='/icon.png' rel='icon'>"#),
            Some("/icon.png".to_string())
        );
    }

    #[test]
    fn test_resolve_url() {
        let base = Url::parse("https://example.com/path/page").unwrap();

        assert_eq!(
            resolve_url("/favicon.ico", &base),
            Some("https://example.com/favicon.ico".to_string())
        );
        assert_eq!(
            resolve_url("icon.png", &base),
            Some("https://example.com/path/icon.png".to_string())
        );
        assert_eq!(
            resolve_url("//cdn.example.com/icon.png", &base),
            Some("https://cdn.example.com/icon.png".to_string())
        );
        assert_eq!(
            resolve_url("https://other.com/icon.png", &base),
            Some("https://other.com/icon.png".to_string())
        );
    }

    #[test]
    fn test_extract_favicon_from_html() {
        let base = Url::parse("https://example.com").unwrap();

        let html = r#"
            <html>
            <head>
                <link rel="icon" href="/static/favicon.ico">
            </head>
            </html>
        "#;
        assert_eq!(
            extract_favicon_from_html(html, &base),
            Some("https://example.com/static/favicon.ico".to_string())
        );

        let html2 = r#"<link rel="shortcut icon" href="https://cdn.example.com/icon.png">"#;
        assert_eq!(
            extract_favicon_from_html(html2, &base),
            Some("https://cdn.example.com/icon.png".to_string())
        );
    }
}
