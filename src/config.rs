use base64::{Engine, engine::general_purpose::STANDARD};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rand::Rng;
use std::env;
use std::net::{IpAddr, SocketAddr};

use crate::utils::url_validation::FetchPolicy;

/// Default `User-Agent` for outbound HTTP requests.
pub const DEFAULT_USER_AGENT: &str = concat!(
    "RDRS/",
    env!("GIT_VERSION"),
    " (RSS Reader; +https://github.com/henry40408/rdrs)"
);

/// Browser `User-Agent` strings the feed-edit form suggests for servers that reject
/// non-browsers. Excludes [`DEFAULT_USER_AGENT`] (the empty field already means it).
/// Each must be a valid `HeaderValue`: `refresh_feed` silently drops an invalid one.
pub const CUSTOM_USER_AGENT_SUGGESTIONS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub server_bind: SocketAddr,
    pub multi_user_enabled: bool,
    /// Root key for every signature; each use derives its own key via [`crate::secret`].
    pub secret: Vec<u8>,
    /// `RDRS_SECRET` was unset or too short, so the key is random and dies with the process.
    pub secret_generated: bool,
    pub user_agent: String,
    /// Private hosts the SSRF guard lets fetchers reach (`RDRS_FETCH_ALLOW_PRIVATE_HOSTS`;
    /// default empty, since feed URLs are attacker-influenced).
    pub fetch_allow_private: FetchPolicy,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub webauthn_rp_name: String,
    pub public_base_url: Option<String>,
    pub cookie_secure: bool,
    pub auth_proxy_header: String,
    pub trusted_proxy_networks: Vec<IpNet>,
    pub auth_proxy_user_creation: bool,
    pub disable_local_auth: bool,
    pub auth_proxy_groups_header: String,
    pub auth_proxy_admin_group: String,
    pub auth_proxy_logout_url: Option<String>,
    /// Attempts per client IP per window, counted per endpoint class
    /// ([`crate::middleware::rate_limit::Bucket`]). `0` disables the limiter.
    pub login_rate_limit_attempts: u32,
    /// Fixed-window length, in seconds, for [`Config::login_rate_limit_attempts`].
    pub login_rate_limit_window_secs: u64,
    /// Whether to send `Strict-Transport-Security`; see [`parse_hsts`].
    pub hsts: bool,
    /// HSTS `max-age` in seconds (default one year). `0` makes browsers forget a
    /// mis-set declaration; it is not "off".
    pub hsts_max_age: u64,
    /// Whether HSTS includes `; includeSubDomains` (default on).
    pub hsts_include_subdomains: bool,
}

/// Parse comma-separated CIDRs or bare IPs (bare IP → `/32` or `/128`); blanks ignored.
pub fn parse_trusted_networks(raw: &str) -> Result<Vec<IpNet>, String> {
    let mut nets = Vec::new();
    for part in raw.split(',') {
        let s = part.trim();
        if s.is_empty() {
            continue;
        }
        if let Ok(net) = s.parse::<IpNet>() {
            nets.push(net);
        } else if let Ok(ip) = s.parse::<IpAddr>() {
            let net = match ip {
                IpAddr::V4(v4) => IpNet::V4(Ipv4Net::new(v4, 32).expect("host prefix is valid")),
                IpAddr::V6(v6) => IpNet::V6(Ipv6Net::new(v6, 128).expect("host prefix is valid")),
            };
            nets.push(net);
        } else {
            return Err(format!(
                "invalid CIDR or IP in RDRS_TRUSTED_PROXY_NETWORKS: '{s}'"
            ));
        }
    }
    Ok(nets)
}

/// Whether the session cookie gets `Secure`: explicit `RDRS_COOKIE_SECURE`, else
/// derived from an `https://` `RDRS_PUBLIC_BASE_URL` (plain-HTTP dev keeps working).
/// An unrecognized value is a hard error, since reading a typo as "off" would strip
/// `Secure` from an HTTPS deployment.
pub fn parse_cookie_secure(
    raw: Option<&str>,
    public_base_url: Option<&str>,
) -> Result<bool, String> {
    let derived = public_base_url
        .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("https://"));
    parse_bool_derived(raw, "RDRS_COOKIE_SECURE", derived)
}

/// Whether to send HSTS; derived like [`parse_cookie_secure`] from `RDRS_HSTS`.
/// HSTS is sticky and unretractable, so only `true`/`false`/`1`/`0` are accepted.
pub fn parse_hsts(raw: Option<&str>, public_base_url: Option<&str>) -> Result<bool, String> {
    let derived = public_base_url
        .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("https://"));
    parse_bool_derived(raw, "RDRS_HSTS", derived)
}

/// Strict boolean: blank → `derived`, `true`/`1`/`false`/`0` (any case), else an error.
/// Used where the default can be `true`, so the lenient [`flag`] is unsafe.
fn parse_bool_derived(raw: Option<&str>, var_name: &str, derived: bool) -> Result<bool, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => Ok(true),
        Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => Ok(false),
        Some(v) => Err(format!(
            "invalid {var_name} '{v}': expected one of true, false, 1, 0"
        )),
        None => Ok(derived),
    }
}

/// Database engine selected by `database_url`, fixed for the process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

/// `postgres://` / `postgresql://` → `Postgres`; anything else → `SQLite`.
pub fn classify_backend(database_url: &str) -> Backend {
    let lower = database_url.trim_start().to_ascii_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        Backend::Postgres
    } else {
        Backend::Sqlite
    }
}

/// Replace the password in a `database_url` with `***` for display; the rest stays legible.
pub fn redact_database_url(database_url: &str) -> String {
    let Some((scheme, rest)) = database_url.split_once("://") else {
        return database_url.to_string();
    };
    // Split at the *last* '@' so an unencoded '@' in the password is not leaked.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return database_url.to_string();
    };
    let user = userinfo.split_once(':').map_or(userinfo, |(u, _)| u);
    if userinfo.contains(':') {
        format!("{scheme}://{user}:***@{host}{tail}")
    } else {
        database_url.to_string()
    }
}

/// Parse `RDRS_SERVER_BIND` (`host:port`); default `127.0.0.1:8080` so nothing is
/// exposed without opting in.
pub fn parse_server_bind(raw: Option<&str>) -> Result<SocketAddr, String> {
    match raw {
        Some(v) if !v.is_empty() => v
            .parse::<SocketAddr>()
            .map_err(|e| format!("invalid RDRS_SERVER_BIND '{v}': {e}")),
        _ => Ok(SocketAddr::from(([127, 0, 0, 1], 8080))),
    }
}

/// Parse `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` (default
/// [`crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS`]); a typo is a hard error.
fn parse_login_rate_limit_attempts(raw: Option<&str>) -> Result<u32, String> {
    match raw {
        Some(v) => v
            .parse::<u32>()
            .map_err(|e| format!("invalid RDRS_LOGIN_RATE_LIMIT_ATTEMPTS '{v}': {e}")),
        None => Ok(crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS),
    }
}

/// Parse `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS`; `0` is rejected because it would
/// silently disable throttling.
fn parse_login_rate_limit_window_secs(raw: Option<&str>) -> Result<u64, String> {
    match raw {
        Some(v) => {
            let secs = v
                .parse::<u64>()
                .map_err(|e| format!("invalid RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS '{v}': {e}"))?;
            if secs == 0 {
                return Err(
                    "invalid RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS '0': the window must be at \
                     least 1 second; a zero-length window elapses instantly and silently \
                     disables throttling. Use RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=0 to disable the \
                     limiter deliberately."
                        .to_string(),
                );
            }
            Ok(secs)
        }
        None => Ok(crate::middleware::rate_limit::LOGIN_WINDOW_SECS),
    }
}

/// Resolve `RDRS_SECRET` (base64 or raw, at least [`crate::secret::MIN_SECRET_LEN`]
/// bytes) into `(key, generated)`; a shorter value is replaced by a random key.
/// Not trimmed via [`nonblank`]: that would silently rotate a whitespace-bearing key.
fn load_secret(raw: Option<String>) -> (Vec<u8>, bool) {
    use crate::secret::MIN_SECRET_LEN;
    if let Some(secret_str) = raw {
        if let Ok(decoded) = STANDARD.decode(&secret_str)
            && decoded.len() >= MIN_SECRET_LEN
        {
            return (decoded, false);
        }
        if secret_str.len() >= MIN_SECRET_LEN {
            return (secret_str.into_bytes(), false);
        }
    }

    let mut secret = vec![0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    (secret, true)
}

/// Read `key` trimmed, treating empty as unset (so `FOO=` means "not configured").
fn nonblank(get: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Lenient boolean: `true` (any case) or `1` is on, anything else off. Only for
/// default-off settings.
fn flag(get: &impl Fn(&str) -> Option<String>, key: &str) -> bool {
    nonblank(get, key).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// Variables renamed by the `RDRS_` prefix migration, old → new. `DATABASE_URL`
/// is deliberately absent: it is a cross-tool convention.
pub const RENAMED_VARS: &[(&str, &str)] = &[
    ("SERVER_BIND", "RDRS_SERVER_BIND"),
    // `SIGNUP_ENABLED` lives in `RETIRED_VARS` instead.
    ("MULTI_USER_ENABLED", "RDRS_MULTI_USER_ENABLED"),
    ("IMAGE_PROXY_SECRET", "RDRS_SECRET"),
    ("USER_AGENT", "RDRS_USER_AGENT"),
    ("WEBAUTHN_RP_ID", "RDRS_WEBAUTHN_RP_ID"),
    ("WEBAUTHN_RP_ORIGIN", "RDRS_WEBAUTHN_RP_ORIGIN"),
    ("WEBAUTHN_RP_NAME", "RDRS_WEBAUTHN_RP_NAME"),
    ("PUBLIC_BASE_URL", "RDRS_PUBLIC_BASE_URL"),
    ("COOKIE_SECURE", "RDRS_COOKIE_SECURE"),
    ("AUTH_PROXY_HEADER", "RDRS_AUTH_PROXY_HEADER"),
    ("TRUSTED_PROXY_NETWORKS", "RDRS_TRUSTED_PROXY_NETWORKS"),
    ("AUTH_PROXY_USER_CREATION", "RDRS_AUTH_PROXY_USER_CREATION"),
    ("DISABLE_LOCAL_AUTH", "RDRS_DISABLE_LOCAL_AUTH"),
    ("AUTH_PROXY_GROUPS_HEADER", "RDRS_AUTH_PROXY_GROUPS_HEADER"),
    ("AUTH_PROXY_ADMIN_GROUP", "RDRS_AUTH_PROXY_ADMIN_GROUP"),
    ("AUTH_PROXY_LOGOUT_URL", "RDRS_AUTH_PROXY_LOGOUT_URL"),
    ("KAGI_API_BASE", "RDRS_KAGI_API_BASE"),
    // Read by clap in `main`, but listed so the check still catches it.
    ("LOG_FORMAT", "RDRS_LOG_FORMAT"),
];

/// Variables whose feature is gone, with what replaced them. Refused at startup so
/// an operator is not misled into thinking e.g. public signup still exists.
pub const RETIRED_VARS: &[(&str, &str)] = &[
    ("RDRS_SIGNUP_ENABLED", SIGNUP_RETIRED),
    ("SIGNUP_ENABLED", SIGNUP_RETIRED),
];

/// Why both spellings of the signup flag no longer configure anything.
const SIGNUP_RETIRED: &str = "self-service registration was removed; an admin now creates accounts from \
     /admin and hands out a one-time link. The first account is still created \
     at /setup on a fresh install";

/// Refuse to start when a [`RETIRED_VARS`] entry still carries a value.
pub fn reject_retired_vars(get: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let stale: Vec<String> = RETIRED_VARS
        .iter()
        .filter(|(name, _)| nonblank(get, name).is_some())
        .map(|(name, why)| format!("  {name}: {why}"))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    Err(format!(
        "these environment variables no longer configure anything. Remove them and \
         restart:\n{}",
        stale.join("\n")
    ))
}

/// Refuse to start when a pre-prefix name still has a (non-blank) value; ignoring it
/// would boot a working server on defaults against an empty database.
pub fn reject_legacy_vars(get: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let stale: Vec<String> = RENAMED_VARS
        .iter()
        .filter(|(old, _)| nonblank(get, old).is_some())
        .map(|(old, new)| format!("  {old} -> {new}"))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    Err(format!(
        "these environment variables were renamed and are no longer read. Rename them \
         and restart:\n{}\nrdrs refuses to start rather than silently fall back to its \
         defaults, which would come up against an empty database.",
        stale.join("\n")
    ))
}

impl Config {
    /// Build the config from the process environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_map(|key| env::var(key).ok())
    }

    /// Key for encrypting service tokens at rest, or `None` when the secret is
    /// generated (encrypting with it would lose the tokens on restart).
    pub fn service_token_key(&self) -> Option<&[u8]> {
        (!self.secret_generated).then_some(self.secret.as_slice())
    }

    /// Build the config from a key→value lookup, so tests need not mutate the environment.
    pub fn from_map(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        reject_legacy_vars(&get)?;
        reject_retired_vars(&get)?;

        let (secret, secret_generated) = load_secret(get("RDRS_SECRET"));
        let server_bind = parse_server_bind(nonblank(&get, "RDRS_SERVER_BIND").as_deref())?;

        let trusted_proxy_networks = parse_trusted_networks(
            &nonblank(&get, "RDRS_TRUSTED_PROXY_NETWORKS").unwrap_or_default(),
        )?;

        let fetch_allow_private = FetchPolicy::parse(
            &nonblank(&get, "RDRS_FETCH_ALLOW_PRIVATE_HOSTS").unwrap_or_default(),
        )?;

        let public_base_url = nonblank(&get, "RDRS_PUBLIC_BASE_URL");
        // Raw, not `nonblank`: the parser must tell "unset" from "unrecognized".
        let cookie_secure = parse_cookie_secure(
            get("RDRS_COOKIE_SECURE").as_deref(),
            public_base_url.as_deref(),
        )?;

        let login_rate_limit_attempts = parse_login_rate_limit_attempts(
            nonblank(&get, "RDRS_LOGIN_RATE_LIMIT_ATTEMPTS").as_deref(),
        )?;
        let login_rate_limit_window_secs = parse_login_rate_limit_window_secs(
            nonblank(&get, "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS").as_deref(),
        )?;

        // Raw, for the same reason as `cookie_secure`.
        let hsts = parse_hsts(get("RDRS_HSTS").as_deref(), public_base_url.as_deref())?;
        let hsts_max_age = nonblank(&get, "RDRS_HSTS_MAX_AGE")
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|e| format!("invalid RDRS_HSTS_MAX_AGE '{v}': {e}"))
            })
            .transpose()?
            .unwrap_or(31_536_000);
        let hsts_include_subdomains = parse_bool_derived(
            get("RDRS_HSTS_INCLUDE_SUBDOMAINS").as_deref(),
            "RDRS_HSTS_INCLUDE_SUBDOMAINS",
            true,
        )?;

        Ok(Self {
            // Not prefixed — see `RENAMED_VARS`.
            database_url: nonblank(&get, "DATABASE_URL")
                .unwrap_or_else(|| "rdrs.sqlite3".to_string()),
            server_bind,
            multi_user_enabled: flag(&get, "RDRS_MULTI_USER_ENABLED"),
            secret,
            secret_generated,
            user_agent: nonblank(&get, "RDRS_USER_AGENT")
                .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
            fetch_allow_private,
            webauthn_rp_id: nonblank(&get, "RDRS_WEBAUTHN_RP_ID")
                .unwrap_or_else(|| "localhost".to_string()),
            webauthn_rp_origin: nonblank(&get, "RDRS_WEBAUTHN_RP_ORIGIN")
                .unwrap_or_else(|| format!("http://localhost:{}", server_bind.port())),
            webauthn_rp_name: nonblank(&get, "RDRS_WEBAUTHN_RP_NAME")
                .unwrap_or_else(|| "rdrs".to_string()),
            public_base_url,
            cookie_secure,
            auth_proxy_header: nonblank(&get, "RDRS_AUTH_PROXY_HEADER").unwrap_or_default(),
            trusted_proxy_networks,
            auth_proxy_user_creation: flag(&get, "RDRS_AUTH_PROXY_USER_CREATION"),
            disable_local_auth: flag(&get, "RDRS_DISABLE_LOCAL_AUTH"),
            auth_proxy_groups_header: nonblank(&get, "RDRS_AUTH_PROXY_GROUPS_HEADER")
                .unwrap_or_default(),
            auth_proxy_admin_group: nonblank(&get, "RDRS_AUTH_PROXY_ADMIN_GROUP")
                .unwrap_or_default(),
            auth_proxy_logout_url: nonblank(&get, "RDRS_AUTH_PROXY_LOGOUT_URL"),
            login_rate_limit_attempts,
            login_rate_limit_window_secs,
            hsts,
            hsts_max_age,
            hsts_include_subdomains,
        })
    }

    /// Whether forward-auth (trusted-header) login is enabled.
    pub fn auth_proxy_enabled(&self) -> bool {
        !self.auth_proxy_header.is_empty()
    }

    /// Whether group → role mapping is active (both header and admin group set).
    pub fn group_mapping_enabled(&self) -> bool {
        !self.auth_proxy_groups_header.is_empty() && !self.auth_proxy_admin_group.is_empty()
    }

    /// Whether `ip` (the TCP peer) falls inside a trusted proxy network.
    pub fn is_trusted_peer(&self, ip: IpAddr) -> bool {
        self.trusted_proxy_networks
            .iter()
            .any(|net| net.contains(&ip))
    }

    /// The originating client IP. Forwarding headers are honoured ONLY from a trusted
    /// peer; `X-Forwarded-For` is read right-to-left, since the left-most is forgeable.
    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &axum::http::HeaderMap) -> IpAddr {
        let Some(peer) = peer else {
            return IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        };
        if !self.is_trusted_peer(peer) {
            return peer;
        }
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            for part in xff.rsplit(',') {
                let Ok(ip) = part.trim().parse::<IpAddr>() else {
                    // A malformed hop breaks the trust chain; stop here.
                    break;
                };
                if !self.is_trusted_peer(ip) {
                    return ip;
                }
            }
        }
        // No untrusted XFF entry: fall back to `X-Real-IP`, then the peer.
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
        peer
    }

    /// The database engine selected by `database_url`.
    pub fn backend(&self) -> Backend {
        classify_backend(&self.database_url)
    }

    /// Validate cross-field invariants at startup. Returns the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.auth_proxy_enabled() && self.trusted_proxy_networks.is_empty() {
            return Err(
                "RDRS_AUTH_PROXY_HEADER is set but RDRS_TRUSTED_PROXY_NETWORKS is empty. \
                 Refusing to trust an identity header without a trusted-source check."
                    .to_string(),
            );
        }
        if self.disable_local_auth && !self.auth_proxy_enabled() {
            return Err(
                "RDRS_DISABLE_LOCAL_AUTH is set but RDRS_AUTH_PROXY_HEADER is not configured. \
                 This would leave no way to log in via the browser."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Whether first-run setup is open: only with zero accounts, which is what makes
    /// an anonymous account-creating endpoint acceptable.
    pub fn can_setup(&self, user_count: i64) -> bool {
        user_count == 0
    }

    /// Whether an admin may create another account (`RDRS_MULTI_USER_ENABLED`).
    pub fn can_create_account(&self, user_count: i64) -> bool {
        user_count == 0 || self.multi_user_enabled
    }

    /// Warn when the `WebAuthn` RP origin is `localhost` or disagrees with
    /// `RDRS_PUBLIC_BASE_URL`, either of which breaks passkeys.
    pub fn webauthn_rp_warning(&self) -> Option<String> {
        if self.webauthn_rp_origin.contains("localhost") {
            return Some(format!(
                "RDRS_WEBAUTHN_RP_ORIGIN is still '{}'. Passkeys will be rejected from any other \
                 origin — set RDRS_WEBAUTHN_RP_ID and RDRS_WEBAUTHN_RP_ORIGIN to your deployment domain.",
                self.webauthn_rp_origin
            ));
        }
        if let Some(base) = &self.public_base_url
            && base.trim_end_matches('/') != self.webauthn_rp_origin.trim_end_matches('/')
        {
            return Some(format!(
                "RDRS_WEBAUTHN_RP_ORIGIN ('{}') does not match RDRS_PUBLIC_BASE_URL ('{}'). Passkeys \
                     may be rejected — align RDRS_WEBAUTHN_RP_ORIGIN with the URL users access.",
                self.webauthn_rp_origin, base
            ));
        }
        None
    }

    /// The HSTS header value, or `None` when disabled. Never contains `preload`:
    /// the preload list is effectively irreversible.
    pub fn hsts_header_value(&self) -> Option<String> {
        if !self.hsts {
            return None;
        }
        let mut value = format!("max-age={}", self.hsts_max_age);
        if self.hsts_include_subdomains {
            value.push_str("; includeSubDomains");
        }
        Some(value)
    }

    /// Warn when rate limiting runs without trusted proxies: behind a proxy every
    /// visitor would share one bucket, letting one abuser lock out everyone.
    pub fn rate_limit_proxy_warning(&self) -> Option<String> {
        if self.login_rate_limit_attempts > 0 && self.trusted_proxy_networks.is_empty() {
            return Some(
                "RDRS_LOGIN_RATE_LIMIT_ATTEMPTS is enabled but RDRS_TRUSTED_PROXY_NETWORKS is \
                 empty. Without a trusted-proxy list rdrs keys the credential rate limiter on \
                 the TCP peer, so behind a reverse proxy every visitor shares one bucket and a \
                 single abuser can lock out all users. Set RDRS_TRUSTED_PROXY_NETWORKS to the \
                 proxy's address(es) so X-Forwarded-For is honoured."
                    .to_string(),
            );
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::rate_limit::{LOGIN_MAX_ATTEMPTS, LOGIN_WINDOW_SECS};

    fn test_config() -> Config {
        Config {
            database_url: "test.db".to_string(),
            server_bind: "127.0.0.1:8080".parse().unwrap(),
            multi_user_enabled: false,
            secret: vec![0u8; 32],
            secret_generated: false,
            user_agent: DEFAULT_USER_AGENT.to_string(),
            fetch_allow_private: FetchPolicy::default(),
            webauthn_rp_id: "localhost".to_string(),
            webauthn_rp_origin: "http://localhost:8080".to_string(),
            webauthn_rp_name: "rdrs".to_string(),
            public_base_url: None,
            cookie_secure: false,
            auth_proxy_header: String::new(),
            trusted_proxy_networks: Vec::new(),
            auth_proxy_user_creation: false,
            disable_local_auth: false,
            auth_proxy_groups_header: String::new(),
            auth_proxy_admin_group: String::new(),
            auth_proxy_logout_url: None,
            login_rate_limit_attempts: LOGIN_MAX_ATTEMPTS,
            login_rate_limit_window_secs: LOGIN_WINDOW_SECS,
            hsts: false,
            hsts_max_age: 31_536_000,
            hsts_include_subdomains: true,
        }
    }

    #[test]
    fn test_parse_server_bind() {
        assert_eq!(
            parse_server_bind(None).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(
            parse_server_bind(Some("")).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(
            parse_server_bind(Some("127.0.0.1:9000")).unwrap(),
            "127.0.0.1:9000".parse().unwrap()
        );
        let err = parse_server_bind(Some("not-an-addr")).unwrap_err();
        assert!(err.contains("invalid RDRS_SERVER_BIND"), "got: {err}");
        assert!(parse_server_bind(Some("127.0.0.1")).is_err());
    }

    /// Build a config from a fixed set of variables; everything else is unset.
    fn from_vars(vars: &[(&str, &str)]) -> Config {
        Config::from_map(|key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        })
        .expect("from_map should succeed")
    }

    #[test]
    fn fetch_allow_private_defaults_to_allowing_nothing() {
        let config = from_vars(&[]);
        let private = url::Url::parse("http://192.168.1.10/feed.xml").unwrap();
        assert!(config.fetch_allow_private.validate(&private).is_err());
    }

    #[test]
    fn fetch_allow_private_opts_named_hosts_back_in() {
        let config = from_vars(&[("RDRS_FETCH_ALLOW_PRIVATE_HOSTS", "192.168.0.0/16,nas.local")]);
        assert!(
            config
                .fetch_allow_private
                .validate(&url::Url::parse("http://192.168.1.10/feed.xml").unwrap())
                .is_ok()
        );
        assert!(
            config
                .fetch_allow_private
                .validate(&url::Url::parse("http://nas.local/feed.xml").unwrap())
                .is_ok()
        );
        assert!(
            config
                .fetch_allow_private
                .validate(&url::Url::parse("http://127.0.0.1/feed.xml").unwrap())
                .is_err()
        );
    }

    #[test]
    fn fetch_allow_private_rejects_a_malformed_entry_at_startup() {
        let err = Config::from_map(|k| {
            (k == "RDRS_FETCH_ALLOW_PRIVATE_HOSTS").then(|| "10.0.0.0/64".into())
        })
        .expect_err("a malformed allow list must fail startup");
        assert!(err.contains("RDRS_FETCH_ALLOW_PRIVATE_HOSTS"), "{err}");
    }

    #[test]
    fn test_legacy_var_names_refuse_to_start() {
        let err = Config::from_map(|k| (k == "SERVER_BIND").then(|| "0.0.0.0:8080".into()))
            .expect_err("a legacy name must fail startup");
        assert!(err.contains("SERVER_BIND -> RDRS_SERVER_BIND"), "{err}");

        // All stale names are reported at once.
        let err = Config::from_map(|k| match k {
            "IMAGE_PROXY_SECRET" => Some("x".repeat(32)),
            "AUTH_PROXY_HEADER" => Some("Remote-User".into()),
            _ => None,
        })
        .expect_err("legacy names must fail startup");
        assert!(err.contains("IMAGE_PROXY_SECRET -> RDRS_SECRET"), "{err}");
        assert!(
            err.contains("AUTH_PROXY_HEADER -> RDRS_AUTH_PROXY_HEADER"),
            "{err}"
        );

        // Blank does not count.
        assert!(Config::from_map(|k| (k == "SERVER_BIND").then(|| "  ".into())).is_ok());

        // DATABASE_URL keeps its bare name.
        let config = from_vars(&[("DATABASE_URL", "postgres://u:p@db/rdrs")]);
        assert_eq!(config.backend(), Backend::Postgres);
    }

    #[test]
    fn test_server_bind_drives_listener_and_rp_origin() {
        let config = from_vars(&[("RDRS_SERVER_BIND", "127.0.0.1:9137")]);
        assert_eq!(config.server_bind, "127.0.0.1:9137".parse().unwrap());
        assert_eq!(config.webauthn_rp_origin, "http://localhost:9137");
    }

    #[test]
    fn test_defaults_with_nothing_configured() {
        let config = from_vars(&[]);
        assert_eq!(config.database_url, "rdrs.sqlite3");
        assert_eq!(
            config.server_bind,
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(config.user_agent, DEFAULT_USER_AGENT);
        assert_eq!(config.webauthn_rp_id, "localhost");
        assert_eq!(config.webauthn_rp_name, "rdrs");
        assert!(!config.multi_user_enabled);
        assert!(!config.cookie_secure);
        assert!(config.public_base_url.is_none());
        assert!(config.auth_proxy_logout_url.is_none());
        assert!(!config.auth_proxy_enabled());
        assert!(config.secret_generated);
    }

    /// `refresh_feed` silently drops an invalid `User-Agent`, so every suggestion must parse.
    #[test]
    fn custom_user_agent_suggestions_are_valid_header_values() {
        assert!(!CUSTOM_USER_AGENT_SUGGESTIONS.is_empty());

        for ua in CUSTOM_USER_AGENT_SUGGESTIONS {
            assert!(
                reqwest::header::HeaderValue::from_str(ua).is_ok(),
                "suggestion {ua:?} cannot be sent as a header value"
            );
        }

        assert!(
            !CUSTOM_USER_AGENT_SUGGESTIONS.contains(&DEFAULT_USER_AGENT),
            "the default belongs to the empty field, not the list"
        );
    }

    #[test]
    fn test_blank_values_count_as_unset() {
        let config = from_vars(&[
            ("DATABASE_URL", "  "),
            ("RDRS_USER_AGENT", ""),
            ("RDRS_PUBLIC_BASE_URL", "   "),
            ("RDRS_AUTH_PROXY_LOGOUT_URL", " "),
            // Blank must leave forward auth off.
            ("RDRS_AUTH_PROXY_HEADER", "  "),
        ]);
        assert_eq!(config.database_url, "rdrs.sqlite3");
        assert_eq!(config.user_agent, DEFAULT_USER_AGENT);
        assert!(config.public_base_url.is_none());
        assert!(config.auth_proxy_logout_url.is_none());
        assert!(!config.auth_proxy_enabled());
    }

    #[test]
    fn test_values_are_trimmed() {
        let config = from_vars(&[
            ("DATABASE_URL", "  postgres://u:p@db/rdrs  "),
            ("RDRS_AUTH_PROXY_HEADER", " Remote-User "),
            (
                "RDRS_AUTH_PROXY_LOGOUT_URL",
                " https://auth.example.com/logout ",
            ),
        ]);
        assert_eq!(config.database_url, "postgres://u:p@db/rdrs");
        assert_eq!(config.backend(), Backend::Postgres);
        assert_eq!(config.auth_proxy_header, "Remote-User");
        assert_eq!(
            config.auth_proxy_logout_url.as_deref(),
            Some("https://auth.example.com/logout")
        );
    }

    #[test]
    fn test_boolean_flags() {
        for raw in ["true", "TRUE", "True", "1", " true "] {
            assert!(
                from_vars(&[("RDRS_MULTI_USER_ENABLED", raw)]).multi_user_enabled,
                "{raw} should enable"
            );
        }
        // Anything else is off.
        for raw in ["false", "0", "yes", "on", "", "  "] {
            assert!(
                !from_vars(&[("RDRS_MULTI_USER_ENABLED", raw)]).multi_user_enabled,
                "{raw} should not enable"
            );
        }
    }

    #[test]
    fn test_image_proxy_secret_sources() {
        let raw = STANDARD.encode([7u8; 32]);
        let (secret, generated) = load_secret(Some(raw));
        assert_eq!(secret, vec![7u8; 32]);
        assert!(!generated);

        let (secret, generated) = load_secret(Some("!".repeat(16)));
        assert_eq!(secret, "!".repeat(16).into_bytes());
        assert!(!generated);

        // Too short or unset → generated key.
        for raw in [Some("short".to_string()), None] {
            let (secret, generated) = load_secret(raw);
            assert_eq!(secret.len(), 32);
            assert!(generated);
        }
    }

    /// `parse_cookie_secure` for cases that must succeed.
    fn cookie_secure(raw: Option<&str>, public_base_url: Option<&str>) -> bool {
        parse_cookie_secure(raw, public_base_url).expect("valid RDRS_COOKIE_SECURE")
    }

    #[test]
    fn test_parse_cookie_secure_derives_from_public_base_url() {
        assert!(cookie_secure(None, Some("https://rdrs.example.com")));
        assert!(!cookie_secure(None, Some("http://localhost:8080")));
        assert!(!cookie_secure(None, None));
        assert!(cookie_secure(None, Some("  HTTPS://rdrs.example.com")));
        // A host that merely starts with "https" is not an https:// URL.
        assert!(!cookie_secure(None, Some("http://https.example.com")));
    }

    #[test]
    fn test_parse_cookie_secure_explicit_override() {
        assert!(cookie_secure(Some("true"), Some("http://localhost")));
        assert!(cookie_secure(Some("1"), None));
        assert!(cookie_secure(Some("TRUE"), None));
        assert!(!cookie_secure(
            Some("false"),
            Some("https://rdrs.example.com")
        ));
        assert!(!cookie_secure(Some("0"), Some("https://rdrs.example.com")));
        assert!(cookie_secure(Some(" true "), None));
        // Blank is "unset", not "off".
        assert!(cookie_secure(Some(""), Some("https://rdrs.example.com")));
        assert!(cookie_secure(Some("   "), Some("https://rdrs.example.com")));
    }

    #[test]
    fn test_parse_cookie_secure_rejects_unrecognized_value() {
        for raw in ["yes", "on", "enabled", "no", "off", "2", "tru"] {
            let err = parse_cookie_secure(Some(raw), Some("https://rdrs.example.com"))
                .expect_err("unrecognized RDRS_COOKIE_SECURE must be rejected");
            assert!(err.contains("RDRS_COOKIE_SECURE"), "{err}");
            assert!(err.contains(raw), "{err}");
        }
    }

    /// `parse_hsts` for cases that must succeed.
    fn hsts(raw: Option<&str>, public_base_url: Option<&str>) -> bool {
        parse_hsts(raw, public_base_url).expect("valid RDRS_HSTS")
    }

    #[test]
    fn test_parse_hsts_derives_from_public_base_url() {
        assert!(hsts(None, Some("https://rdrs.example.com")));
        assert!(!hsts(None, Some("http://localhost:8080")));
        assert!(!hsts(None, None));
        assert!(hsts(None, Some("  HTTPS://x")));
        // A host that merely starts with "https" is not an https:// URL.
        assert!(!hsts(None, Some("http://https.example.com")));
    }

    #[test]
    fn test_parse_hsts_explicit_override() {
        assert!(hsts(Some("true"), Some("http://localhost")));
        assert!(hsts(Some("1"), None));
        assert!(hsts(Some("TRUE"), None));
        assert!(!hsts(Some("false"), Some("https://rdrs.example.com")));
        assert!(!hsts(Some("0"), Some("https://rdrs.example.com")));
        assert!(hsts(Some(""), Some("https://rdrs.example.com")));
        assert!(hsts(Some("   "), Some("https://rdrs.example.com")));
    }

    #[test]
    fn test_parse_hsts_rejects_unrecognized_value() {
        for raw in ["yes", "on", "off", "2"] {
            let err = parse_hsts(Some(raw), Some("https://rdrs.example.com"))
                .expect_err("unrecognized RDRS_HSTS must be rejected");
            assert!(err.contains("RDRS_HSTS"), "{err}");
            assert!(err.contains(raw), "{err}");
        }
    }

    #[test]
    fn test_hsts_max_age_default_and_override() {
        let config = from_vars(&[("RDRS_PUBLIC_BASE_URL", "https://rdrs.example.com")]);
        assert_eq!(config.hsts_max_age, 31_536_000);

        let config = from_vars(&[
            ("RDRS_PUBLIC_BASE_URL", "https://rdrs.example.com"),
            ("RDRS_HSTS_MAX_AGE", "3600"),
        ]);
        assert_eq!(config.hsts_max_age, 3600);

        // 0 is valid: it is the recovery path for a mis-set declaration.
        let config = from_vars(&[
            ("RDRS_PUBLIC_BASE_URL", "https://rdrs.example.com"),
            ("RDRS_HSTS_MAX_AGE", "0"),
        ]);
        assert_eq!(config.hsts_max_age, 0);

        let err = Config::from_map(|k| (k == "RDRS_HSTS_MAX_AGE").then(|| "soon".into()))
            .expect_err("non-numeric RDRS_HSTS_MAX_AGE must fail startup");
        assert!(err.contains("RDRS_HSTS_MAX_AGE"), "{err}");
        assert!(err.contains("soon"), "{err}");
    }

    #[test]
    fn test_hsts_include_subdomains_default_and_override() {
        let config = from_vars(&[("RDRS_PUBLIC_BASE_URL", "https://rdrs.example.com")]);
        assert!(config.hsts_include_subdomains);

        let config = from_vars(&[
            ("RDRS_PUBLIC_BASE_URL", "https://rdrs.example.com"),
            ("RDRS_HSTS_INCLUDE_SUBDOMAINS", "false"),
        ]);
        assert!(!config.hsts_include_subdomains);

        let err =
            Config::from_map(|k| (k == "RDRS_HSTS_INCLUDE_SUBDOMAINS").then(|| "sometimes".into()))
                .expect_err("unrecognized RDRS_HSTS_INCLUDE_SUBDOMAINS must fail startup");
        assert!(err.contains("RDRS_HSTS_INCLUDE_SUBDOMAINS"), "{err}");
    }

    #[test]
    fn test_hsts_header_value_never_contains_preload() {
        // Pins the decision: preload is effectively irreversible.
        let config = Config {
            hsts: true,
            hsts_max_age: 31_536_000,
            hsts_include_subdomains: true,
            ..test_config()
        };
        let value = config.hsts_header_value().unwrap();
        assert!(!value.contains("preload"), "{value}");
        assert_eq!(value, "max-age=31536000; includeSubDomains");
    }

    #[test]
    fn test_hsts_header_value_off_by_default() {
        let config = test_config();
        assert!(!config.hsts);
        assert!(config.hsts_header_value().is_none());
    }

    #[test]
    fn test_hsts_header_value_include_subdomains_toggle() {
        let with = Config {
            hsts: true,
            hsts_max_age: 100,
            hsts_include_subdomains: true,
            ..test_config()
        };
        assert_eq!(
            with.hsts_header_value().unwrap(),
            "max-age=100; includeSubDomains"
        );

        let without = Config {
            hsts_include_subdomains: false,
            ..with
        };
        assert_eq!(without.hsts_header_value().unwrap(), "max-age=100");
    }

    #[test]
    fn test_classify_backend() {
        assert_eq!(classify_backend("rdrs.sqlite3"), Backend::Sqlite);
        assert_eq!(classify_backend("/var/lib/rdrs/data.db"), Backend::Sqlite);
        assert_eq!(classify_backend("sqlite://rdrs.sqlite3"), Backend::Sqlite);
        assert_eq!(
            classify_backend("postgres://user:pw@localhost/rdrs"),
            Backend::Postgres
        );
        assert_eq!(
            classify_backend("postgresql://user@db:5432/rdrs"),
            Backend::Postgres
        );
        assert_eq!(classify_backend("  POSTGRES://x"), Backend::Postgres);
    }

    #[test]
    fn test_redact_database_url() {
        assert_eq!(
            redact_database_url("postgres://user:s3cr3t@db.internal:5432/rdrs"),
            "postgres://user:***@db.internal:5432/rdrs"
        );
        assert_eq!(
            redact_database_url("postgres://u:p@host/rdrs?sslmode=require"),
            "postgres://u:***@host/rdrs?sslmode=require"
        );
        // A '@' inside the password does not confuse the split.
        assert_eq!(
            redact_database_url("postgres://user:p@ss@host/rdrs"),
            "postgres://user:***@host/rdrs"
        );
        assert_eq!(
            redact_database_url("postgres://user@host/rdrs"),
            "postgres://user@host/rdrs"
        );
        assert_eq!(redact_database_url("rdrs.sqlite3"), "rdrs.sqlite3");
        assert_eq!(
            redact_database_url("sqlite:///data/rdrs.sqlite3"),
            "sqlite:///data/rdrs.sqlite3"
        );
    }

    #[test]
    fn test_validate_accepts_both_backends() {
        let mut config = test_config();
        config.database_url = "postgres://user@localhost/rdrs".to_string();
        assert!(config.validate().is_ok());

        config.database_url = "rdrs.sqlite3".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn setup_is_open_only_on_an_empty_instance() {
        let config = test_config();
        assert!(config.can_setup(0));
        assert!(!config.can_setup(1));

        // Multi-user does not reopen setup.
        let multi = Config {
            multi_user_enabled: true,
            ..config
        };
        assert!(!multi.can_setup(1));
    }

    #[test]
    fn admin_account_creation_follows_multi_user() {
        let config = test_config();
        assert!(config.can_create_account(0));
        assert!(!config.can_create_account(1));

        let multi = Config {
            multi_user_enabled: true,
            ..config
        };
        assert!(multi.can_create_account(0));
        assert!(multi.can_create_account(5));
    }

    #[test]
    fn a_retired_variable_refuses_startup() {
        let err = Config::from_map(|k| (k == "RDRS_SIGNUP_ENABLED").then(|| "true".to_string()))
            .expect_err("a retired variable must refuse startup");
        assert!(err.contains("RDRS_SIGNUP_ENABLED"), "{err}");
        assert!(
            err.contains("/admin"),
            "the message must say what replaced it: {err}"
        );

        assert!(Config::from_map(|k| (k == "RDRS_SIGNUP_ENABLED").then(String::new)).is_ok());
    }

    #[test]
    fn the_pre_prefix_signup_flag_is_refused_once() {
        // Must be refused as retired, not pointed at a name that is refused too.
        let err = Config::from_map(|k| (k == "SIGNUP_ENABLED").then(|| "true".to_string()))
            .expect_err("the pre-prefix signup flag must refuse startup");
        assert!(
            err.contains("no longer configure anything"),
            "it must be refused as retired, not as renamed: {err}"
        );
        assert!(
            !err.contains("-> RDRS_SIGNUP_ENABLED"),
            "it must not point at a name that is refused too: {err}"
        );
        assert!(err.contains("/admin"), "{err}");
    }

    #[test]
    fn test_parse_trusted_networks() {
        let nets = parse_trusted_networks("10.0.0.0/8, 192.168.1.0/24 , 127.0.0.1").unwrap();
        assert_eq!(nets.len(), 3);
        assert!(parse_trusted_networks("").unwrap().is_empty());
        assert!(parse_trusted_networks("not-an-ip").is_err());
    }

    #[test]
    fn test_is_trusted_peer() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        assert!(cfg.is_trusted_peer("10.1.2.3".parse().unwrap()));
        assert!(!cfg.is_trusted_peer("192.168.0.1".parse().unwrap()));
    }

    #[test]
    fn test_client_ip() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        // (peer, X-Forwarded-For, X-Real-IP, expected client)
        for (peer, xff, real_ip, expected) in [
            // An untrusted peer's forwarding headers are ignored.
            (Some("203.0.113.1"), Some("8.8.8.8"), None, "203.0.113.1"),
            // KEY case: a client-supplied spoof on the left is not believed.
            (
                Some("10.0.0.1"),
                Some("8.8.8.8, 203.0.113.9"),
                None,
                "203.0.113.9",
            ),
            // Trusted hops are skipped.
            (
                Some("10.0.0.1"),
                Some("203.0.113.9, 10.0.0.5"),
                None,
                "203.0.113.9",
            ),
            // All-trusted XFF falls back to X-Real-IP.
            (
                Some("10.0.0.1"),
                Some("10.0.0.5, 10.0.0.6"),
                Some("198.51.100.7"),
                "198.51.100.7",
            ),
            (Some("10.1.2.3"), None, None, "10.1.2.3"),
            (None, None, None, "127.0.0.1"),
            // Malformed right-most token: bail to the peer, not the spoof.
            (
                Some("10.0.0.1"),
                Some("8.8.8.8, not-an-ip"),
                None,
                "10.0.0.1",
            ),
        ] {
            let mut headers = axum::http::HeaderMap::new();
            if let Some(xff) = xff {
                headers.insert("x-forwarded-for", xff.parse().unwrap());
            }
            if let Some(real_ip) = real_ip {
                headers.insert("x-real-ip", real_ip.parse().unwrap());
            }
            let peer = peer.map(|p| p.parse::<IpAddr>().unwrap());
            assert_eq!(
                cfg.client_ip(peer, &headers),
                expected.parse::<IpAddr>().unwrap(),
                "peer {peer:?}, XFF {xff:?}, X-Real-IP {real_ip:?}"
            );
        }
    }

    #[test]
    fn test_validate_header_requires_trusted_networks() {
        let bad = Config {
            auth_proxy_header: "Remote-User".to_string(),
            trusted_proxy_networks: Vec::new(),
            ..test_config()
        };
        assert!(bad.validate().is_err());

        let good = Config {
            auth_proxy_header: "Remote-User".to_string(),
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        assert!(good.validate().is_ok());
    }

    #[test]
    fn test_validate_disable_local_auth_requires_header() {
        let bad = Config {
            disable_local_auth: true,
            auth_proxy_header: String::new(),
            ..test_config()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_group_mapping_enabled() {
        let off = test_config();
        assert!(!off.group_mapping_enabled());
        let on = Config {
            auth_proxy_groups_header: "Remote-Groups".to_string(),
            auth_proxy_admin_group: "admins".to_string(),
            ..test_config()
        };
        assert!(on.group_mapping_enabled());
    }

    #[test]
    fn test_webauthn_rp_warning() {
        let config = test_config();
        assert!(config.webauthn_rp_warning().is_some());

        let deployed = Config {
            webauthn_rp_id: "rdrs.example.com".to_string(),
            webauthn_rp_origin: "https://rdrs.example.com".to_string(),
            ..test_config()
        };
        assert!(deployed.webauthn_rp_warning().is_none());

        // Trailing slash ignored.
        let matched = Config {
            public_base_url: Some("https://rdrs.example.com/".to_string()),
            ..deployed.clone()
        };
        assert!(matched.webauthn_rp_warning().is_none());

        let mismatched = Config {
            public_base_url: Some("https://reader.example.com".to_string()),
            ..deployed
        };
        assert!(mismatched.webauthn_rp_warning().is_some());
    }

    #[test]
    fn test_login_rate_limit_defaults_when_unset() {
        let config = from_vars(&[]);
        assert_eq!(config.login_rate_limit_attempts, LOGIN_MAX_ATTEMPTS);
        assert_eq!(config.login_rate_limit_window_secs, LOGIN_WINDOW_SECS);

        let config = from_vars(&[
            ("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "  "),
            ("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS", ""),
        ]);
        assert_eq!(config.login_rate_limit_attempts, LOGIN_MAX_ATTEMPTS);
        assert_eq!(config.login_rate_limit_window_secs, LOGIN_WINDOW_SECS);
    }

    #[test]
    fn test_login_rate_limit_explicit_override() {
        let config = from_vars(&[
            ("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "10"),
            ("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS", "120"),
        ]);
        assert_eq!(config.login_rate_limit_attempts, 10);
        assert_eq!(config.login_rate_limit_window_secs, 120);

        // 0 disables the limiter.
        let config = from_vars(&[("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "0")]);
        assert_eq!(config.login_rate_limit_attempts, 0);
    }

    #[test]
    fn test_login_rate_limit_non_numeric_value_is_a_hard_error() {
        let err =
            Config::from_map(|k| (k == "RDRS_LOGIN_RATE_LIMIT_ATTEMPTS").then(|| "five".into()))
                .expect_err("non-numeric RDRS_LOGIN_RATE_LIMIT_ATTEMPTS must fail startup");
        assert!(err.contains("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS"), "{err}");
        assert!(err.contains("five"), "{err}");

        let err =
            Config::from_map(|k| (k == "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS").then(|| "-1".into()))
                .expect_err("non-numeric RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS must fail startup");
        assert!(err.contains("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS"), "{err}");
        assert!(err.contains("-1"), "{err}");
    }

    #[test]
    fn test_login_rate_limit_zero_window_is_a_hard_error() {
        let err =
            Config::from_map(|k| (k == "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS").then(|| "0".into()))
                .expect_err("a zero RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS must fail startup");
        assert!(err.contains("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS"), "{err}");
        assert!(err.contains('0'), "{err}");
    }

    #[test]
    fn test_rate_limit_proxy_warning() {
        let config = test_config();
        assert!(config.rate_limit_proxy_warning().is_some());

        let with_proxies = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        assert!(with_proxies.rate_limit_proxy_warning().is_none());

        // Limiter disabled → no warning.
        let disabled = Config {
            login_rate_limit_attempts: 0,
            trusted_proxy_networks: Vec::new(),
            ..test_config()
        };
        assert!(disabled.rate_limit_proxy_warning().is_none());
    }
}
