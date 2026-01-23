use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
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
pub fn create_proxy_url(original_url: &str, secret: &[u8]) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(original_url);
    let signature = sign_url(original_url, secret);
    format!("/api/proxy/image?url={}&s={}", encoded, signature)
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

        let proxy_url = create_proxy_url(url, secret);

        assert!(proxy_url.starts_with("/api/proxy/image?url="));
        assert!(proxy_url.contains("&s="));

        // Verify the signature part
        let parts: Vec<&str> = proxy_url.split("&s=").collect();
        assert_eq!(parts.len(), 2);
        let signature = parts[1];
        assert!(verify_signature(url, signature, secret));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"helloworld"));
    }
}
