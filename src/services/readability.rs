use std::net::IpAddr;
use std::time::Duration;

use readability::extractor;
use url::Url;

use crate::error::{AppError, AppResult};

pub struct ExtractedContent {
    pub title: Option<String>,
    pub content: String,
}

/// Fetches HTML from URL and extracts readable content using readability crate.
pub async fn fetch_and_extract(url: &str, user_agent: &str) -> AppResult<ExtractedContent> {
    // Parse and validate URL (SSRF protection)
    let parsed_url = Url::parse(url).map_err(|_| AppError::InvalidUrl)?;
    validate_url(&parsed_url)?;

    // Fetch HTML using existing reqwest (rustls-tls)
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(user_agent)
        .build()
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AppError::FetchError(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let html = response
        .text()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    // Extract readable content
    let product = extractor::extract(&mut html.as_bytes(), &parsed_url)
        .map_err(|e| AppError::FetchError(format!("Failed to extract content: {}", e)))?;

    Ok(ExtractedContent {
        title: Some(product.title).filter(|t| !t.is_empty()),
        content: product.content,
    })
}

/// Validates URL to prevent SSRF attacks.
fn validate_url(url: &Url) -> AppResult<()> {
    // Only allow http/https schemes
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(AppError::InvalidUrl),
    }

    // Get the host
    let host = url.host_str().ok_or(AppError::InvalidUrl)?;

    // Block localhost and loopback addresses
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return Err(AppError::InvalidUrl);
    }

    // Block .local and .internal domains
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(AppError::InvalidUrl);
    }

    // Try to parse as IP address and check for private ranges
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(AppError::InvalidUrl);
        }
    }

    // Also check if it's an IPv6 address in brackets
    if host.starts_with('[') && host.ends_with(']') {
        if let Ok(ip) = host[1..host.len() - 1].parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return Err(AppError::InvalidUrl);
            }
        }
    }

    Ok(())
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
                || ipv4.is_unspecified()
        }
        IpAddr::V6(ipv6) => ipv6.is_loopback() || ipv6.is_unspecified(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_valid() {
        let url = Url::parse("https://example.com/article").unwrap();
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_validate_url_localhost() {
        let url = Url::parse("http://localhost/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_loopback() {
        let url = Url::parse("http://127.0.0.1/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_private_ip() {
        let url = Url::parse("http://192.168.1.1/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_file_scheme() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(validate_url(&url).is_err());
    }
}
