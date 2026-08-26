use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::secret::{DOMAIN_IMAGE, tag};

/// Bytes of the derived tag kept in a proxy-URL signature. 8 bytes (64 bits)
/// keeps the URL short; forging one is still a 2^64 search against a keyed MAC
/// whose root key never appears in a URL.
const SIG_BYTES: usize = 8;

/// Signs a URL under the image domain and returns a truncated,
/// base64-encoded signature.
///
/// The signature derives from the shared root key through
/// [`DOMAIN_IMAGE`](crate::secret::DOMAIN_IMAGE), so it can never coincide with
/// a session-cookie or CSRF tag built from the same key.
pub fn sign_url(url: &str, secret: &[u8]) -> String {
    let t = tag(secret, DOMAIN_IMAGE, &[url.as_bytes()]);
    URL_SAFE_NO_PAD.encode(&t[..SIG_BYTES])
}

pub fn verify_signature(url: &str, signature: &str, secret: &[u8]) -> bool {
    let expected = sign_url(url, secret);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Absolute when `base_url` is given, relative otherwise — absolute is what
/// feeds served to external readers need, relative is enough in-page.
pub fn create_proxy_url(original_url: &str, secret: &[u8], base_url: Option<&str>) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(original_url);
    let signature = sign_url(original_url, secret);
    let path = format!("/api/proxy/image?url={encoded}&s={signature}");

    match base_url {
        Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
        None => path,
    }
}

/// Signs `url|referrer` as one message, so a signature minted for one referrer
/// cannot be replayed with another.
pub fn sign_url_with_referrer(url: &str, referrer: &str, secret: &[u8]) -> String {
    let message = format!("{url}|{referrer}");
    let t = tag(secret, DOMAIN_IMAGE, &[message.as_bytes()]);
    URL_SAFE_NO_PAD.encode(&t[..SIG_BYTES])
}

pub fn verify_signature_with_referrer(
    url: &str,
    referrer: &str,
    signature: &str,
    secret: &[u8],
) -> bool {
    let expected = sign_url_with_referrer(url, referrer, secret);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// As [`create_proxy_url`], but carries the referrer the upstream image server
/// needs to serve hotlink-protected images.
pub fn create_proxy_url_with_referrer(
    original_url: &str,
    referrer: &str,
    secret: &[u8],
    base_url: Option<&str>,
) -> String {
    let encoded_url = URL_SAFE_NO_PAD.encode(original_url);
    let encoded_referrer = URL_SAFE_NO_PAD.encode(referrer);
    let signature = sign_url_with_referrer(original_url, referrer, secret);
    let path = format!("/api/proxy/image?url={encoded_url}&s={signature}&r={encoded_referrer}");

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

        let encoded_referrer = URL_SAFE_NO_PAD.encode(referrer);
        assert!(proxy_url.contains(&format!("&r={encoded_referrer}")));

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
