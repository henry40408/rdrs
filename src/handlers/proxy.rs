use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use url::Url;

use crate::{
    AppState,
    error::{AppError, AppResult},
    services::http::{RetryConfig, SHARED_CLIENT, send_with_retry_on_error},
    services::{verify_signature, verify_signature_with_referrer},
    utils::url_validation,
};

const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

/// Fallback caching directive used only when the origin image specifies none.
const DEFAULT_CACHE_CONTROL: &str = "public, max-age=86400";

/// Pick the `Cache-Control` to send for a proxied image: mirror the origin's
/// directive when it sends a non-empty one (so an upstream `no-store`,
/// `private`, or shorter `max-age` wins), otherwise fall back to a 1-day
/// public TTL.
fn choose_cache_control(origin: Option<&str>) -> &str {
    origin
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_CACHE_CONTROL)
}

#[derive(Deserialize)]
pub struct ProxyQuery {
    url: String,
    s: String,
    r: Option<String>,
}

/// Decodes a base64url-encoded referrer parameter.
fn decode_referrer(encoded: &str) -> AppResult<String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AppError::InvalidImageUrl)?;
    String::from_utf8(bytes).map_err(|_| AppError::InvalidImageUrl)
}

/// Verifies the proxy URL signature, accounting for optional referrer.
fn verify_proxy_signature(
    url: &str,
    signature: &str,
    referrer: Option<&str>,
    secret: &[u8],
) -> bool {
    if let Some(referrer) = referrer {
        verify_signature_with_referrer(url, referrer, signature, secret)
    } else {
        verify_signature(url, signature, secret)
    }
}

pub async fn proxy_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ProxyQuery>,
) -> AppResult<Response> {
    // A proxied image is immutable for a given URL, and the request signature
    // `s` is a stable per-URL token — so it doubles as the ETag. When the
    // browser revalidates a cached image it sends `If-None-Match`; answer 304
    // immediately and skip the origin round-trip entirely. This mirrors
    // miniflux's media proxy and is what makes a refresh / post-TTL revisit
    // cheap instead of re-downloading every image from origin.
    let etag = format!("\"{}\"", query.s);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v == etag || v == "*")
        .unwrap_or(false)
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, DEFAULT_CACHE_CONTROL.to_string()),
            ],
        )
            .into_response());
    }

    // Decode the base64 URL
    let url_bytes = URL_SAFE_NO_PAD
        .decode(&query.url)
        .map_err(|_| AppError::InvalidImageUrl)?;
    let url_str = String::from_utf8(url_bytes).map_err(|_| AppError::InvalidImageUrl)?;

    // Decode referrer if present
    let referrer = query.r.as_deref().map(decode_referrer).transpose()?;

    // Verify signature (with or without referrer)
    if !verify_proxy_signature(
        &url_str,
        &query.s,
        referrer.as_deref(),
        &state.config.image_proxy_secret,
    ) {
        return Err(AppError::InvalidSignature);
    }

    // Parse and validate the URL
    let url = Url::parse(&url_str).map_err(|_| AppError::InvalidImageUrl)?;
    url_validation::validate_url(&url).map_err(|_| AppError::InvalidImageUrl)?;

    // Fetch the image through the shared, connection-pooled client.
    let url_str = url.to_string();
    let user_agent = state.config.user_agent.clone();
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        let mut req = SHARED_CLIENT
            .get(&url_str)
            .header("User-Agent", &user_agent);
        if let Some(ref referrer) = referrer {
            req = req.header("Referer", referrer.as_str());
        }
        req
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

    // Mirror the origin's caching directive when it sends one (see
    // `choose_cache_control`), else apply our default TTL.
    let cache_control = choose_cache_control(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
    )
    .to_string();

    // Check Content-Length if available
    if let Some(content_length) = response.content_length()
        && content_length > MAX_IMAGE_SIZE
    {
        return Err(AppError::ImageTooLarge);
    }

    // Read the body with size limit
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::ImageFetchError(e.to_string()))?;

    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(AppError::ImageTooLarge);
    }

    // Return the image with appropriate headers. The ETag lets the browser
    // revalidate cheaply on its next visit (see the `If-None-Match` 304
    // short-circuit at the top of this handler).
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response())
}

fn is_valid_image_type(content_type: &str) -> bool {
    let ct = content_type.to_lowercase();
    ct.starts_with("image/") || ct == "application/octet-stream" // Some servers don't set proper content-type
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{sign_url, sign_url_with_referrer};

    #[test]
    fn test_decode_referrer_valid() {
        let referrer = "https://example.com";
        let encoded = URL_SAFE_NO_PAD.encode(referrer);
        let decoded = decode_referrer(&encoded).unwrap();
        assert_eq!(decoded, referrer);
    }

    #[test]
    fn test_decode_referrer_invalid_base64() {
        let result = decode_referrer("!!!invalid!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_referrer_invalid_utf8() {
        // Encode invalid UTF-8 bytes
        let encoded = URL_SAFE_NO_PAD.encode([0xff, 0xfe]);
        let result = decode_referrer(&encoded);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_proxy_signature_without_referrer() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let sig = sign_url(url, secret);

        assert!(verify_proxy_signature(url, &sig, None, secret));
        assert!(!verify_proxy_signature(url, "invalid", None, secret));
    }

    #[test]
    fn test_verify_proxy_signature_with_referrer() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";
        let sig = sign_url_with_referrer(url, referrer, secret);

        assert!(verify_proxy_signature(url, &sig, Some(referrer), secret));
        assert!(!verify_proxy_signature(
            url,
            &sig,
            Some("https://other.com"),
            secret
        ));
    }

    #[test]
    fn test_verify_proxy_signature_referrer_mismatch() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";

        // Signature with referrer should not pass without referrer
        let sig_with_ref = sign_url_with_referrer(url, referrer, secret);
        assert!(!verify_proxy_signature(url, &sig_with_ref, None, secret));

        // Signature without referrer should not pass with referrer
        let sig_no_ref = sign_url(url, secret);
        assert!(!verify_proxy_signature(
            url,
            &sig_no_ref,
            Some(referrer),
            secret
        ));
    }

    #[test]
    fn test_choose_cache_control_mirrors_origin() {
        // Origin directive wins verbatim.
        assert_eq!(choose_cache_control(Some("max-age=3600")), "max-age=3600");
        assert_eq!(choose_cache_control(Some("no-store")), "no-store");
        assert_eq!(
            choose_cache_control(Some("private, max-age=0")),
            "private, max-age=0"
        );
        // Surrounding whitespace is trimmed.
        assert_eq!(choose_cache_control(Some("  no-cache  ")), "no-cache");
    }

    #[test]
    fn test_choose_cache_control_falls_back_when_absent() {
        assert_eq!(choose_cache_control(None), DEFAULT_CACHE_CONTROL);
        assert_eq!(choose_cache_control(Some("")), DEFAULT_CACHE_CONTROL);
        assert_eq!(choose_cache_control(Some("   ")), DEFAULT_CACHE_CONTROL);
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
