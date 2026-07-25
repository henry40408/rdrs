use base64::{Engine, engine::general_purpose::STANDARD};
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rand::Rng;
use std::env;
use std::net::{IpAddr, SocketAddr};

/// Default user agent for HTTP requests (transparent and responsible crawling)
pub const DEFAULT_USER_AGENT: &str = concat!(
    "RDRS/",
    env!("GIT_VERSION"),
    " (RSS Reader; +https://github.com/henry40408/rdrs)"
);

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub server_bind: SocketAddr,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
    /// Process-wide root key backing every signature rdrs produces (session
    /// cookies, image-proxy URLs, the `GReader` post token). See [`crate::secret`]
    /// for the domain-separated derivation; each use derives its own tag.
    pub secret: Vec<u8>,
    /// Whether [`Config::secret`] was randomly generated because `RDRS_SECRET`
    /// was unset or too short, rather than configured. Drives the startup
    /// warning — a generated key means every browser session ends and every
    /// cached image-proxy URL breaks on each restart.
    pub secret_generated: bool,
    pub user_agent: String,
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
    /// [`crate::middleware::rate_limit::Bucket`]: password login — including
    /// `GReader` `ClientLogin` and passkey completion — registration, and
    /// passkey ceremony start each keep their own budget). Separate budgets
    /// stop a registration refused by configuration from also exhausting the
    /// login budget for the same IP. `0` disables the limiter entirely — an
    /// escape hatch for deployments that already throttle upstream (e.g.
    /// behind an authenticating reverse proxy).
    pub login_rate_limit_attempts: u32,
    /// Fixed-window length, in seconds, for [`Config::login_rate_limit_attempts`].
    pub login_rate_limit_window_secs: u64,
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
/// An explicit `RDRS_COOKIE_SECURE` wins; otherwise the answer is derived from
/// `RDRS_PUBLIC_BASE_URL`'s scheme. Deriving beats a standalone knob because a real
/// deployment already has to set `RDRS_PUBLIC_BASE_URL` correctly (it drives the
/// absolute image-proxy URLs), so an HTTPS install gets `Secure` without a
/// second setting to forget — while a plain `http://` dev run keeps working.
/// That last part matters: a browser silently drops a `Secure` cookie sent over
/// HTTP, so defaulting it on would lock a developer out with no visible error.
///
/// The override exists for TLS-terminating setups that cannot advertise their
/// public URL here, and for forcing the flag off while debugging.
///
/// Unlike the other boolean env vars, an unrecognized value is a hard error
/// rather than a silent "off". Those default to `false`, so a typo there is a
/// no-op; here the derived default can be `true`, so treating `RDRS_COOKIE_SECURE=yes`
/// as "off" would quietly strip `Secure` from a correctly-configured HTTPS
/// deployment — exactly the failure this setting exists to prevent.
pub fn parse_cookie_secure(
    raw: Option<&str>,
    public_base_url: Option<&str>,
) -> Result<bool, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) if v.eq_ignore_ascii_case("true") || v == "1" => Ok(true),
        Some(v) if v.eq_ignore_ascii_case("false") || v == "0" => Ok(false),
        Some(v) => Err(format!(
            "invalid RDRS_COOKIE_SECURE '{v}': expected one of true, false, 1, 0"
        )),
        None => Ok(public_base_url
            .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("https://"))),
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
/// A `PostgreSQL` URL carries credentials inline
/// (`postgres://user:secret@host/db`); the settings page renders the running
/// instance's `DATABASE_URL`, so the password must never survive into the
/// response. Only the password component is replaced — the scheme, user, host
/// and database name stay legible, which is what the page is actually for.
/// `SQLite` paths have no userinfo and pass through untouched.
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

/// Resolve the `RDRS_SERVER_BIND` value into a [`SocketAddr`]. An unset or empty
/// value yields the default `127.0.0.1:8080` (loopback only, so a bare-metal
/// run is not exposed on all interfaces without opting in); any non-empty
/// value must be a valid `host:port` socket address. The container image sets
/// `RDRS_SERVER_BIND=0.0.0.0:8080` so a reverse proxy in a separate container can
/// reach it.
pub fn parse_server_bind(raw: Option<&str>) -> Result<SocketAddr, String> {
    match raw {
        Some(v) if !v.is_empty() => v
            .parse::<SocketAddr>()
            .map_err(|e| format!("invalid RDRS_SERVER_BIND '{v}': {e}")),
        _ => Ok(SocketAddr::from(([127, 0, 0, 1], 8080))),
    }
}

/// Resolve `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` into the attempt budget for
/// [`Config::login_rate_limit_attempts`]. An unset or blank value yields the
/// default of [`crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS`]; a
/// non-empty value must parse as a `u32` (`0` is valid and disables the
/// limiter — see the field doc). Unparseable input is a hard startup error
/// rather than a silent fallback: silently keeping the default here would
/// mean a typo (`RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=5o`) leaves the protection
/// looking configured while actually running on the default, exactly the
/// failure mode `parse_cookie_secure` refuses for the same reason.
fn parse_login_rate_limit_attempts(raw: Option<&str>) -> Result<u32, String> {
    match raw {
        Some(v) => v
            .parse::<u32>()
            .map_err(|e| format!("invalid RDRS_LOGIN_RATE_LIMIT_ATTEMPTS '{v}': {e}")),
        None => Ok(crate::middleware::rate_limit::LOGIN_MAX_ATTEMPTS),
    }
}

/// Resolve `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` into the window length for
/// [`Config::login_rate_limit_window_secs`]. Same rules as
/// [`parse_login_rate_limit_attempts`]: unset/blank falls back to
/// [`crate::middleware::rate_limit::LOGIN_WINDOW_SECS`], anything else must
/// parse as a `u64` or startup fails. A parsed `0` is also rejected: the
/// window elapses the instant it is recorded, so every attempt starts a
/// fresh window and the limiter silently never throttles anything while
/// still looking configured. `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=0` is the
/// correct way to disable the limiter on purpose.
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

/// Resolve the root signing key from a raw `RDRS_SECRET` value, returning the
/// key bytes and whether they were generated rather than configured. A base64
/// value is decoded; otherwise the raw bytes are used. Either way at least
/// [`crate::secret::MIN_SECRET_LEN`] bytes are required — a shorter value is
/// discarded in favour of a fresh random key rather than used as a guessable
/// one.
///
/// Unlike every other string setting this does *not* go through [`nonblank`]:
/// trimming would change the key bytes for an existing deployment whose value
/// happens to carry whitespace, rotating the key — which ends every browser
/// session and breaks every image-proxy URL already cached by a Google Reader
/// client.
fn load_secret(raw: Option<String>) -> (Vec<u8>, bool) {
    use crate::secret::MIN_SECRET_LEN;
    if let Some(secret_str) = raw {
        // Try to decode as base64 first
        if let Ok(decoded) = STANDARD.decode(&secret_str)
            && decoded.len() >= MIN_SECRET_LEN
        {
            return (decoded, false);
        }
        // Use raw bytes if at least MIN_SECRET_LEN characters
        if secret_str.len() >= MIN_SECRET_LEN {
            return (secret_str.into_bytes(), false);
        }
    }

    // Generate a random 32-byte secret
    let mut secret = vec![0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    (secret, true)
}

/// Read `key` through `get` and normalize it: surrounding whitespace is
/// trimmed, and a value that is empty afterwards counts as unset.
///
/// Every string-valued setting goes through here so `FOO=` or `FOO="  "` in a
/// compose file means "not configured" rather than "configured to the empty
/// string". The distinction is load-bearing for the settings that are disabled
/// by being empty — a whitespace-only `RDRS_AUTH_PROXY_HEADER` would otherwise
/// enable forward auth against a header name no proxy can send.
fn nonblank(get: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Whether a boolean env var is on. `true` (any case) and `1` are the only
/// accepted values; anything else — including a typo — reads as off, which is
/// safe because every setting using this defaults to off anyway.
/// `RDRS_COOKIE_SECURE` deliberately does *not* use this: its default can be `true`,
/// so it rejects unrecognized values instead. See [`parse_cookie_secure`].
fn flag(get: &impl Fn(&str) -> Option<String>, key: &str) -> bool {
    nonblank(get, key).is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// Every variable renamed by the `RDRS_` prefix migration, old name → new name.
///
/// `IMAGE_PROXY_SECRET` is the one entry that is not a straight prefixing: it
/// became `RDRS_SECRET` because the key no longer signs only image-proxy URLs.
///
/// `DATABASE_URL` is deliberately absent: it is a genuine cross-tool convention
/// (Twelve-Factor, Heroku/Railway/Render, sqlx, Diesel), and pingward keeps it
/// bare for the same reason. The rest of this list only *looks* generic —
/// `USER_AGENT` and `SERVER_BIND` in particular are rdrs's own names, which is
/// exactly what makes them collide in a shared compose file.
pub const RENAMED_VARS: &[(&str, &str)] = &[
    ("SERVER_BIND", "RDRS_SERVER_BIND"),
    ("SIGNUP_ENABLED", "RDRS_SIGNUP_ENABLED"),
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

/// Refuse to start when a pre-prefix variable name still carries a value.
///
/// Ignoring the old name would be the worst of the three options: an operator
/// who upgrades without editing their compose file would get a *working* server
/// — running on defaults, so a fresh empty database at `rdrs.sqlite3`, signup
/// off, and a regenerated secret — instead of their actual deployment. A
/// warning fares little better, since the same wrong server comes up and the
/// line scrolls past. Failing names every variable that has to move, once.
///
/// Only a value that survives [`nonblank`] counts: `FOO=` left behind in a
/// compose file configured nothing before the rename either, so it is not worth
/// blocking a boot over.
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

    /// Build the config from an arbitrary key→value lookup.
    ///
    /// `from_env` is the one-line adapter over the real environment; every test
    /// passes a closure instead. That keeps config tests pure — mutating the
    /// process environment is `unsafe` under edition 2024 and only survives
    /// because nextest forks per test, which is a property of the runner rather
    /// than of the code being tested.
    pub fn from_map(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        reject_legacy_vars(&get)?;

        let (secret, secret_generated) = load_secret(get("RDRS_SECRET"));
        let server_bind = parse_server_bind(nonblank(&get, "RDRS_SERVER_BIND").as_deref())?;

        let trusted_proxy_networks = parse_trusted_networks(
            &nonblank(&get, "RDRS_TRUSTED_PROXY_NETWORKS").unwrap_or_default(),
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

        Ok(Self {
            // Not prefixed — see `RENAMED_VARS`.
            database_url: nonblank(&get, "DATABASE_URL")
                .unwrap_or_else(|| "rdrs.sqlite3".to_string()),
            server_bind,
            signup_enabled: flag(&get, "RDRS_SIGNUP_ENABLED"),
            multi_user_enabled: flag(&get, "RDRS_MULTI_USER_ENABLED"),
            secret,
            secret_generated,
            user_agent: nonblank(&get, "RDRS_USER_AGENT")
                .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string()),
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
    /// they are attacker-controlled and ignored, and the peer address is used.
    ///
    /// `X-Forwarded-For` is read RIGHT-to-left: each hop *appends* the address
    /// it saw (`client, proxy1, proxy2, ...`), which is how common append-mode
    /// reverse proxies populate it (nginx's `$proxy_add_x_forwarded_for`,
    /// Traefik, Caddy). The real client is therefore the right-most entry that
    /// is not itself one of our trusted proxies; entries to its left are
    /// client-supplied and must not be believed. Taking the left-most entry
    /// instead would let any client forge its own logged IP.
    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &axum::http::HeaderMap) -> IpAddr {
        let Some(peer) = peer else {
            return IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
        };
        // A direct (untrusted) client connection: its TCP source is the client;
        // forwarded headers from it are attacker-controlled and ignored.
        if !self.is_trusted_peer(peer) {
            return peer;
        }
        // `X-Forwarded-For` is "client, proxy1, proxy2, ..." where each hop APPENDS
        // the address it saw. The real client is the RIGHT-MOST entry that is not
        // one of our own trusted proxies; entries to its left are client-supplied
        // and must not be believed. (Leftmost is forgeable under append-mode
        // proxies like nginx's `$proxy_add_x_forwarded_for`.)
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

    /// Whether a new account may be registered given the current user count.
    ///
    /// The very first account is always allowed so a fresh install (including a
    /// source build that never set `RDRS_SIGNUP_ENABLED`) can always create its
    /// initial admin. Every subsequent account requires both `RDRS_SIGNUP_ENABLED`
    /// and `RDRS_MULTI_USER_ENABLED`.
    pub fn can_register(&self, user_count: i64) -> bool {
        user_count == 0 || (self.signup_enabled && self.multi_user_enabled)
    }

    /// A startup warning about `WebAuthn` relying-party config that would silently
    /// break passkeys in a real deployment, or `None` when the config looks
    /// deployable. Returns a message when the RP origin still points at
    /// `localhost` (the default), or when it disagrees with `RDRS_PUBLIC_BASE_URL`.
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

    /// A startup warning about the credential rate limiter running without a
    /// trusted-proxy list, or `None` when the config looks deployable. Fires
    /// when the limiter is enabled but `RDRS_TRUSTED_PROXY_NETWORKS` is empty:
    /// [`Config::client_ip`] then falls back to the TCP peer for every
    /// request, so behind a reverse proxy every visitor collapses into the
    /// proxy's one address and a single abuser can exhaust the shared bucket
    /// and lock out every real user.
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
            signup_enabled: true,
            multi_user_enabled: false,
            secret: vec![0u8; 32],
            secret_generated: false,
            user_agent: DEFAULT_USER_AGENT.to_string(),
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
        assert!(!config.signup_enabled);
        assert!(!config.multi_user_enabled);
        assert!(!config.cookie_secure);
        assert!(config.public_base_url.is_none());
        assert!(config.auth_proxy_logout_url.is_none());
        assert!(!config.auth_proxy_enabled());
        // Nothing configured means no persistent secret, which the startup
        // warning in `main` reports.
        assert!(config.secret_generated);
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
                from_vars(&[("RDRS_SIGNUP_ENABLED", raw)]).signup_enabled,
                "{raw} should enable"
            );
        }
        // Anything else is off. These all default to off, so a typo is a no-op
        // rather than a silent downgrade (unlike RDRS_COOKIE_SECURE, which rejects).
        for raw in ["false", "0", "yes", "on", "", "  "] {
            assert!(
                !from_vars(&[("RDRS_SIGNUP_ENABLED", raw)]).signup_enabled,
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
    fn test_can_register() {
        // Single-user (signup on, multi-user off): only the first account.
        let config = test_config();
        assert!(config.can_register(0));
        assert!(!config.can_register(1));

        // Multi-user: every account is allowed.
        let config_multi = Config {
            multi_user_enabled: true,
            ..config.clone()
        };
        assert!(config_multi.can_register(0));
        assert!(config_multi.can_register(5));

        // Signup disabled: the first account still works (fresh-install bootstrap),
        // but no further accounts may register.
        let config_disabled = Config {
            signup_enabled: false,
            ..config
        };
        assert!(config_disabled.can_register(0));
        assert!(!config_disabled.can_register(1));
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

    /// KEY test: an append-mode proxy (nginx `$proxy_add_x_forwarded_for`,
    /// Traefik, Caddy) appends the real client to the RIGHT of whatever the
    /// client itself sent. A client that pre-populates the header with a
    /// spoofed value (`8.8.8.8`) must not have that value believed — the
    /// right-most, non-trusted entry (`203.0.113.9`, appended by our proxy)
    /// is the real client.
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
        // A zero-second window elapses instantly, so every attempt starts a
        // fresh window and the limiter never actually throttles anything —
        // while `RDRS_LOGIN_RATE_LIMIT_ATTEMPTS` still reads as configured.
        // That must fail startup rather than boot a silently-disabled limiter.
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
