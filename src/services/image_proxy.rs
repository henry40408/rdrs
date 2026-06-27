use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Signs a URL using HMAC-SHA256 and returns a truncated base64-encoded signature.
/// The signature is truncated to 8 bytes (64 bits) for URL brevity.
pub fn sign_url(url: &str, secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(url.as_bytes());
    let result = mac.finalize().into_bytes();
    // Truncate to 8 bytes and base64 encode
    URL_SAFE_NO_PAD.encode(&result[..8])
}

/// Verifies a signature for a given URL.
pub fn verify_signature(url: &str, signature: &str, secret: &[u8]) -> bool {
    let expected = sign_url(url, secret);
    // Use constant-time comparison to prevent timing attacks
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Creates a proxy URL with signature for an image URL.
/// If `base_url` is provided, returns an absolute URL; otherwise returns a relative path.
pub fn create_proxy_url(original_url: &str, secret: &[u8], base_url: Option<&str>) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(original_url);
    let signature = sign_url(original_url, secret);
    let path = format!("/api/proxy/image?url={}&s={}", encoded, signature);

    match base_url {
        Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
        None => path,
    }
}

/// Signs a URL combined with a referrer using HMAC-SHA256.
/// The message is `url|referrer` to bind both values together.
pub fn sign_url_with_referrer(url: &str, referrer: &str, secret: &[u8]) -> String {
    let message = format!("{}|{}", url, referrer);
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    let result = mac.finalize().into_bytes();
    URL_SAFE_NO_PAD.encode(&result[..8])
}

/// Verifies a signature for a given URL and referrer pair.
pub fn verify_signature_with_referrer(
    url: &str,
    referrer: &str,
    signature: &str,
    secret: &[u8],
) -> bool {
    let expected = sign_url_with_referrer(url, referrer, secret);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Creates a proxy URL with signature for an image URL, including a referrer parameter.
/// If `base_url` is provided, returns an absolute URL; otherwise returns a relative path.
pub fn create_proxy_url_with_referrer(
    original_url: &str,
    referrer: &str,
    secret: &[u8],
    base_url: Option<&str>,
) -> String {
    let encoded_url = URL_SAFE_NO_PAD.encode(original_url);
    let encoded_referrer = URL_SAFE_NO_PAD.encode(referrer);
    let signature = sign_url_with_referrer(original_url, referrer, secret);
    let path = format!(
        "/api/proxy/image?url={}&s={}&r={}",
        encoded_url, signature, encoded_referrer
    );

    match base_url {
        Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
        None => path,
    }
}

/// Constant-time equality comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_url() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";

        let signature = sign_url(url, secret);
        // Signature should be 11 characters (8 bytes base64 encoded without padding)
        assert_eq!(signature.len(), 11);
    }

    #[test]
    fn test_verify_signature_valid() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";

        let signature = sign_url(url, secret);
        assert!(verify_signature(url, &signature, secret));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";

        assert!(!verify_signature(url, "invalid_sig", secret));
    }

    #[test]
    fn test_verify_signature_wrong_url() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let other_url = "https://example.com/other.jpg";

        let signature = sign_url(url, secret);
        assert!(!verify_signature(other_url, &signature, secret));
    }

    #[test]
    fn test_verify_signature_wrong_secret() {
        let secret1 = b"test_secret_key_32_bytes_long!!!";
        let secret2 = b"other_secret_key_32_bytes_long!!";
        let url = "https://example.com/image.jpg";

        let signature = sign_url(url, secret1);
        assert!(!verify_signature(url, &signature, secret2));
    }

    #[test]
    fn test_create_proxy_url() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";

        let proxy_url = create_proxy_url(url, secret, None);

        assert!(proxy_url.starts_with("/api/proxy/image?url="));
        assert!(proxy_url.contains("&s="));

        // Verify the signature part
        let parts: Vec<&str> = proxy_url.split("&s=").collect();
        assert_eq!(parts.len(), 2);
        let signature = parts[1];
        assert!(verify_signature(url, signature, secret));
    }

    #[test]
    fn test_create_proxy_url_with_base() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let base = "https://my-instance.com";

        let proxy_url = create_proxy_url(url, secret, Some(base));
        assert!(proxy_url.starts_with("https://my-instance.com/api/proxy/image?url="));
        assert!(proxy_url.contains("&s="));

        // Verify signature still works
        let parts: Vec<&str> = proxy_url.split("&s=").collect();
        assert_eq!(parts.len(), 2);
        let signature = parts[1];
        assert!(verify_signature(url, signature, secret));
    }

    #[test]
    fn test_create_proxy_url_with_base_trailing_slash() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let base = "https://my-instance.com/"; // trailing slash

        let proxy_url = create_proxy_url(url, secret, Some(base));
        // Should not have double slash
        assert!(!proxy_url.contains("com//api"));
        assert!(proxy_url.starts_with("https://my-instance.com/api/proxy/image?url="));
    }

    #[test]
    fn test_sign_url_with_referrer() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";

        let sig = sign_url_with_referrer(url, referrer, secret);
        assert_eq!(sig.len(), 11);

        // Different referrer should produce different signature
        let sig2 = sign_url_with_referrer(url, "https://other.com", secret);
        assert_ne!(sig, sig2);

        // Should differ from sign_url without referrer
        let sig_no_ref = sign_url(url, secret);
        assert_ne!(sig, sig_no_ref);
    }

    #[test]
    fn test_verify_signature_with_referrer_valid() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";

        let sig = sign_url_with_referrer(url, referrer, secret);
        assert!(verify_signature_with_referrer(url, referrer, &sig, secret));
    }

    #[test]
    fn test_verify_signature_with_referrer_wrong_referrer() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";

        let sig = sign_url_with_referrer(url, referrer, secret);
        assert!(!verify_signature_with_referrer(
            url,
            "https://other.com",
            &sig,
            secret
        ));
    }

    #[test]
    fn test_create_proxy_url_with_referrer() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";

        let proxy_url = create_proxy_url_with_referrer(url, referrer, secret, None);

        assert!(proxy_url.starts_with("/api/proxy/image?url="));
        assert!(proxy_url.contains("&s="));
        assert!(proxy_url.contains("&r="));

        // Verify the encoded referrer
        let encoded_referrer = URL_SAFE_NO_PAD.encode(referrer);
        assert!(proxy_url.contains(&format!("&r={}", encoded_referrer)));

        // Verify the signature
        let parts: Vec<&str> = proxy_url.split('&').collect();
        let sig = parts[1].strip_prefix("s=").unwrap();
        assert!(verify_signature_with_referrer(url, referrer, sig, secret));
    }

    #[test]
    fn test_create_proxy_url_with_referrer_and_base() {
        let secret = b"test_secret_key_32_bytes_long!!!";
        let url = "https://example.com/image.jpg";
        let referrer = "https://example.com";
        let base = "https://rdrs.example.com";

        let proxy_url = create_proxy_url_with_referrer(url, referrer, secret, Some(base));

        assert!(proxy_url.starts_with("https://rdrs.example.com/api/proxy/image?url="));
        assert!(proxy_url.contains("&s="));
        assert!(proxy_url.contains("&r="));

        // Verify signature still works
        let parts: Vec<&str> = proxy_url.split('&').collect();
        let sig = parts[1].strip_prefix("s=").unwrap();
        assert!(verify_signature_with_referrer(url, referrer, sig, secret));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"helloworld"));
    }
}
