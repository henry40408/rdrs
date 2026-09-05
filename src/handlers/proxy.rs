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
    services::http::{RetryConfig, send_with_retry_on_error},
    services::{verify_signature, verify_signature_with_referrer},
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
        .map_err(|_e| AppError::InvalidImageUrl)?;
    String::from_utf8(bytes).map_err(|_e| AppError::InvalidImageUrl)
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

/// The 304 answer to a revalidation request, or `None` if the client did not
/// send a matching validator.
///
/// `signature` reaches the response as an `ETag`, so it goes through
/// `HeaderValue::from_str` rather than being formatted straight into the header
/// list: a percent-decoded CR/LF in the query would otherwise fail deeper in the
/// response path, where there is no longer a way to answer the request.
fn not_modified(headers: &HeaderMap, signature: &str) -> Option<Response> {
    let etag = format!("\"{signature}\"");

    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag || v == "*");
    if !matches {
        return None;
    }

    let etag = header::HeaderValue::from_str(&etag).ok()?;

    Some(
        (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static(DEFAULT_CACHE_CONTROL),
                ),
            ],
        )
            .into_response(),
    )
}

pub async fn proxy_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ProxyQuery>,
) -> AppResult<Response> {
    // Decode the base64 URL
    let url_bytes = URL_SAFE_NO_PAD
        .decode(&query.url)
        .map_err(|_e| AppError::InvalidImageUrl)?;
    let url_str = String::from_utf8(url_bytes).map_err(|_e| AppError::InvalidImageUrl)?;

    // Decode referrer if present
    let referrer = query.r.as_deref().map(decode_referrer).transpose()?;

    // Verify signature (with or without referrer)
    if !verify_proxy_signature(
        &url_str,
        &query.s,
        referrer.as_deref(),
        &state.config.secret,
    ) {
        return Err(AppError::InvalidSignature);
    }

    // Parse and validate the URL. The fetcher re-checks every redirect hop and
    // every resolved address as it goes; this refuses the obvious cases before
    // a connection is opened at all.
    let url = Url::parse(&url_str).map_err(|_e| AppError::InvalidImageUrl)?;
    state
        .fetcher
        .validate(&url)
        .map_err(|_e| AppError::InvalidImageUrl)?;

    // A proxied image is immutable for a given URL, and the request signature
    // `s` is a stable per-URL token — so it doubles as the ETag. When the
    // browser revalidates a cached image it sends `If-None-Match`; answer 304
    // immediately and skip the origin round-trip entirely. This mirrors
    // miniflux's media proxy and is what makes a refresh / post-TTL revisit
    // cheap instead of re-downloading every image from origin.
    //
    // It runs *after* the signature check on purpose: ahead of it, anyone could
    // send `If-None-Match: *` with an unsigned URL and get a cacheable 304 whose
    // `ETag` echoed their own input back.
    if let Some(response) = not_modified(&headers, &query.s) {
        return Ok(response);
    }

    // Fetch the image through the shared, connection-pooled client.
    let url_str = url.to_string();
    let user_agent = state.config.user_agent.clone();
    let response = send_with_retry_on_error(&RetryConfig::default(), || {
        let mut req = state
            .fetcher
            .client(false)
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

    if let Some(content_length) = response.content_length()
        && content_length > MAX_IMAGE_SIZE
    {
        return Err(AppError::ImageTooLarge);
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::ImageFetchError(e.to_string()))?;

    if bytes.len() as u64 > MAX_IMAGE_SIZE {
        return Err(AppError::ImageTooLarge);
    }

    // The bytes decide, not the label. Accommodating an origin that mislabels
    // an image is worth doing; relaying something that is not an image at all
    // under this server's name is not. Serving the sniffed type also stops a
    // wrong label from travelling any further.
    let content_type = sniff_image_type(&bytes)
        .ok_or(AppError::UnsupportedImageType)?
        .to_string();

    // Return the image with appropriate headers. The ETag lets the browser
    // revalidate cheaply on its next visit (see the `If-None-Match` 304
    // short-circuit above).
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, format!("\"{}\"", query.s)),
        ],
        bytes,
    )
        .into_response())
}

/// Whether the origin's `Content-Type` is one this proxy will consider.
///
/// `application/octet-stream` used to be accepted outright, for the servers
/// that never set a real type. That made the proxy a relay for arbitrary bytes,
/// passed through under the origin's own label. It is still accepted, but only
/// as "unlabelled": [`sniff_image_type`] then has to recognise the actual bytes
/// before anything is served.
fn is_valid_image_type(content_type: &str) -> bool {
    let ct = content_type.to_lowercase();
    ct.starts_with("image/") || ct == "application/octet-stream"
}

/// The image type the first bytes of `body` actually are, or `None` for
/// anything not recognised as an image.
///
/// Signatures rather than a crate: the list is short, it does not change, and
/// this is a security check — a dependency here would be one more thing to
/// trust for very little.
///
/// SVG is deliberately included. It is scriptable, unlike every other entry
/// here, but feeds embed SVG constantly (every shields.io badge in a release
/// feed), and the two things that would make a proxied SVG dangerous are
/// already shut: `X-Content-Type-Options: nosniff` and a CSP with no
/// `unsafe-inline` apply to this response like any other, so script inside one
/// does not run even when the URL is opened directly.
fn sniff_image_type(body: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"BM", "image/bmp"),
        (b"\x00\x00\x01\x00", "image/x-icon"),
        (b"II*\x00", "image/tiff"),
        (b"MM\x00*", "image/tiff"),
    ];

    for (magic, mime) in SIGNATURES {
        if body.starts_with(magic) {
            return Some(mime);
        }
    }

    // RIFF containers carry the format in bytes 8..12.
    if body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WEBP".as_slice()) {
        return Some("image/webp");
    }

    // ISO-BMFF: AVIF and HEIC declare their brand in the `ftyp` box.
    if body.get(4..8) == Some(b"ftyp".as_slice()) {
        return match body.get(8..12) {
            Some(b"avif" | b"avis") => Some("image/avif"),
            Some(b"heic" | b"heix" | b"mif1") => Some("image/heic"),
            _ => None,
        };
    }

    is_svg(body).then_some("image/svg+xml")
}

/// SVG has no magic number — it is XML — so this looks for the root element
/// within the leading bytes, allowing for an XML declaration, a doctype or a
/// comment ahead of it.
fn is_svg(body: &[u8]) -> bool {
    const SNIFF_LIMIT: usize = 1024;

    let head = &body[..body.len().min(SNIFF_LIMIT)];
    // Not UTF-8 within the window: whatever it is, it is not an SVG document
    // this server should re-serve. `from_utf8` on a truncated multi-byte
    // character would also fail, which is fine — an SVG root element is ASCII
    // and lands well inside the window.
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };

    let text = text.trim_start().to_ascii_lowercase();
    text.starts_with("<svg")
        || ((text.starts_with("<?xml")
            || text.starts_with("<!doctype")
            || text.starts_with("<!--"))
            && text.contains("<svg"))
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

    #[test]
    fn sniffs_the_formats_a_feed_actually_carries() {
        for (bytes, expected) in [
            (b"\x89PNG\r\n\x1a\n\x00\x00".as_slice(), "image/png"),
            (b"\xff\xd8\xff\xe0JFIF".as_slice(), "image/jpeg"),
            (b"GIF89a....".as_slice(), "image/gif"),
            (b"RIFF\x00\x00\x00\x00WEBPVP8 ".as_slice(), "image/webp"),
            (b"\x00\x00\x00\x20ftypavif\x00".as_slice(), "image/avif"),
            (b"BM\x00\x00".as_slice(), "image/bmp"),
        ] {
            assert_eq!(sniff_image_type(bytes), Some(expected));
        }
    }

    /// Kept on purpose: a release feed is full of shields.io badges, and the
    /// scriptable part of SVG is already shut off by `nosniff` and the CSP.
    #[test]
    fn svg_is_still_proxied() {
        assert_eq!(
            sniff_image_type(br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#),
            Some("image/svg+xml")
        );
        assert_eq!(
            sniff_image_type(b"<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 1 1\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(
            sniff_image_type(b"  \n\t<SVG width=\"10\"></SVG>"),
            Some("image/svg+xml")
        );
    }

    /// The hole this closes: `application/octet-stream` used to be waved
    /// through, so whatever the origin sent was relayed under this server's
    /// name.
    #[test]
    fn refuses_bytes_that_are_not_an_image() {
        for bytes in [
            b"<!DOCTYPE html><html><body>hi</body></html>".as_slice(),
            b"alert('hi')".as_slice(),
            b"{\"secret\":\"value\"}".as_slice(),
            b"%PDF-1.7".as_slice(),
            b"".as_slice(),
            // Close to a signature but not one: an `ftyp` box of a video brand.
            b"\x00\x00\x00\x20ftypmp42".as_slice(),
            // An HTML document that merely mentions svg somewhere.
            b"<html><p>about &lt;svg&gt; files</p></html>".as_slice(),
        ] {
            assert_eq!(
                sniff_image_type(bytes),
                None,
                "must not be served as an image: {:?}",
                String::from_utf8_lossy(&bytes[..bytes.len().min(24)])
            );
        }
    }

    #[test]
    fn sniffing_does_not_panic_on_short_or_binary_bodies() {
        for bytes in [
            b"".as_slice(),
            b"\x89".as_slice(),
            b"RIFF".as_slice(),
            b"\xff\xfe\xfd".as_slice(),
        ] {
            let _ = sniff_image_type(bytes);
        }
    }
}
