# Forward-Auth (Trusted-Header) Login Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users sign in to rdrs via an upstream forward-auth proxy (Authelia, authentik, oauth2-proxy, …) that injects a trusted identity header, mapping it to existing rdrs accounts by username with zero schema change.

**Architecture:** A tower middleware on the SSR page routes reads a configurable identity header — but only when the request's TCP peer IP is inside a trusted CIDR list — looks up the account by username (optionally JIT-creating it), establishes a normal session cookie, and redirects. Local password/passkey/GReader auth are untouched; an optional `DISABLE_LOCAL_AUTH` switch hides the browser password form.

**Tech Stack:** Rust, axum 0.8, axum-extra (cookies), rusqlite, `ipnet` (new), axum-test for integration tests, nextest.

## Global Constraints

- Run tests with `cargo nextest run` (never `cargo test`); set `RDRS_FAST_HASH=1` for local runs.
- Run `cargo fmt` before every commit; `cargo clippy -- -D warnings` must pass.
- All commits GPG-signed (default git config); end messages with the `Co-Authored-By` trailer.
- Stage files explicitly by name — never `git add -A`/`git add .`.
- New dependency `ipnet` MUST be pinned to a version published ≥7 days ago (verify with `cargo info ipnet`; expected `2.12`, released >1 year ago).
- No database schema change / migration. Account mapping is by **username** only.
- Match existing code style: `*Params`-free small functions, inline env-bool parsing as in `config.rs`.

---

### Task 1: Config fields, CIDR parsing, and startup validation

Adds the six env vars, the `ipnet` dependency, CIDR parsing, trust/role helper methods, and a `validate()` invoked at startup. Adding fields to `Config` breaks every struct literal, so all three are updated in the same commit.

**Files:**
- Modify: `Cargo.toml` (add `ipnet`)
- Modify: `src/config.rs` (fields, parsing, methods, validation, tests)
- Modify: `src/main.rs:22` (call `validate()` after `from_env`)
- Modify: `tests/common/mod.rs` (`default_test_config` literal)
- Modify: `src/auth/webauthn.rs:25` (`test_config` literal)

**Interfaces:**
- Produces:
  - `Config` fields: `auth_proxy_header: String`, `trusted_proxy_networks: Vec<ipnet::IpNet>`, `auth_proxy_user_creation: bool`, `disable_local_auth: bool`, `auth_proxy_groups_header: String`, `auth_proxy_admin_group: String`
  - `Config::auth_proxy_enabled(&self) -> bool`
  - `Config::group_mapping_enabled(&self) -> bool`
  - `Config::is_trusted_peer(&self, ip: std::net::IpAddr) -> bool`
  - `Config::validate(&self) -> Result<(), String>`
  - `config::parse_trusted_networks(raw: &str) -> Result<Vec<ipnet::IpNet>, String>` (pub)

- [ ] **Step 1: Add the dependency**

Run: `cargo info ipnet` to confirm the latest version is ≥7 days old, then add it:

```bash
cargo add ipnet@2.12
```

Expected: `Cargo.toml` gains `ipnet = "2.12"`.

- [ ] **Step 2: Write failing config tests**

Add to the `tests` module in `src/config.rs`:

```rust
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs config::`
Expected: FAIL — `parse_trusted_networks`, `is_trusted_peer`, `validate`, `group_mapping_enabled`, and the new fields don't exist yet.

- [ ] **Step 4: Add fields, parsing, methods, and validation**

In `src/config.rs`, add imports at the top:

```rust
use std::net::IpAddr;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
```

Add the six fields to `struct Config` (after `public_base_url`):

```rust
    pub auth_proxy_header: String,
    pub trusted_proxy_networks: Vec<IpNet>,
    pub auth_proxy_user_creation: bool,
    pub disable_local_auth: bool,
    pub auth_proxy_groups_header: String,
    pub auth_proxy_admin_group: String,
```

Add this free function above `impl Config`:

```rust
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
                "invalid CIDR or IP in TRUSTED_PROXY_NETWORKS: '{}'",
                s
            ));
        }
    }
    Ok(nets)
}
```

In `from_env`, add the six fields to the returned `Self { ... }`:

```rust
            auth_proxy_header: env::var("AUTH_PROXY_HEADER").unwrap_or_default(),
            trusted_proxy_networks: parse_trusted_networks(
                &env::var("TRUSTED_PROXY_NETWORKS").unwrap_or_default(),
            )
            .unwrap_or_default(),
            auth_proxy_user_creation: env::var("AUTH_PROXY_USER_CREATION")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            disable_local_auth: env::var("DISABLE_LOCAL_AUTH")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false),
            auth_proxy_groups_header: env::var("AUTH_PROXY_GROUPS_HEADER").unwrap_or_default(),
            auth_proxy_admin_group: env::var("AUTH_PROXY_ADMIN_GROUP").unwrap_or_default(),
```

Add these methods inside `impl Config`:

```rust
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
        self.trusted_proxy_networks.iter().any(|net| net.contains(&ip))
    }

    /// Validate cross-field invariants at startup. Returns the first problem.
    pub fn validate(&self) -> Result<(), String> {
        // Surface CIDR parse errors with the offending entry.
        if let Ok(raw) = std::env::var("TRUSTED_PROXY_NETWORKS") {
            parse_trusted_networks(&raw)?;
        }
        if self.auth_proxy_enabled() && self.trusted_proxy_networks.is_empty() {
            return Err("AUTH_PROXY_HEADER is set but TRUSTED_PROXY_NETWORKS is empty. \
                 Refusing to trust an identity header without a trusted-source check."
                .to_string());
        }
        if self.disable_local_auth && !self.auth_proxy_enabled() {
            return Err("DISABLE_LOCAL_AUTH is set but AUTH_PROXY_HEADER is not configured. \
                 This would leave no way to log in via the browser."
                .to_string());
        }
        Ok(())
    }
```

Update the `test_config()` helper inside `src/config.rs` tests to include the new fields:

```rust
            auth_proxy_header: String::new(),
            trusted_proxy_networks: Vec::new(),
            auth_proxy_user_creation: false,
            disable_local_auth: false,
            auth_proxy_groups_header: String::new(),
            auth_proxy_admin_group: String::new(),
```

- [ ] **Step 5: Fix the other two Config literals**

Add the same six lines to the `Config { ... }` literal in `tests/common/mod.rs` (`default_test_config`) and in `src/auth/webauthn.rs` (`test_config`, ~line 36, before the closing `}`).

- [ ] **Step 6: Wire validation into startup**

In `src/main.rs`, immediately after `let config = Config::from_env();` (line 22):

```rust
    if let Err(msg) = config.validate() {
        eprintln!("Configuration error: {msg}");
        std::process::exit(1);
    }
```

- [ ] **Step 7: Run the full check**

Run: `cargo fmt && RDRS_FAST_HASH=1 cargo nextest run -p rdrs config:: && cargo clippy -- -D warnings`
Expected: config tests PASS, clippy clean, whole workspace still compiles (all three Config literals updated).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/main.rs tests/common/mod.rs src/auth/webauthn.rs
git commit -m "feat(config): add forward-auth settings, CIDR trust, and startup validation

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Forward-auth middleware

The core feature: a `from_fn_with_state` middleware that engages on browser page routes when a trusted peer sends the identity header, resolving/creating the account and establishing a session. Includes wiring `ConnectInfo` in `main.rs` and the layer in `lib.rs`.

**Files:**
- Create: `src/middleware/forward_auth.rs`
- Modify: `src/middleware/mod.rs` (declare module)
- Modify: `src/main.rs` (`into_make_service_with_connect_info::<SocketAddr>()`)
- Modify: `src/lib.rs` (add the layer to `core`)
- Create: `tests/forward_auth_test.rs`

**Interfaces:**
- Consumes: `Config::auth_proxy_enabled`, `group_mapping_enabled`, `is_trusted_peer` (Task 1); `user::find_by_username`, `user::create_user`, `user::update_role`, `user::count`, `models::category::create_category`, `session::create_session`, `SESSION_COOKIE_NAME`.
- Produces:
  - `middleware::forward_auth::forward_auth` (the axum middleware fn)
  - `middleware::forward_auth::parse_groups(raw: &str) -> Vec<String>`
  - `middleware::forward_auth::role_from_groups(groups: &[String], admin_group: &str) -> Role`

- [ ] **Step 1: Write failing unit tests for the pure helpers**

Create `src/middleware/forward_auth.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;

    #[test]
    fn test_parse_groups() {
        assert_eq!(
            parse_groups("admins, users ,, dev"),
            vec!["admins".to_string(), "users".to_string(), "dev".to_string()]
        );
        assert!(parse_groups("   ").is_empty());
    }

    #[test]
    fn test_role_from_groups() {
        let groups = vec!["users".to_string(), "admins".to_string()];
        assert_eq!(role_from_groups(&groups, "admins"), Role::Admin);
        assert_eq!(role_from_groups(&groups, "superadmins"), Role::User);
        assert_eq!(role_from_groups(&[], "admins"), Role::User);
    }
}
```

Declare the module in `src/middleware/mod.rs`:

```rust
pub mod forward_auth;
```

- [ ] **Step 2: Run unit tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs forward_auth::tests`
Expected: FAIL — `parse_groups`/`role_from_groups` not defined.

- [ ] **Step 3: Implement the middleware**

Put this above the `tests` module in `src/middleware/forward_auth.rs`:

```rust
use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use time::Duration;

use crate::error::AppError;
use crate::middleware::{FlashRedirect, SESSION_COOKIE_NAME};
use crate::models::user::{self, Role};
use crate::models::{category, session};
use crate::AppState;

/// Path prefixes that must never trigger forward-auth auto-login: machine
/// endpoints (GReader native clients, JSON/passkey APIs, SSE, static assets)
/// authenticate by their own means.
const SKIP_PREFIXES: &[&str] = &[
    "/api", "/reader", "/accounts", "/events", "/static", "/favicon", "/health",
];

/// Parse a comma-separated groups header into trimmed, non-empty names.
pub fn parse_groups(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Map groups to a role: `Admin` iff `admin_group` is present, else `User`.
pub fn role_from_groups(groups: &[String], admin_group: &str) -> Role {
    if groups.iter().any(|g| g == admin_group) {
        Role::Admin
    } else {
        Role::User
    }
}

pub async fn forward_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    let config = &state.config;

    // Feature off, or already carrying a session cookie → nothing to do.
    if !config.auth_proxy_enabled() || jar.get(SESSION_COOKIE_NAME).is_some() {
        return next.run(req).await;
    }

    // Only engage for browser page routes.
    if SKIP_PREFIXES
        .iter()
        .any(|p| req.uri().path().starts_with(p))
    {
        return next.run(req).await;
    }

    // Fail closed: without a known peer IP we cannot trust the header.
    let Some(peer_ip) = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return next.run(req).await;
    };
    if !config.is_trusted_peer(peer_ip) {
        return next.run(req).await;
    }

    // Read the identity header.
    let Some(username) = req
        .headers()
        .get(config.auth_proxy_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return next.run(req).await;
    };

    // Optional group → role mapping (recomputed on every login when enabled).
    let desired_role = if config.group_mapping_enabled() {
        let groups = req
            .headers()
            .get(config.auth_proxy_groups_header.as_str())
            .and_then(|v| v.to_str().ok())
            .map(parse_groups)
            .unwrap_or_default();
        Some(role_from_groups(&groups, &config.auth_proxy_admin_group))
    } else {
        None
    };

    let allow_creation = config.auth_proxy_user_creation;

    // Resolve (or JIT-create) the account and open a session. `None` means
    // "reject" (unknown user with creation off, or a disabled account).
    let outcome = state
        .db
        .user(move |conn| {
            let user = match user::find_by_username(conn, &username)? {
                Some(u) => {
                    if u.is_disabled() {
                        return Ok::<Option<String>, AppError>(None);
                    }
                    if let Some(role) = desired_role {
                        if u.role != role {
                            user::update_role(conn, u.id, role)?;
                        }
                    }
                    u
                }
                None => {
                    if !allow_creation {
                        return Ok(None);
                    }
                    let role = match desired_role {
                        Some(r) => r,
                        None if user::count(conn)? == 0 => Role::Admin,
                        None => Role::User,
                    };
                    // Sentinel hash never verifies, so local password login is
                    // impossible for forward-auth-provisioned accounts.
                    let created = user::create_user(conn, &username, "!", role)?;
                    category::create_category(conn, created.id, "Uncategorized")?;
                    created
                }
            };
            let new_session = session::create_session(conn, user.id)?;
            Ok(Some(new_session.session_token))
        })
        .await;

    let token = match outcome {
        Ok(Ok(Some(token))) => token,
        Ok(Ok(None)) => {
            return FlashRedirect::warning(
                "/login",
                "You are not authorized to access this instance.",
            )
            .into_response();
        }
        // DB/join error → fail closed: fall back to the normal (cookie) flow.
        _ => return next.run(req).await,
    };

    let cookie = Cookie::build((SESSION_COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(session::SESSION_ABSOLUTE_MAX_DAYS))
        .build();

    // Redirect to the same URL; the just-set cookie authenticates the retry.
    let location = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    (jar.add(cookie), Redirect::to(&location)).into_response()
}
```

- [ ] **Step 4: Run unit tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs forward_auth::tests`
Expected: PASS.

- [ ] **Step 5: Wire `ConnectInfo` in `main.rs`**

Add near the other `use` lines in `src/main.rs`:

```rust
use std::net::SocketAddr;
```

Change the serve call (line ~127) from `axum::serve(listener, app)` to:

```rust
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
```

- [ ] **Step 6: Add the layer in `lib.rs`**

In `src/lib.rs`, in `create_router`, append one more `.layer(...)` to the end of the `core` chain — immediately after the existing `.layer(TimeoutLayer::with_status_code(...))` and before the terminating `;`:

```rust
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::forward_auth::forward_auth,
        ));
```

(`state` is still owned here; it is moved into `.with_state(state)` afterwards. `AppState` derives `Clone`.)

- [ ] **Step 7: Write failing integration tests**

Create `tests/forward_auth_test.rs`:

```rust
mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, config::parse_trusted_networks, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;

fn open_shared_memory(name: &str) -> Connection {
    let uri = format!("file:{}?mode=memory&cache=shared", name);
    Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

const DB_NAME: &str = "test_forward_auth";

/// Build a server over a real loopback HTTP transport so the middleware sees a
/// genuine `ConnectInfo` peer (127.0.0.1). `trusted` controls whether loopback
/// is inside the trusted network.
fn create_server(mut mutate: impl FnMut(&mut Config)) -> TestServer {
    let write_conn = open_shared_memory(DB_NAME);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(DB_NAME);

    let mut config = default_test_config();
    config.auth_proxy_header = "Remote-User".to_string();
    config.auth_proxy_groups_header = "Remote-Groups".to_string();
    config.auth_proxy_admin_group = "admins".to_string();
    mutate(&mut config);

    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let (pool, _handle) = DbPool::new(write_conn, read_conn);
    let state = AppState {
        db: pool,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    let app = create_router(state);
    TestServer::builder()
        .http_transport()
        .save_cookies()
        .build(app)
}

fn seed_user(name: &str, role: rdrs::models::user::Role) {
    let conn = open_shared_memory(DB_NAME);
    rdrs::models::user::create_user(&conn, name, "$argon2id$invalid", role).unwrap();
}

#[tokio::test]
async fn test_trusted_existing_user_gets_session() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user("alice", rdrs::models::user::Role::User);

    let res = server.get("/").add_header("Remote-User", "alice").await;

    // Redirect carrying a freshly-minted session cookie.
    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());
}

#[tokio::test]
async fn test_untrusted_peer_ignores_header() {
    let server = create_server(|c| {
        // Loopback is NOT in this network → header must be ignored.
        c.trusted_proxy_networks = parse_trusted_networks("10.0.0.0/8").unwrap();
    });
    seed_user("alice", rdrs::models::user::Role::User);

    let res = server.get("/").add_header("Remote-User", "alice").await;

    assert!(res.maybe_cookie("session_token").is_none());
}

#[tokio::test]
async fn test_unknown_user_creation_disabled_rejected() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = false;
    });

    let res = server.get("/").add_header("Remote-User", "ghost").await;

    assert!(res.maybe_cookie("session_token").is_none());
    assert_eq!(res.header("location"), "/login");
}

#[tokio::test]
async fn test_unknown_user_jit_created_as_admin_via_groups() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
        c.auth_proxy_user_creation = true;
    });

    let res = server
        .get("/")
        .add_header("Remote-User", "bob")
        .add_header("Remote-Groups", "users,admins")
        .await;

    assert!(res.status_code().is_redirection());
    assert!(res.maybe_cookie("session_token").is_some());

    // Account was created with Admin role from the groups header.
    let conn = open_shared_memory(DB_NAME);
    let created = rdrs::models::user::find_by_username(&conn, "bob")
        .unwrap()
        .unwrap();
    assert_eq!(created.role, rdrs::models::user::Role::Admin);
}

#[tokio::test]
async fn test_disabled_user_rejected() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user("carol", rdrs::models::user::Role::User);
    let conn = open_shared_memory(DB_NAME);
    let u = rdrs::models::user::find_by_username(&conn, "carol")
        .unwrap()
        .unwrap();
    rdrs::models::user::disable_user(&conn, u.id).unwrap();

    let res = server.get("/").add_header("Remote-User", "carol").await;

    assert!(res.maybe_cookie("session_token").is_none());
}
```

Note: this test references `rdrs::models::user` and `rdrs::config::parse_trusted_networks` as public paths. Confirm `pub mod models;` / `pub mod config;` re-exports exist in `src/lib.rs`; they are already public (used by other test files). If `models` is not `pub`, add `pub use` as needed in a tiny follow-up — do not broaden visibility beyond what the tests need.

- [ ] **Step 8: Run integration tests to verify they fail, then pass**

Run: `cargo fmt && RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test`
Expected: after Steps 3/5/6 are in place, all five PASS. If `ConnectInfo` is absent under the test transport, the trusted tests will not set a cookie — that indicates `.http_transport()` is required (already used above); do not switch to mock transport.

- [ ] **Step 9: Full lint + workspace test**

Run: `cargo fmt && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run`
Expected: clean; no regressions.

- [ ] **Step 10: Commit**

```bash
git add src/middleware/forward_auth.rs src/middleware/mod.rs src/main.rs src/lib.rs tests/forward_auth_test.rs
git commit -m "feat(auth): forward-auth trusted-header login middleware

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Enforce `DISABLE_LOCAL_AUTH`

Blocks the browser password path when configured, while leaving GReader `ClientLogin` and passkey APIs untouched.

**Files:**
- Modify: `src/handlers/auth.rs` (`login` guard)
- Modify: `src/handlers/pages/mod.rs` (`LoginTemplate` field + `login_page`)
- Modify: `templates/login.html` (conditionally render the password form)
- Modify: `tests/auth_test.rs` (new tests)

**Interfaces:**
- Consumes: `Config::disable_local_auth` (Task 1).
- Produces: `LoginTemplate.local_auth_enabled: bool`.

- [ ] **Step 1: Write failing tests**

Add to `tests/auth_test.rs` (uses that file's existing `create_test_server`/`default_test_config`):

```rust
#[tokio::test]
async fn test_disable_local_auth_blocks_password_login() {
    let mut config = default_test_config();
    config.disable_local_auth = true;
    config.auth_proxy_header = "Remote-User".to_string();
    config.trusted_proxy_networks =
        rdrs::config::parse_trusted_networks("127.0.0.0/8").unwrap();
    let server = create_test_server(config);

    // Seed a normal password user.
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Password login is now forbidden.
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_disable_local_auth_blocks_password_login`
Expected: FAIL — login still returns 200/cookie.

- [ ] **Step 3: Guard the login handler**

In `src/handlers/auth.rs`, at the very start of `pub async fn login(...)` (before the DB closure):

```rust
    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_disable_local_auth_blocks_password_login`
Expected: PASS.

- [ ] **Step 5: Hide the password form in the UI**

Add a field to `LoginTemplate` in `src/handlers/pages/mod.rs`:

```rust
    pub local_auth_enabled: bool,
```

Set it in `login_page` (inside the returned `LoginTemplate { ... }`):

```rust
            local_auth_enabled: !state.config.disable_local_auth,
```

In `templates/login.html`, wrap the password form and its "or use password" divider. Replace the divider line and the `<form id="login-form">…</form>` block so they render only when enabled:

```html
        <div id="passkey-section" style="display: none;">
            <button type="button" id="passkey-login-btn" class="btn-primary auth-passkey-btn">
                Login with Passkey
            </button>
            {% if local_auth_enabled %}
            <p class="muted auth-divider">or use password</p>
            {% endif %}
        </div>

        {% if local_auth_enabled %}
        <form id="login-form" data-testid="login-form">
            <div class="form-group">
                <label for="username">Username</label>
                <input type="text" id="username" name="username" required autocomplete="username" data-testid="username-input">
            </div>
            <div class="form-group">
                <label for="password">Password</label>
                <input type="password" id="password" name="password" required autocomplete="current-password" data-testid="password-input">
            </div>
            <button type="submit" class="btn-block" data-testid="login-submit">Sign In</button>
        </form>
        {% endif %}
```

The login `<script>` references `getElementById('login-form')`; guard the listener so it no-ops when the form is absent. In the script, change the listener attachment to:

```javascript
    const loginForm = document.getElementById('login-form');
    if (loginForm) loginForm.addEventListener('submit', async (e) => {
```

and close the `if` with the existing `});` (no other change to the body).

- [ ] **Step 6: Rebuild (templates are embedded) and verify**

Run: `cargo build && cargo fmt && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test`
Expected: builds (template compiles with the new `local_auth_enabled` field), clippy clean, auth tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/handlers/auth.rs src/handlers/pages/mod.rs templates/login.html tests/auth_test.rs
git commit -m "feat(auth): honor DISABLE_LOCAL_AUTH for browser password login

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

Document the new env vars, the trust model, and the migration story.

**Files:**
- Modify: `ARCHITECTURE.md` (authentication section + config list)
- Modify: `README.md` (environment variable table)

**Interfaces:** none.

- [ ] **Step 1: Update `ARCHITECTURE.md`**

In the Configuration section, add the six new variables. In the "Authentication Flow" area, add a "Forward-Auth (Trusted-Header) Login" subsection summarizing: configurable identity header; trusted-peer CIDR check on the TCP peer IP (not `X-Forwarded-For`); username-based mapping (no schema change); optional JIT creation; optional group→role sync; `DISABLE_LOCAL_AUTH` affects only the browser password form, not GReader/passkey. Include the operator warning that the proxy must authoritatively overwrite/strip the inbound identity/groups headers, and must bypass forward-auth for `/accounts/ClientLogin` and `/reader/api/...` so native clients keep working.

- [ ] **Step 2: Update `README.md`**

Add a row per variable to the environment table:

| Variable | Default | Description |
|---|---|---|
| `AUTH_PROXY_HEADER` | (unset) | Header carrying the username from a forward-auth proxy (e.g. `Remote-User`). Empty disables the feature. |
| `TRUSTED_PROXY_NETWORKS` | (unset) | Comma-separated CIDRs/IPs; the TCP peer must match for the header to be trusted. Required when `AUTH_PROXY_HEADER` is set. |
| `AUTH_PROXY_USER_CREATION` | `false` | JIT-create unknown users instead of rejecting. |
| `AUTH_PROXY_GROUPS_HEADER` | (unset) | Header with comma-separated groups (e.g. `Remote-Groups`). |
| `AUTH_PROXY_ADMIN_GROUP` | (unset) | Membership grants the admin role (synced on every forward-auth login). |
| `DISABLE_LOCAL_AUTH` | `false` | Hide the browser password form and reject `POST /api/session`. Does not affect GReader API or passkeys. Requires `AUTH_PROXY_HEADER`. |

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md README.md
git commit -m "docs: document forward-auth login configuration and migration

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Configurable header + multi-provider → Task 1/2 (`AUTH_PROXY_HEADER`, header read by name). ✓
- Username mapping, no schema change → Task 2 (`find_by_username`, sentinel hash, no migration). ✓
- Unknown identity default-reject + opt-in JIT → Task 2 (`auth_proxy_user_creation`). ✓
- Trusted CIDR on TCP peer IP → Task 1 (`is_trusted_peer`) + Task 2 (`ConnectInfo`, fail-closed). ✓
- Local auth coexistence + `DISABLE_LOCAL_AUTH` (browser only, GReader/passkey intact) → Task 3 (guard scoped to `POST /api/session`; `SKIP_PREFIXES` keeps `/accounts`, `/reader`, `/api` out of forward-auth). ✓
- Group→role sync on every login, Authelia authoritative → Task 2 (`desired_role` recomputed, `update_role` on diff). ✓
- Startup validation → Task 1 (`validate()` + main.rs). ✓
- Docs, no screenshot regen → Task 4 (default `/login` unchanged). ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✓

**Type consistency:** `forward_auth`, `parse_groups`, `role_from_groups`, `parse_trusted_networks`, `is_trusted_peer`, `auth_proxy_enabled`, `group_mapping_enabled`, `validate`, `local_auth_enabled` used consistently across tasks. Sentinel hash `"!"` in middleware; tests seed disabled/role users via `models::user`. ✓

## Notes / risks

- `axum-test` `.http_transport()` is required for the middleware to observe a real `ConnectInfo` peer (loopback). The middleware fails closed when `ConnectInfo` is absent, so a wrong transport shows up as "no session cookie" in the trusted tests, not a panic.
- If `models`/`config` are not already `pub` in `src/lib.rs`, expose exactly the items the tests use (`pub mod config;` is needed for `parse_trusted_networks`; other test files already import `rdrs::models::...`).
