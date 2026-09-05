use base64::{Engine, engine::general_purpose::STANDARD};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rand::Rng;
use std::env;
use std::net::{IpAddr, SocketAddr};

use crate::utils::url_validation::FetchPolicy;

/// Default user agent for HTTP requests (transparent and responsible crawling)
pub const DEFAULT_USER_AGENT: &str = concat!(
    "RDRS/",
    env!("GIT_VERSION"),
    " (RSS Reader; +https://github.com/henry40408/rdrs)"
);

/// Values the feed-edit form suggests for a per-feed `User-Agent` override.
///
/// The field exists for the one case [`DEFAULT_USER_AGENT`] does not survive: a
/// server that turns away anything not shaped like a browser. So the list is
/// browser strings, the shape a blank box gives no hint of.
///
/// [`DEFAULT_USER_AGENT`] is deliberately **not** among them: leaving the field
/// empty already selects it, and picking it from a list would freeze today's
/// `GIT_VERSION` into the feed's row.
///
/// Every entry must survive `HeaderValue::from_str` — `feed_sync::refresh_feed`
/// *drops* the header on failure rather than erroring, so a bad suggestion would
/// silently send no `User-Agent` at all. Enforced by
/// `custom_user_agent_suggestions_are_valid_header_values`.
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
    /// Process-wide root key backing every signature rdrs produces (session
    /// cookies, image-proxy URLs, the `GReader` post token). See [`crate::secret`]
    /// for the domain-separated derivation; each use derives its own tag.
    pub secret: Vec<u8>,
    /// Whether [`Config::secret`] was randomly generated because `RDRS_SECRET`
    /// was unset or too short. Drives the startup warning: a generated key ends
    /// every browser session and breaks every cached image-proxy URL on restart.
    pub secret_generated: bool,
    pub user_agent: String,
    /// Hosts the SSRF guard lets the feed, icon and discovery fetchers reach
    /// despite resolving inward — from `RDRS_FETCH_ALLOW_PRIVATE_HOSTS`. Empty
    /// by default, which is the safe reading of "a feed URL is attacker
    /// influenced"; a self-hoster subscribing to something on their own LAN
    /// names it here.
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
    /// Attempts allowed per client IP per [`Config::login_rate_limit_window_secs`],
    /// applied separately to each credential-accepting endpoint *class* (see
    /// [`crate::middleware::rate_limit::Bucket`]), so a registration refused by
    /// configuration cannot also exhaust the login budget for the same IP. `0`
    /// disables the limiter entirely, for deployments that throttle upstream.
    pub login_rate_limit_attempts: u32,
    /// Fixed-window length, in seconds, for [`Config::login_rate_limit_attempts`].
    pub login_rate_limit_window_secs: u64,
    /// Whether to send `Strict-Transport-Security` on every response. See
    /// [`parse_hsts`] for the derivation rule and why an unrecognized value is
    /// a hard startup error rather than a silent "off".
    pub hsts: bool,
    /// HSTS `max-age` in seconds, defaulting to OWASP's recommended one year.
    /// `0` is an escape hatch rather than a synonym for "off": it tells browsers
    /// to *forget* a previous declaration, which is the supported way to recover
    /// from a mis-set one.
    pub hsts_max_age: u64,
    /// Whether the HSTS declaration includes `; includeSubDomains`. Defaults to
    /// on; see [`Config::hsts_header_value`] for the apex-domain caveat that
    /// makes the escape hatch necessary.
    pub hsts_include_subdomains: bool,
}

/// Parse a comma-separated list of CIDR networks or bare IPs into `IpNet`s.
/// Whitespace around entries and empty entries are ignored. A bare IP becomes
/// a host route (`/32` or `/128`).
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

/// Whether the session cookie should carry the `Secure` attribute.
///
/// An explicit `RDRS_COOKIE_SECURE` wins; otherwise it is derived from
/// `RDRS_PUBLIC_BASE_URL`'s scheme. Deriving beats a standalone knob because a
/// real deployment already has to set that URL correctly, so an HTTPS install
/// gets `Secure` without a second setting to forget — while a plain `http://`
/// dev run keeps working. That last part matters: a browser silently drops a
/// `Secure` cookie sent over HTTP, so defaulting it on would lock a developer
/// out with no visible error. The override exists for TLS-terminating setups
/// that cannot advertise their public URL here.
///
/// Unlike the other boolean env vars, an unrecognized value is a hard error:
/// those default to `false`, so a typo is a no-op, but here the derived default
/// can be `true` and reading `RDRS_COOKIE_SECURE=yes` as "off" would strip
/// `Secure` from a correctly-configured HTTPS deployment.
pub fn parse_cookie_secure(
    raw: Option<&str>,
    public_base_url: Option<&str>,
) -> Result<bool, String> {
    let derived = public_base_url
        .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("https://"));
    parse_bool_derived(raw, "RDRS_COOKIE_SECURE", derived)
}

/// Whether `Strict-Transport-Security` should be sent on every response.
///
/// An explicit `RDRS_HSTS` wins; otherwise derived from
/// `RDRS_PUBLIC_BASE_URL`'s scheme, the same rule and reasoning as
/// [`parse_cookie_secure`]: an HTTPS install gets HSTS without a second setting
/// to forget, while a plain `http://` deployment cannot lock itself out.
///
/// HSTS is *sticky* — a browser that has seen the header refuses plain HTTP for
/// the whole `max-age`, and the server cannot retract it — so an unrecognized
/// value is more dangerous here than anywhere else. Reading a typo as "off"
/// leaves an HTTPS deployment unprotected; reading it as "on" can lock browsers
/// out of a plain-HTTP one with no server-side way back. Only
/// `true`/`false`/`1`/`0` are accepted.
pub fn parse_hsts(raw: Option<&str>, public_base_url: Option<&str>) -> Result<bool, String> {
    let derived = public_base_url
        .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("https://"));
    parse_bool_derived(raw, "RDRS_HSTS", derived)
}

/// Shared strict boolean parser behind [`parse_cookie_secure`] and
/// [`parse_hsts`]: unset or blank falls back to `derived`, `true`/`1` and
/// `false`/`0` are recognized case-insensitively, anything else is a hard
/// startup error naming `var_name`. Both callers have a derived default that
/// can be `true`, which is why neither can use the lenient [`flag`] helper.
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

/// Which database engine `database_url` selects. Determined once at startup —
/// rdrs runs against exactly one backend for the life of the process (no
/// mid-flight switching). See the migration spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

/// Classify a `database_url` into a [`Backend`] by scheme. A `postgres://` or
/// `postgresql://` URL selects `Postgres`; anything else — a `sqlite://` URL or
/// a bare file path like `rdrs.sqlite3` — selects `SQLite`.
pub fn classify_backend(database_url: &str) -> Backend {
    let lower = database_url.trim_start().to_ascii_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        Backend::Postgres
    } else {
        Backend::Sqlite
    }
}

/// Redact the password out of a `database_url` so it is safe to display.
///
/// A `PostgreSQL` URL carries credentials inline, and the settings page renders
/// the running instance's `DATABASE_URL`. Only the password component is
/// replaced — scheme, user, host and database name stay legible, which is what
/// the page is for. `SQLite` paths have no userinfo and pass through.
pub fn redact_database_url(database_url: &str) -> String {
    let Some((scheme, rest)) = database_url.split_once("://") else {
        return database_url.to_string();
    };
    // The authority ends at the first '/', '?' or '#'. Split userinfo off at
    // the *last* '@' within it: an unencoded '@' in the password would
    // otherwise leave part of the secret in the "host" half.
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

/// Resolve `RDRS_SERVER_BIND` into a [`SocketAddr`]. Unset or empty yields
/// `127.0.0.1:8080`, so a bare-metal run is not exposed on all interfaces
/// without opting in; anything else must be a valid `host:port`. The container
/// image sets `0.0.0.0:8080` so a proxy in another container can reach it.
pub fn parse_server_bind(raw: Option<&str>) -> Result<SocketAddr, String> {
    match raw {
        Some(v) if !v.is_empty() => v
            .parse::<SocketAddr>()
            .map_err(|e| format!("invalid RDRS_SERVER_BIND '{v}': {e}")),
        _ => Ok(SocketAddr::from(([127, 0, 0, 1], 8080))),
    }
}

/// Resolve `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` into the attempt budget for
/// [`Config::login_rate_limit_attempts`]. Unset or blank yields
/// [`crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS`]; anything else must
/// parse as a `u32` (`0` is valid and disables the limiter). Unparseable input
/// is a hard startup error: silently keeping the default would leave a typo
/// looking configured while the protection ran on defaults.
fn parse_login_rate_limit_attempts(raw: Option<&str>) -> Result<u32, String> {
    match raw {
        Some(v) => v
            .parse::<u32>()
            .map_err(|e| format!("invalid RDRS_LOGIN_RATE_LIMIT_ATTEMPTS '{v}': {e}")),
        None => Ok(crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS),
    }
}

/// Resolve `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` into the window length. Same
/// rules as [`parse_login_rate_limit_attempts`], plus a parsed `0` is rejected:
/// the window elapses the instant it is recorded, so every attempt starts a
/// fresh one and the limiter never throttles while still looking configured.
/// `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=0` is the way to disable it on purpose.
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

/// Resolve the root signing key from a raw `RDRS_SECRET`, returning the key
/// bytes and whether they were generated rather than configured. A base64 value
/// is decoded, otherwise the raw bytes are used; either way at least
/// [`crate::secret::MIN_SECRET_LEN`] bytes are required, and a shorter value is
/// discarded for a fresh random key rather than used as a guessable one.
///
/// Deliberately not routed through [`nonblank`]: trimming would change the key
/// bytes for a deployment whose value carries whitespace, rotating the key —
/// ending every session and breaking every cached image-proxy URL.
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

    // Generate a random 32-byte secret
    let mut secret = vec![0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    (secret, true)
}

/// Read `key` through `get` and normalize it: surrounding whitespace trimmed,
/// and an empty result counts as unset.
///
/// Every string-valued setting goes through here, so `FOO=` in a compose file
/// means "not configured". The distinction is load-bearing for settings
/// disabled by being empty — a whitespace-only `RDRS_AUTH_PROXY_HEADER` would
/// otherwise enable forward auth against a header no proxy can send.
fn nonblank(get: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Whether a boolean env var is on. `true` (any case) and `1` are the only
/// accepted values; anything else reads as off, which is safe because every
/// setting using this defaults to off. `RDRS_COOKIE_SECURE` deliberately does
/// not — its default can be `true`. See [`parse_cookie_secure`].
fn flag(get: &impl Fn(&str) -> Option<String>, key: &str) -> bool {
    nonblank(get, key).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// Every variable renamed by the `RDRS_` prefix migration, old name → new name.
///
/// `IMAGE_PROXY_SECRET` is the one entry that is not a straight prefixing: it
/// became `RDRS_SECRET` once the key stopped signing only image-proxy URLs.
///
/// `DATABASE_URL` is deliberately absent — it is a genuine cross-tool
/// convention. The rest only *look* generic; `USER_AGENT` and `SERVER_BIND` are
/// rdrs's own names, which is what makes them collide in a shared compose file.
pub const RENAMED_VARS: &[(&str, &str)] = &[
    ("SERVER_BIND", "RDRS_SERVER_BIND"),
    // `SIGNUP_ENABLED` is deliberately absent — see `RETIRED_VARS`. Renaming it
    // here would only route the operator to a name that is then refused too.
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
    // Not read here — clap binds it on `Args` in `main` — but listed so the
    // check still catches it. `main` parses args before building the config, so
    // an old `LOG_FORMAT` is honoured for the few lines until this rejects it.
    ("LOG_FORMAT", "RDRS_LOG_FORMAT"),
];

/// Variables that no longer configure anything, with what replaced them.
///
/// Distinct from [`RENAMED_VARS`]: there is no new name to move the value to,
/// the feature itself is gone. Still a startup refusal, and for a sharper
/// reason than a rename — `RDRS_SIGNUP_ENABLED=true` reads as "anyone may sign
/// up", and ignoring it would leave an operator believing a public registration
/// form exists when the endpoint has been removed.
///
/// The pre-prefix `SIGNUP_ENABLED` is listed here rather than in
/// [`RENAMED_VARS`], even though it was also renamed: [`reject_legacy_vars`]
/// runs first, so a rename entry would send the operator to
/// `RDRS_SIGNUP_ENABLED` only for the next boot to refuse that too.
pub const RETIRED_VARS: &[(&str, &str)] = &[
    ("RDRS_SIGNUP_ENABLED", SIGNUP_RETIRED),
    ("SIGNUP_ENABLED", SIGNUP_RETIRED),
];

/// Why both spellings of the signup flag no longer configure anything.
const SIGNUP_RETIRED: &str = "self-service registration was removed; an admin now creates accounts from \
     /admin and hands out a one-time link. The first account is still created \
     at /setup on a fresh install";

/// Refuse to start when a retired variable still carries a value.
///
/// See [`RETIRED_VARS`] for why this is a refusal and not a warning.
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

/// Refuse to start when a pre-prefix variable name still carries a value.
///
/// Ignoring the old name is the worst of the three options: an operator who
/// upgrades without editing their compose file gets a *working* server running
/// on defaults — a fresh empty database, signup off, a regenerated secret —
/// instead of their actual deployment. A warning fares little better, since the
/// same wrong server comes up and the line scrolls past.
///
/// Only a value that survives [`nonblank`] counts: `FOO=` configured nothing
/// before the rename either.
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

    /// The key third-party service credentials are encrypted with at rest, or
    /// `None` when there is nothing durable to encrypt with.
    ///
    /// A generated [`Config::secret`] is new on every restart. Encrypting with
    /// it would turn a restart into permanent loss of the user's Linkding and
    /// Kagi tokens — worse than the plaintext storage it replaces, and for no
    /// gain, since a key that only exists in this process protects nothing that
    /// outlives it. So an install that never set `RDRS_SECRET` keeps storing
    /// them as it did before, and `/admin` says so.
    pub fn service_token_key(&self) -> Option<&[u8]> {
        (!self.secret_generated).then_some(self.secret.as_slice())
    }

    /// Build the config from an arbitrary key→value lookup.
    ///
    /// `from_env` is the one-line adapter over the real environment; tests pass
    /// a closure instead. That keeps config tests pure — mutating the process
    /// environment is `unsafe` under edition 2024 and only survives because
    /// nextest forks per test, a property of the runner rather than the code.
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
        // Passed raw, not through `nonblank`: `parse_cookie_secure` does its own
        // trimming and has to tell "unset" from "unrecognized" itself.
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

        // Passed raw, not through `nonblank`, for the same reason as
        // `cookie_secure` above: `parse_hsts` does its own trimming and has to
        // tell "unset" from "unrecognized" itself.
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

    /// The originating client IP. `X-Forwarded-For` / `X-Real-IP` are honoured
    /// ONLY when the TCP peer is a trusted proxy (`is_trusted_peer`); otherwise
    /// they are attacker-controlled and the peer address is used.
    ///
    /// `X-Forwarded-For` is read RIGHT-to-left: each hop *appends* the address
    /// it saw, so the real client is the right-most entry that is not itself one
    /// of our trusted proxies. Taking the left-most instead would let any client
    /// forge its own logged IP.
    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &axum::http::HeaderMap) -> IpAddr {
        let Some(peer) = peer else {
            return IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        };
        // A direct (untrusted) client connection: its TCP source is the client;
        // forwarded headers from it are attacker-controlled and ignored.
        if !self.is_trusted_peer(peer) {
            return peer;
        }
        // Each hop APPENDS the address it saw, so the real client is the
        // RIGHT-MOST entry that is not one of our own trusted proxies; leftmost
        // is forgeable under append-mode proxies.
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            for part in xff.rsplit(',') {
                let Ok(ip) = part.trim().parse::<IpAddr>() else {
                    // A malformed hop breaks the trust chain — do not believe any
                    // entry further to the left (they may be client-supplied).
                    break;
                };
                if !self.is_trusted_peer(ip) {
                    return ip;
                }
                // Trusted proxy hop: keep walking left.
            }
        }
        // No untrusted XFF entry (all hops trusted, or no XFF): fall back to
        // `X-Real-IP` (a single value the proxy sets), then the peer itself.
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

    /// Whether the one-time first-run setup page is still open.
    ///
    /// True only while the instance has no accounts at all. There is nothing to
    /// enumerate at that point, which is what makes an anonymous
    /// account-creating endpoint acceptable here and nowhere else. It closes for
    /// good the moment the first account exists.
    pub fn can_setup(&self, user_count: i64) -> bool {
        user_count == 0
    }

    /// Whether an admin may create *another* account.
    ///
    /// `RDRS_MULTI_USER_ENABLED` keeps its meaning from the self-service era —
    /// "is this a single-user deployment?" — it just governs the admin's button
    /// now instead of a public form.
    pub fn can_create_account(&self, user_count: i64) -> bool {
        user_count == 0 || self.multi_user_enabled
    }

    /// A startup warning about `WebAuthn` relying-party config that would
    /// silently break passkeys in a real deployment: the RP origin still points
    /// at `localhost`, or disagrees with `RDRS_PUBLIC_BASE_URL`.
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

    /// The HSTS header value, or `None` when the header must not be sent.
    ///
    /// Deliberately **never contains `preload`**: entering the preload list is
    /// effectively irreversible, so it must never follow from a default. An
    /// operator who wants it can add it at their reverse proxy.
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

    /// A startup warning about the credential rate limiter running without a
    /// trusted-proxy list. [`Config::client_ip`] then falls back to the TCP peer
    /// for every request, so behind a reverse proxy every visitor collapses into
    /// the proxy's one address and a single abuser can lock out every real
    /// user.
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
        // Unset or empty → default 127.0.0.1:8080 (loopback only).
        assert_eq!(
            parse_server_bind(None).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        assert_eq!(
            parse_server_bind(Some("")).unwrap(),
            std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
        );
        // A valid host:port is honored, incl. a loopback-only bind.
        assert_eq!(
            parse_server_bind(Some("127.0.0.1:9000")).unwrap(),
            "127.0.0.1:9000".parse().unwrap()
        );
        // Invalid input fails with a descriptive error; a bare host with no
        // port is not a SocketAddr.
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
        // A pre-prefix name still carrying a value fails startup rather than
        // being ignored, which would boot a *working* server on defaults —
        // against an empty rdrs.sqlite3 rather than the operator's database.
        let err = Config::from_map(|k| (k == "SERVER_BIND").then(|| "0.0.0.0:8080".into()))
            .expect_err("a legacy name must fail startup");
        assert!(err.contains("SERVER_BIND -> RDRS_SERVER_BIND"), "{err}");

        // The rename is spelled out for every stale variable at once, so a
        // migration takes one restart rather than one per variable.
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

        // Blank is not "still configured": `FOO=` left in a compose file
        // configured nothing before the rename either.
        assert!(Config::from_map(|k| (k == "SERVER_BIND").then(|| "  ".into())).is_ok());

        // DATABASE_URL keeps its bare name — it is a real cross-tool
        // convention, so it must not be caught by the legacy check.
        let config = from_vars(&[("DATABASE_URL", "postgres://u:p@db/rdrs")]);
        assert_eq!(config.backend(), Backend::Postgres);
    }

    #[test]
    fn test_server_bind_drives_listener_and_rp_origin() {
        let config = from_vars(&[("RDRS_SERVER_BIND", "127.0.0.1:9137")]);
        assert_eq!(config.server_bind, "127.0.0.1:9137".parse().unwrap());
        // The RDRS_WEBAUTHN_RP_ORIGIN default derives its port from RDRS_SERVER_BIND.
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
        // Nothing configured means no persistent secret, which the startup
        // warning in `main` reports.
        assert!(config.secret_generated);
    }

    /// The `<datalist>` promise for the per-feed user agent. `refresh_feed`
    /// builds the header with `HeaderValue::from_str` and *drops it on failure*,
    /// so a suggestion that cannot become a header value would leave the feed
    /// sending no `User-Agent` at all — silently, and only for the operator who
    /// picked it out of the dropdown.
    #[test]
    fn custom_user_agent_suggestions_are_valid_header_values() {
        assert!(!CUSTOM_USER_AGENT_SUGGESTIONS.is_empty());

        for ua in CUSTOM_USER_AGENT_SUGGESTIONS {
            assert!(
                reqwest::header::HeaderValue::from_str(ua).is_ok(),
                "suggestion {ua:?} cannot be sent as a header value"
            );
        }

        // Offering the default would freeze today's GIT_VERSION into the
        // feed's row; leaving the field empty already selects it.
        assert!(
            !CUSTOM_USER_AGENT_SUGGESTIONS.contains(&DEFAULT_USER_AGENT),
            "the default belongs to the empty field, not the list"
        );
    }

    #[test]
    fn test_blank_values_count_as_unset() {
        // A variable present but empty (or whitespace-only) must fall back to
        // the default, not override it with nothing. `FOO=` is the common shape
        // in a compose file where a value was left to be filled in later.
        let config = from_vars(&[
            ("DATABASE_URL", "  "),
            ("RDRS_USER_AGENT", ""),
            ("RDRS_PUBLIC_BASE_URL", "   "),
            ("RDRS_AUTH_PROXY_LOGOUT_URL", " "),
            // Blank here must leave forward auth *off*: a whitespace header
            // name would otherwise pass `auth_proxy_enabled` and then fail
            // `validate`, refusing to boot over a variable nobody meant to set.
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
        // The header name is compared against a real HTTP header, so a stray
        // space from a compose file must not survive into the lookup.
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
        // Anything else is off. These all default to off, so a typo is a no-op
        // rather than a silent downgrade (unlike RDRS_COOKIE_SECURE, which rejects).
        for raw in ["false", "0", "yes", "on", "", "  "] {
            assert!(
                !from_vars(&[("RDRS_MULTI_USER_ENABLED", raw)]).multi_user_enabled,
                "{raw} should not enable"
            );
        }
    }

    #[test]
    fn test_image_proxy_secret_sources() {
        // A base64 value decoding to >= 16 bytes is used decoded.
        let raw = STANDARD.encode([7u8; 32]);
        let (secret, generated) = load_secret(Some(raw));
        assert_eq!(secret, vec![7u8; 32]);
        assert!(!generated);

        // A non-base64 value of at least 16 characters is used as raw bytes.
        let (secret, generated) = load_secret(Some("!".repeat(16)));
        assert_eq!(secret, "!".repeat(16).into_bytes());
        assert!(!generated);

        // Too short to be trusted, and unset, both fall back to a generated key
        // rather than a guessable one.
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
        // Unset → follow RDRS_PUBLIC_BASE_URL's scheme.
        assert!(cookie_secure(None, Some("https://rdrs.example.com")));
        assert!(!cookie_secure(None, Some("http://localhost:8080")));
        // No RDRS_PUBLIC_BASE_URL at all → off, so a bare `cargo run` stays usable.
        assert!(!cookie_secure(None, None));
        // Scheme match is case-insensitive and tolerant of leading whitespace,
        // matching `classify_backend`'s handling of DATABASE_URL.
        assert!(cookie_secure(None, Some("  HTTPS://rdrs.example.com")));
        // A host that merely starts with "https" is not an https:// URL.
        assert!(!cookie_secure(None, Some("http://https.example.com")));
    }

    #[test]
    fn test_parse_cookie_secure_explicit_override() {
        // An explicit value wins over the derived one, in both directions.
        assert!(cookie_secure(Some("true"), Some("http://localhost")));
        assert!(cookie_secure(Some("1"), None));
        assert!(cookie_secure(Some("TRUE"), None));
        assert!(!cookie_secure(
            Some("false"),
            Some("https://rdrs.example.com")
        ));
        assert!(!cookie_secure(Some("0"), Some("https://rdrs.example.com")));
        // Surrounding whitespace is trimmed off a real value.
        assert!(cookie_secure(Some(" true "), None));
        // Empty / whitespace-only is "unset", not "off" — an empty env var
        // must not silently disable the derived value.
        assert!(cookie_secure(Some(""), Some("https://rdrs.example.com")));
        assert!(cookie_secure(Some("   "), Some("https://rdrs.example.com")));
    }

    #[test]
    fn test_parse_cookie_secure_rejects_unrecognized_value() {
        // A typo must not silently strip `Secure` from an HTTPS deployment, so
        // anything outside true/false/1/0 fails startup instead of being read
        // as "off".
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
        // Unset → follow RDRS_PUBLIC_BASE_URL's scheme, exactly like
        // parse_cookie_secure.
        assert!(hsts(None, Some("https://rdrs.example.com")));
        assert!(!hsts(None, Some("http://localhost:8080")));
        // No RDRS_PUBLIC_BASE_URL at all → off, so a plain-HTTP internal
        // deployment cannot lock itself out.
        assert!(!hsts(None, None));
        // Scheme match is case-insensitive and tolerant of leading whitespace.
        assert!(hsts(None, Some("  HTTPS://x")));
        // A host that merely starts with "https" is not an https:// URL — the
        // shared helper must not regress this trap.
        assert!(!hsts(None, Some("http://https.example.com")));
    }

    #[test]
    fn test_parse_hsts_explicit_override() {
        // An explicit value wins over the derived one, in both directions.
        assert!(hsts(Some("true"), Some("http://localhost")));
        assert!(hsts(Some("1"), None));
        assert!(hsts(Some("TRUE"), None));
        assert!(!hsts(Some("false"), Some("https://rdrs.example.com")));
        assert!(!hsts(Some("0"), Some("https://rdrs.example.com")));
        // Empty / whitespace-only is "unset", not "off".
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

        // 0 is a valid, meaningful override: the documented recovery path for
        // a mis-set HSTS declaration.
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
        // Defaults to on.
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
        // Pins the irreversibility decision (entering the preload list cannot
        // be undone quickly) so nobody "improves" this later by adding it.
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
        // scheme match is case-insensitive and tolerant of leading whitespace
        assert_eq!(classify_backend("  POSTGRES://x"), Backend::Postgres);
    }

    #[test]
    fn test_redact_database_url() {
        // Password stripped, everything else legible.
        assert_eq!(
            redact_database_url("postgres://user:s3cr3t@db.internal:5432/rdrs"),
            "postgres://user:***@db.internal:5432/rdrs"
        );
        // Query parameters after the authority survive.
        assert_eq!(
            redact_database_url("postgres://u:p@host/rdrs?sslmode=require"),
            "postgres://u:***@host/rdrs?sslmode=require"
        );
        // A '@' inside the password does not confuse the split.
        assert_eq!(
            redact_database_url("postgres://user:p@ss@host/rdrs"),
            "postgres://user:***@host/rdrs"
        );
        // No credentials, no change.
        assert_eq!(
            redact_database_url("postgres://user@host/rdrs"),
            "postgres://user@host/rdrs"
        );
        // SQLite paths and URLs pass through untouched.
        assert_eq!(redact_database_url("rdrs.sqlite3"), "rdrs.sqlite3");
        assert_eq!(
            redact_database_url("sqlite:///data/rdrs.sqlite3"),
            "sqlite:///data/rdrs.sqlite3"
        );
    }

    #[test]
    fn test_validate_accepts_both_backends() {
        // Both a PostgreSQL URL and a SQLite path pass validation now that the
        // sqlx data layer supports both backends (Phase C).
        let mut config = test_config();
        config.database_url = "postgres://user@localhost/rdrs".to_string();
        assert!(config.validate().is_ok());

        config.database_url = "rdrs.sqlite3".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn setup_is_open_only_on_an_empty_instance() {
        // The one anonymous account-creating path, and the only reason it is
        // acceptable: with zero accounts there is no username to enumerate.
        let config = test_config();
        assert!(config.can_setup(0));
        assert!(!config.can_setup(1));

        // Not a switch an operator can flip back on — multi-user governs the
        // admin's create button, not this.
        let multi = Config {
            multi_user_enabled: true,
            ..config
        };
        assert!(!multi.can_setup(1));
    }

    #[test]
    fn admin_account_creation_follows_multi_user() {
        // Single-user: the instance gets exactly the one account.
        let config = test_config();
        assert!(config.can_create_account(0));
        assert!(!config.can_create_account(1));

        // Multi-user: an admin may keep adding.
        let multi = Config {
            multi_user_enabled: true,
            ..config
        };
        assert!(multi.can_create_account(0));
        assert!(multi.can_create_account(5));
    }

    #[test]
    fn a_retired_variable_refuses_startup() {
        // Silently ignoring RDRS_SIGNUP_ENABLED=true would leave an operator
        // believing a public registration form exists when the endpoint is
        // gone. See RETIRED_VARS.
        let err = Config::from_map(|k| (k == "RDRS_SIGNUP_ENABLED").then(|| "true".to_string()))
            .expect_err("a retired variable must refuse startup");
        assert!(err.contains("RDRS_SIGNUP_ENABLED"), "{err}");
        assert!(
            err.contains("/admin"),
            "the message must say what replaced it: {err}"
        );

        // Blank is not "set": an empty value configured nothing before either.
        assert!(Config::from_map(|k| (k == "RDRS_SIGNUP_ENABLED").then(String::new)).is_ok());
    }

    #[test]
    fn the_pre_prefix_signup_flag_is_refused_once() {
        // An operator upgrading from the pre-prefix era must not be told to
        // rename SIGNUP_ENABLED to a variable that the next boot then rejects.
        // The single refusal names the feature that went away.
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
    fn test_client_ip_untrusted_peer_ignores_xff() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "203.0.113.1".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "8.8.8.8".parse().unwrap());

        assert_eq!(cfg.client_ip(Some(peer), &headers), peer);
    }

    /// KEY test: an append-mode proxy appends the real client to the RIGHT of
    /// whatever the client itself sent, so a pre-populated spoof (`8.8.8.8`)
    /// must not be believed — the right-most non-trusted entry is the client.
    #[test]
    fn test_client_ip_appendmode_spoof_uses_rightmost_untrusted() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "8.8.8.8, 203.0.113.9".parse().unwrap());

        let expected: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(cfg.client_ip(Some(peer), &headers), expected);
    }

    #[test]
    fn test_client_ip_multi_trusted_hop_skips_trusted_entries() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 10.0.0.5".parse().unwrap());

        let expected: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(cfg.client_ip(Some(peer), &headers), expected);
    }

    #[test]
    fn test_client_ip_all_trusted_xff_falls_back_to_x_real_ip() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.5, 10.0.0.6".parse().unwrap());
        headers.insert("x-real-ip", "198.51.100.7".parse().unwrap());

        let expected: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(cfg.client_ip(Some(peer), &headers), expected);
    }

    #[test]
    fn test_client_ip_trusted_peer_no_forwarded_headers_returns_peer() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "10.1.2.3".parse().unwrap();
        let headers = axum::http::HeaderMap::new();

        assert_eq!(cfg.client_ip(Some(peer), &headers), peer);
    }

    #[test]
    fn test_client_ip_no_peer_returns_localhost() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let headers = axum::http::HeaderMap::new();

        assert_eq!(
            cfg.client_ip(None, &headers),
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn test_client_ip_unparseable_rightmost_hop_does_not_fall_through_to_spoof() {
        let cfg = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        let peer: IpAddr = "10.0.0.1".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        // Attacker put a spoof on the left; the right-most (proxy-appended) token
        // is malformed. We must NOT fall through to the spoofed 8.8.8.8; we bail to
        // the trusted peer instead.
        headers.insert("x-forwarded-for", "8.8.8.8, not-an-ip".parse().unwrap());

        assert_eq!(cfg.client_ip(Some(peer), &headers), peer);
    }

    #[test]
    fn test_validate_header_requires_trusted_networks() {
        // Header set, no trusted networks → error.
        let bad = Config {
            auth_proxy_header: "Remote-User".to_string(),
            trusted_proxy_networks: Vec::new(),
            ..test_config()
        };
        assert!(bad.validate().is_err());

        // Header set with trusted networks → ok.
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
        // Default localhost origin → warn.
        let config = test_config();
        assert!(config.webauthn_rp_warning().is_some());

        // Real domain, no RDRS_PUBLIC_BASE_URL → fine.
        let deployed = Config {
            webauthn_rp_id: "rdrs.example.com".to_string(),
            webauthn_rp_origin: "https://rdrs.example.com".to_string(),
            ..test_config()
        };
        assert!(deployed.webauthn_rp_warning().is_none());

        // Real domain matching RDRS_PUBLIC_BASE_URL (trailing slash ignored) → fine.
        let matched = Config {
            public_base_url: Some("https://rdrs.example.com/".to_string()),
            ..deployed.clone()
        };
        assert!(matched.webauthn_rp_warning().is_none());

        // Real domain disagreeing with RDRS_PUBLIC_BASE_URL → warn.
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

        // Blank counts as unset, same as every other setting.
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

        // 0 is a valid, meaningful override: it disables the limiter.
        let config = from_vars(&[("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "0")]);
        assert_eq!(config.login_rate_limit_attempts, 0);
    }

    #[test]
    fn test_login_rate_limit_non_numeric_value_is_a_hard_error() {
        // A typo must not silently fall back to the default and leave the
        // protection looking configured while actually running unconfigured.
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
        // A zero-second window elapses instantly, so the limiter never throttles
        // while `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` still reads as configured.
        let err =
            Config::from_map(|k| (k == "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS").then(|| "0".into()))
                .expect_err("a zero RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS must fail startup");
        assert!(err.contains("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS"), "{err}");
        assert!(err.contains('0'), "{err}");
    }

    #[test]
    fn test_rate_limit_proxy_warning() {
        // Limiter enabled, no trusted proxies configured → warn: every
        // visitor behind a reverse proxy would collapse into one bucket.
        let config = test_config();
        assert!(config.rate_limit_proxy_warning().is_some());

        // Limiter enabled with a trusted-proxy list → fine.
        let with_proxies = Config {
            trusted_proxy_networks: parse_trusted_networks("10.0.0.0/8").unwrap(),
            ..test_config()
        };
        assert!(with_proxies.rate_limit_proxy_warning().is_none());

        // Limiter disabled (0 attempts) → no warning regardless of proxy config,
        // since there is no shared bucket to worry about.
        let disabled = Config {
            login_rate_limit_attempts: 0,
            trusted_proxy_networks: Vec::new(),
            ..test_config()
        };
        assert!(disabled.rate_limit_proxy_warning().is_none());
    }
}
