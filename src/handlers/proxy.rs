use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use std::net::IpAddr;
use url::Url;

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    services::http::{send_with_retry, RetryConfig, DEFAULT_TIMEOUT},
    services::verify_signature,
    AppState,
};

const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

#[derive(Deserialize)]
pub struct ProxyQuery {
    url: String,
    s: String,
}

pub async fn proxy_image(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(query): Query<ProxyQuery>,
) -> AppResult<Response> {
    // Decode the base64 URL
    let url_bytes = URL_SAFE_NO_PAD
        .decode(&query.url)
        .map_err(|_| AppError::InvalidImageUrl)?;
    let url_str = String::from_utf8(url_bytes).map_err(|_| AppError::InvalidImageUrl)?;

    // Verify signature
    if !verify_signature(&url_str, &query.s, &state.config.image_proxy_secret) {
        return Err(AppError::InvalidSignature);
    }

    // Parse and validate the URL
    let url = Url::parse(&url_str).map_err(|_| AppError::InvalidImageUrl)?;
    validate_url(&url)?;

    // Fetch the image
    let client = reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(|e| AppError::ImageFetchError(e.to_string()))?;

    let url_str = url.to_string();
    let user_agent = state.config.user_agent.clone();
    let response = send_with_retry(&RetryConfig::default(), || {
        client.get(&url_str).header("User-Agent", &user_agent)
    })
    .await
    .map_err(|e| AppError::ImageFetchError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AppError::ImageFetchError(format!(
            "HTTP {}",
            response.status()
        )));
    }

    // Validate Content-Type
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    if !is_valid_image_type(&content_type) {
        return Err(AppError::UnsupportedImageType);
    }

    // Check Content-Length if available
    if let Some(content_length) = response.content_length() {
        if content_length > MAX_IMAGE_SIZE {
            return Err(AppError::ImageTooLarge);
        }
    }

    // Read the body with size limit
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::ImageFetchError(e.to_string()))?;

    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(AppError::ImageTooLarge);
    }

    // Return the image with appropriate headers
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        bytes,
    )
        .into_response())
}

fn validate_url(url: &Url) -> AppResult<()> {
    // Only allow http/https schemes
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(AppError::InvalidImageUrl),
    }

    // Get the host
    let host = url.host_str().ok_or(AppError::InvalidImageUrl)?;

    // Block localhost and loopback addresses
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return Err(AppError::InvalidImageUrl);
    }

    // Block .local and .internal domains
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(AppError::InvalidImageUrl);
    }

    // Try to parse as IP address and check for private ranges
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(AppError::InvalidImageUrl);
        }
    }

    // Also check if it's an IPv6 address in brackets
    if host.starts_with('[') && host.ends_with(']') {
        if let Ok(ip) = host[1..host.len() - 1].parse::<IpAddr>() {
            if is_private_ip(&ip) {
                return Err(AppError::InvalidImageUrl);
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

fn is_valid_image_type(content_type: &str) -> bool {
    let ct = content_type.to_lowercase();
    ct.starts_with("image/") || ct == "application/octet-stream" // Some servers don't set proper content-type
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_valid() {
        let url = Url::parse("https://example.com/image.jpg").unwrap();
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_validate_url_localhost() {
        let url = Url::parse("http://localhost/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_loopback() {
        let url = Url::parse("http://127.0.0.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_private_10() {
        let url = Url::parse("http://10.0.0.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_private_172() {
        let url = Url::parse("http://172.16.0.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_private_192() {
        let url = Url::parse("http://192.168.1.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_local_domain() {
        let url = Url::parse("http://myhost.local/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_internal_domain() {
        let url = Url::parse("http://server.internal/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_ftp_scheme() {
        let url = Url::parse("ftp://example.com/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_file_scheme() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_is_valid_image_type() {
        assert!(is_valid_image_type("image/jpeg"));
        assert!(is_valid_image_type("image/png"));
        assert!(is_valid_image_type("image/gif"));
        assert!(is_valid_image_type("image/webp"));
        assert!(is_valid_image_type("IMAGE/JPEG"));
        assert!(is_valid_image_type("application/octet-stream"));
        assert!(!is_valid_image_type("text/html"));
        assert!(!is_valid_image_type("application/javascript"));
    }
}
