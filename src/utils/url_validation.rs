use std::fmt;
use std::net::IpAddr;

use url::Url;

/// Error type for URL validation failures.
#[derive(Debug)]
pub enum UrlValidationError {
    /// URL scheme is not http/https.
    InvalidScheme,
    /// URL has no host.
    NoHost,
    /// URL points to a blocked hostname (localhost, loopback, etc.).
    BlockedHost,
    /// URL points to a private/reserved IP address.
    PrivateIp,
}

impl fmt::Display for UrlValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlValidationError::InvalidScheme => write!(f, "URL scheme must be http or https"),
            UrlValidationError::NoHost => write!(f, "URL has no host"),
            UrlValidationError::BlockedHost => write!(f, "URL host is blocked"),
            UrlValidationError::PrivateIp => write!(f, "URL points to a private IP"),
        }
    }
}

impl std::error::Error for UrlValidationError {}

/// The shared SSRF guard, in front of both the readability fetcher and the
/// image proxy: http(s) only, and never a host that resolves inward —
/// localhost, loopback, `.local`/`.internal`, or any private or reserved range.
/// Both callers take a URL the *user* supplied, so anything reachable only from
/// the server is out of bounds.
pub fn validate_url(url: &Url) -> Result<(), UrlValidationError> {
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(UrlValidationError::InvalidScheme),
    }

    // Get the host
    let host = url.host_str().ok_or(UrlValidationError::NoHost)?;

    // Block localhost and loopback addresses
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return Err(UrlValidationError::BlockedHost);
    }

    // Block .local and .internal domains
    #[allow(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "these are DNS hostname suffixes, not file extensions; `host` is already lowercased by the url crate"
    )]
    if host.ends_with(".local") || host.ends_with(".internal") {
        return Err(UrlValidationError::BlockedHost);
    }

    // Try to parse as IP address and check for private ranges
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_private_ip(&ip)
    {
        return Err(UrlValidationError::PrivateIp);
    }

    // Also check if it's an IPv6 address in brackets
    if host.starts_with('[')
        && host.ends_with(']')
        && let Ok(ip) = host[1..host.len() - 1].parse::<IpAddr>()
        && is_private_ip(&ip)
    {
        return Err(UrlValidationError::PrivateIp);
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
        let url = Url::parse("http://192.168.1.1/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_local_domain() {
        let url = Url::parse("http://myhost.local/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_internal_domain() {
        let url = Url::parse("http://server.internal/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_ftp_scheme() {
        let url = Url::parse("ftp://example.com/file").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_file_scheme() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_ipv6_loopback() {
        let url = Url::parse("http://[::1]/article").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_link_local() {
        let url = Url::parse("http://169.254.1.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_broadcast() {
        let url = Url::parse("http://255.255.255.255/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_documentation_ip() {
        // 192.0.2.0/24 is a documentation range
        let url = Url::parse("http://192.0.2.1/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_unspecified_ipv4() {
        let url = Url::parse("http://0.0.0.0/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_unspecified_ipv6() {
        let url = Url::parse("http://[::]/image.jpg").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn test_validate_url_http_valid() {
        let url = Url::parse("http://example.com/article").unwrap();
        assert!(validate_url(&url).is_ok());
    }

    #[test]
    fn test_error_display() {
        assert_eq!(
            format!("{}", UrlValidationError::InvalidScheme),
            "URL scheme must be http or https"
        );
        assert_eq!(format!("{}", UrlValidationError::NoHost), "URL has no host");
        assert_eq!(
            format!("{}", UrlValidationError::BlockedHost),
            "URL host is blocked"
        );
        assert_eq!(
            format!("{}", UrlValidationError::PrivateIp),
            "URL points to a private IP"
        );
    }

    #[test]
    fn test_error_is_error_trait() {
        let err = UrlValidationError::InvalidScheme;
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn test_is_private_ip_ipv6_unspecified() {
        let ip: IpAddr = "::".parse().unwrap();
        assert!(is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_ip_ipv6_loopback() {
        let ip: IpAddr = "::1".parse().unwrap();
        assert!(is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_ip_ipv6_public() {
        let ip: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(!is_private_ip(&ip));
    }
}
