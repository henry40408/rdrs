use std::fmt;
use std::net::IpAddr;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
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

/// The hosts a deployment has deliberately opted back in to, on top of the
/// blanket block in [`validate_url`].
///
/// A blanket block is wrong for a self-hosted reader: subscribing to a feed on
/// the same LAN (a Gitea instance, a NAS, another reader) is ordinary use, and
/// every suite here points the fetchers at a loopback mock server. So the deny
/// stays the default and a deployment names its exceptions, rather than the
/// guard being skippable wholesale.
///
/// An entry is a hostname (`nas.local`), an IP (`127.0.0.1`) or a CIDR block
/// (`192.168.0.0/16`). A hostname entry only lifts the name-based rules: this
/// layer does not resolve DNS, so a public name pointing at a private address
/// is not caught here either way.
#[derive(Debug, Clone, Default)]
pub struct FetchPolicy {
    nets: Vec<IpNet>,
    hosts: Vec<String>,
}

impl FetchPolicy {
    /// Parse a comma-separated allow list. Empty or whitespace-only yields a
    /// policy that permits nothing beyond [`validate_url`].
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut nets = Vec::new();
        let mut hosts = Vec::new();

        for part in raw.split(',') {
            let s = part.trim();
            if s.is_empty() {
                continue;
            }
            if let Ok(net) = s.parse::<IpNet>() {
                nets.push(net);
            } else if let Ok(ip) = s.parse::<IpAddr>() {
                nets.push(host_net(ip));
            } else if s.contains('/') || s.contains(char::is_whitespace) {
                return Err(format!(
                    "invalid host, IP or CIDR in RDRS_FETCH_ALLOW_PRIVATE_HOSTS: '{s}'"
                ));
            } else {
                hosts.push(s.to_lowercase());
            }
        }

        Ok(Self { nets, hosts })
    }

    /// [`validate_url`] with this policy's exceptions applied.
    ///
    /// A non-http(s) scheme and a missing host stay fatal: those are not
    /// "points somewhere private", they are "not a fetchable URL at all", and
    /// no allow-list entry should turn `file:///etc/passwd` into a feed.
    pub fn validate(&self, url: &Url) -> Result<(), UrlValidationError> {
        match validate_url(url) {
            Ok(()) => Ok(()),
            Err(UrlValidationError::InvalidScheme) => Err(UrlValidationError::InvalidScheme),
            Err(UrlValidationError::NoHost) => Err(UrlValidationError::NoHost),
            Err(blocked) => {
                if url.host_str().is_some_and(|host| self.allows(host)) {
                    Ok(())
                } else {
                    Err(blocked)
                }
            }
        }
    }

    /// Whether this host was named in the allow list, either as a hostname or
    /// as an address inside one of its networks.
    ///
    /// A hostname match is deliberately all-or-nothing: naming `nas.local` says
    /// "this host is mine", so `services::fetch` accepts whatever it resolves
    /// to rather than re-judging the address.
    pub(crate) fn allows(&self, host: &str) -> bool {
        let bare = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);

        if let Ok(ip) = bare.parse::<IpAddr>() {
            return self.allows_ip(&ip);
        }

        self.hosts.iter().any(|allowed| allowed == host)
    }

    /// Whether an address falls inside one of the allow list's networks. Used
    /// on addresses a hostname *resolved* to, where there is no name left to
    /// match — see `services::fetch`.
    pub(crate) fn allows_ip(&self, ip: &IpAddr) -> bool {
        self.nets.iter().any(|net| net.contains(ip))
    }
}

fn host_net(ip: IpAddr) -> IpNet {
    match ip {
        IpAddr::V4(v4) => IpNet::V4(Ipv4Net::new(v4, 32).expect("host prefix is valid")),
        IpAddr::V6(v6) => IpNet::V6(Ipv6Net::new(v6, 128).expect("host prefix is valid")),
    }
}

/// Whether an address is one the server must never be talked into reaching.
///
/// "Private" here means *not reachable from the public internet*, which is
/// wider than RFC 1918: a reader that refuses `192.168.0.1` but follows
/// `100.100.100.100` into a Tailscale network has not stopped anything. The
/// std predicates that would say this in one call (`is_global`, `is_shared`)
/// are still unstable, so the ranges are listed here.
///
/// Made `pub(crate)` for the fetch guard, which applies the same rule to the
/// addresses a *hostname* resolves to — see `services::fetch`.
pub(crate) fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
                || ipv4.is_unspecified()
                // 100.64.0.0/10, carrier-grade NAT — and what Tailscale hands out
                || (ipv4.octets()[0] == 100 && (64..128).contains(&ipv4.octets()[1]))
                // 198.18.0.0/15, benchmarking
                || (ipv4.octets()[0] == 198 && (18..20).contains(&ipv4.octets()[1]))
                // 240.0.0.0/4 reserved, and 224.0.0.0/4 multicast: neither is a
                // host that can serve a feed, and both are reachable on a LAN
                || ipv4.octets()[0] >= 224
        }
        IpAddr::V6(ipv6) => {
            // An IPv4-mapped address is the same host by another spelling:
            // `::ffff:127.0.0.1` must not walk past a check that only looked at
            // the v6 rules.
            if let Some(mapped) = ipv6.to_ipv4_mapped() {
                return is_private_ip(&IpAddr::V4(mapped));
            }

            ipv6.is_loopback()
                || ipv6.is_unspecified()
                // fc00::/7 unique-local, the v6 answer to RFC 1918
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80
                // 2001:db8::/32 documentation
                || (ipv6.segments()[0] == 0x2001 && ipv6.segments()[1] == 0x0db8)
                // ff00::/8 multicast
                || ipv6.is_multicast()
        }
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
        let ip: IpAddr = "2606:4700::1111".parse().unwrap();
        assert!(!is_private_ip(&ip));
    }

    /// Ranges that are just as unreachable from the internet as RFC 1918, and
    /// just as reachable from the server: refusing `192.168.0.1` while
    /// following `100.100.100.100` into a Tailscale network stops nothing.
    #[test]
    fn blocks_the_ranges_that_are_private_without_being_rfc_1918() {
        for addr in [
            "100.64.0.1",         // CGNAT / Tailscale
            "100.127.255.254",    // CGNAT, top of range
            "198.18.0.1",         // benchmarking
            "240.0.0.1",          // reserved
            "224.0.0.1",          // multicast
            "fd00::1",            // IPv6 unique-local
            "fe80::1",            // IPv6 link-local
            "2001:db8::1",        // IPv6 documentation
            "ff02::1",            // IPv6 multicast
            "::ffff:127.0.0.1",   // IPv4-mapped loopback
            "::ffff:192.168.1.1", // IPv4-mapped RFC 1918
        ] {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(is_private_ip(&ip), "{addr} must be treated as private");
        }
    }

    #[test]
    fn leaves_neighbouring_public_ranges_alone() {
        // Each is adjacent to a blocked range and must stay reachable:
        // 100.63/100.128 flank CGNAT, 198.17/198.20 flank benchmarking,
        // 172.67 looks like RFC 1918 but sits above 172.31, and
        // `::ffff:1.1.1.1` is a mapped *public* address.
        for addr in [
            "100.63.255.255",
            "100.128.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "172.67.219.169",
            "::ffff:1.1.1.1",
        ] {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(!is_private_ip(&ip), "{addr} must stay reachable");
        }
    }

    #[test]
    fn empty_policy_allows_nothing_extra() {
        let policy = FetchPolicy::default();
        assert!(
            policy
                .validate(&Url::parse("http://127.0.0.1:8080/rss.xml").unwrap())
                .is_err()
        );
        assert!(
            policy
                .validate(&Url::parse("https://example.com/rss.xml").unwrap())
                .is_ok()
        );
    }

    #[test]
    fn policy_allows_listed_ip() {
        let policy = FetchPolicy::parse("127.0.0.1").unwrap();
        assert!(
            policy
                .validate(&Url::parse("http://127.0.0.1:8080/rss.xml").unwrap())
                .is_ok()
        );
        // A neighbouring loopback address is not the one that was listed.
        assert!(
            policy
                .validate(&Url::parse("http://127.0.0.2/rss.xml").unwrap())
                .is_err()
        );
    }

    #[test]
    fn policy_allows_listed_cidr() {
        let policy = FetchPolicy::parse("192.168.0.0/16, 10.0.0.0/8").unwrap();
        assert!(
            policy
                .validate(&Url::parse("http://192.168.1.5/feed").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .validate(&Url::parse("http://10.1.2.3/feed").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .validate(&Url::parse("http://172.16.0.1/feed").unwrap())
                .is_err()
        );
    }

    #[test]
    fn policy_allows_listed_hostname() {
        let policy = FetchPolicy::parse("nas.local,localhost").unwrap();
        assert!(
            policy
                .validate(&Url::parse("http://nas.local/feed.xml").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .validate(&Url::parse("http://localhost:3000/feed.xml").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .validate(&Url::parse("http://other.local/feed.xml").unwrap())
                .is_err()
        );
    }

    #[test]
    fn policy_allows_listed_ipv6() {
        let policy = FetchPolicy::parse("::1").unwrap();
        assert!(
            policy
                .validate(&Url::parse("http://[::1]:8080/feed").unwrap())
                .is_ok()
        );
    }

    #[test]
    fn policy_never_lifts_scheme_or_host_rules() {
        // Both entries are pointless for these two URLs on purpose: an allow
        // list names hosts that may be *reached*, it never widens what counts
        // as a fetchable URL.
        let policy = FetchPolicy::parse("127.0.0.1,localhost").unwrap();
        assert!(matches!(
            policy.validate(&Url::parse("file:///etc/passwd").unwrap()),
            Err(UrlValidationError::InvalidScheme)
        ));
        assert!(matches!(
            policy.validate(&Url::parse("ftp://localhost/x").unwrap()),
            Err(UrlValidationError::InvalidScheme)
        ));
    }

    #[test]
    fn policy_parse_rejects_malformed_entries() {
        assert!(FetchPolicy::parse("192.168.0.0/99").is_err());
        assert!(FetchPolicy::parse("not a host").is_err());
    }

    #[test]
    fn policy_parse_ignores_blank_entries() {
        let policy = FetchPolicy::parse("  ,, 127.0.0.1 ,").unwrap();
        assert!(
            policy
                .validate(&Url::parse("http://127.0.0.1/feed").unwrap())
                .is_ok()
        );
    }
}
