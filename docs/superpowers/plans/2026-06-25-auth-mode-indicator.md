# Auth-Mode Indicator + App Config Grouping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show an "SSO" pill in the sidebar when the current request is served through forward-auth, and surface the forward-auth config on the App page in a regrouped, reordered Configuration table.

**Architecture:** Forward-auth state is computed dynamically per request (no persistence) via a shared `forward_auth_identity` helper, exposed on the `AuthUser`/`PageAuthUser` extractors as `via_forward_auth`, carried through `SidebarResponse` to the SSR bootstrap and `/api/sidebar`, and rendered as a pill by the sidebar web component. Separately, the `/settings` table is reorganized into labeled groups with columns reordered to Variable · Current · Default · Description, including the forward-auth vars.

**Tech Stack:** Rust, axum 0.8, axum-extra, rusqlite, axum-test, Askama templates, vanilla ES module web component, CSS.

## Global Constraints

- Run tests with `cargo nextest run` (never `cargo test`); prefix local runs with `RDRS_FAST_HASH=1`.
- `cargo fmt` before every commit; `cargo clippy -- -D warnings` must pass.
- Commits GPG-signed (default git config); end messages with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Stage files explicitly by name — never `git add -A` / `git add .`.
- **No schema change** — forward-auth state is computed per request. The forward-auth `Config` fields already exist from prior work; this plan only reads them.
- Static JS / templates / CSS are embedded at compile time; run `cargo build` after editing `static/**` or `templates/**` before any manual/e2e/screenshot check.
- `rdrs::middleware::forward_auth` and `rdrs::config` are public; integration tests may call `forward_auth_identity` and `parse_trusted_networks` directly and build configs via `common::default_test_config()`.
- The pill label is the static string `SSO`. Local (non-forward-auth) requests render no pill.

---

### Task 1: `forward_auth_identity` shared helper + middleware reuse

A pure helper that decides whether a request carries a trusted forward-auth identity. The existing middleware is refactored to use it (DRY), guarded by the existing forward-auth integration tests.

**Files:**
- Modify: `src/middleware/forward_auth.rs` (add helper; refactor middleware to call it)
- Test: `tests/forward_auth_test.rs`

**Interfaces:**
- Produces: `pub fn forward_auth_identity(config: &crate::config::Config, peer_ip: Option<std::net::IpAddr>, headers: &axum::http::HeaderMap) -> Option<String>`

- [ ] **Step 1: Write the failing helper tests**

Add to `tests/forward_auth_test.rs` (it already has `mod common; use common::default_test_config;`):

```rust
use axum::http::{HeaderMap, HeaderName};
use rdrs::config::parse_trusted_networks;
use rdrs::middleware::forward_auth::forward_auth_identity;

fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    h
}

#[test]
fn test_forward_auth_identity() {
    let mut cfg = default_test_config();
    cfg.auth_proxy_header = "Remote-User".to_string();
    cfg.trusted_proxy_networks = parse_trusted_networks("10.0.0.0/8").unwrap();

    let trusted: std::net::IpAddr = "10.1.2.3".parse().unwrap();
    let untrusted: std::net::IpAddr = "192.168.0.1".parse().unwrap();
    let with_header = header_map(&[("Remote-User", "alice")]);

    // trusted peer + header present → identity
    assert_eq!(
        forward_auth_identity(&cfg, Some(trusted), &with_header),
        Some("alice".to_string())
    );
    // untrusted peer → None
    assert_eq!(forward_auth_identity(&cfg, Some(untrusted), &with_header), None);
    // no peer IP → None
    assert_eq!(forward_auth_identity(&cfg, None, &with_header), None);
    // header missing → None
    assert_eq!(forward_auth_identity(&cfg, Some(trusted), &HeaderMap::new()), None);
    // header empty → None
    assert_eq!(
        forward_auth_identity(&cfg, Some(trusted), &header_map(&[("Remote-User", "  ")])),
        None
    );

    // feature off (empty header name) → None
    let mut off = cfg.clone();
    off.auth_proxy_header = String::new();
    assert_eq!(forward_auth_identity(&off, Some(trusted), &with_header), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test test_forward_auth_identity`
Expected: FAIL — `forward_auth_identity` does not exist.

- [ ] **Step 3: Add the helper**

In `src/middleware/forward_auth.rs`, add imports near the top:

```rust
use std::net::IpAddr;

use axum::http::HeaderMap;

use crate::config::Config;
```

Add the helper (above the `forward_auth` function):

```rust
/// The identity supplied by a trusted forward-auth proxy on this request, if
/// any. Returns `None` when the feature is off, the peer IP is missing or not
/// in `TRUSTED_PROXY_NETWORKS`, or the identity header is absent/empty. Shared
/// by the middleware and the `AuthUser`/`PageAuthUser` extractors so the
/// trust logic lives in one place.
pub fn forward_auth_identity(
    config: &Config,
    peer_ip: Option<IpAddr>,
    headers: &HeaderMap,
) -> Option<String> {
    if !config.auth_proxy_enabled() {
        return None;
    }
    let ip = peer_ip?;
    if !config.is_trusted_peer(ip) {
        return None;
    }
    headers
        .get(config.auth_proxy_header.as_str())
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
```

- [ ] **Step 4: Refactor the middleware to use the helper**

In `forward_auth`, replace the inline peer-IP trust check and identity-header read (the block that starts with the `// Fail closed: without a known peer IP` comment and ends with the `let Some(username) = req.headers()...else { return next.run(req).await; };` block) with:

```rust
    // Trusted-peer + identity-header check (shared with the auth extractors).
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let Some(username) = forward_auth_identity(config, peer_ip, req.headers()) else {
        return next.run(req).await;
    };
```

Leave everything before it (the `auth_proxy_enabled` early return, the `SKIP_PREFIXES` check, the valid-session-cookie short-circuit) and after it (group mapping, session creation, redirect) unchanged.

- [ ] **Step 5: Run helper test + full forward-auth suite**

Run: `cargo fmt && RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test && cargo clippy -- -D warnings`
Expected: PASS — the new helper test plus all existing forward_auth integration tests (the refactor is behavior-preserving).

- [ ] **Step 6: Commit**

```bash
git add src/middleware/forward_auth.rs tests/forward_auth_test.rs
git commit -m "refactor(auth): extract forward_auth_identity helper for reuse

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `via_forward_auth` on the extractors and `SidebarResponse`

Compute the flag in the `AuthUser`/`PageAuthUser` extractors and thread it into the SSR sidebar bootstrap and `GET /api/sidebar`.

**Files:**
- Modify: `src/middleware/auth.rs` (`AuthUser`, `PageAuthUser` structs + `from_request_parts`)
- Modify: `src/handlers/user.rs` (`SidebarResponse` field; `build_sidebar_response` param; `get_sidebar`)
- Modify: `src/handlers/pages/mod.rs` (`build_app_layout` inline `SidebarResponse`)
- Test: `tests/forward_auth_test.rs`

**Interfaces:**
- Consumes: `forward_auth_identity` (Task 1).
- Produces: `AuthUser.via_forward_auth: bool`, `PageAuthUser.via_forward_auth: bool`, `SidebarResponse.via_forward_auth: bool`, `build_sidebar_response(state, user, session, via_forward_auth)`.

- [ ] **Step 1: Write the failing integration test**

Add to `tests/forward_auth_test.rs` (uses the file's `create_server`, `seed_user`, `.http_transport().save_cookies()` harness):

```rust
#[tokio::test]
async fn test_sidebar_reports_via_forward_auth_dynamically() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user("grace", rdrs::models::user::Role::User);

    // Establish a session via forward-auth (page route mints the cookie).
    server.get("/").add_header("Remote-User", "grace").await;

    // With the proxy header present → via_forward_auth true.
    let with = server.get("/api/sidebar").add_header("Remote-User", "grace").await;
    with.assert_status_ok();
    assert_eq!(with.json::<serde_json::Value>()["via_forward_auth"], true);

    // Same valid session, but no proxy header on this request → false (dynamic).
    let without = server.get("/api/sidebar").await;
    without.assert_status_ok();
    assert_eq!(without.json::<serde_json::Value>()["via_forward_auth"], false);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test test_sidebar_reports_via_forward_auth_dynamically`
Expected: FAIL — `via_forward_auth` is not a field in the JSON / does not compile.

- [ ] **Step 3: Add `via_forward_auth` to the extractors**

In `src/middleware/auth.rs`, add imports:

```rust
use std::net::SocketAddr;

use axum::extract::ConnectInfo;
```

Add the field to both structs:

```rust
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}
```

```rust
#[derive(Debug, Clone)]
pub struct PageAuthUser {
    pub user: User,
    pub session: Session,
    pub via_forward_auth: bool,
}
```

In `AuthUser::from_request_parts`, just before `Ok(AuthUser { user, session })`, compute and include the flag:

```rust
        let peer_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let via_forward_auth =
            crate::middleware::forward_auth::forward_auth_identity(&state.config, peer_ip, &parts.headers)
                .is_some();

        Ok(AuthUser {
            user,
            session,
            via_forward_auth,
        })
```

Do the same in `PageAuthUser::from_request_parts`, returning `Ok(PageAuthUser { user, session, via_forward_auth })`. (`AdminUser`/`PageAdminUser` build from these and only read `user`/`session`, so they need no change.)

- [ ] **Step 4: Thread the flag through `SidebarResponse`**

In `src/handlers/user.rs`, add the field to `SidebarResponse`:

```rust
    pub via_forward_auth: bool,
```

Change `build_sidebar_response` to accept and set it:

```rust
pub async fn build_sidebar_response(
    state: &AppState,
    user: &crate::models::User,
    session: &crate::models::session::Session,
    via_forward_auth: bool,
) -> AppResult<SidebarResponse> {
    // ... existing body unchanged ...
    Ok(SidebarResponse {
        username: user.username.clone(),
        is_admin,
        is_masquerading,
        categories: chrome.categories,
        total_unread: chrome.total_unread,
        total_summarized: chrome.total_summarized,
        via_forward_auth,
    })
}
```

Update `get_sidebar` to pass it:

```rust
    let payload =
        build_sidebar_response(&state, &auth_user.user, &auth_user.session, auth_user.via_forward_auth)
            .await?;
```

- [ ] **Step 5: Set the flag in the SSR bootstrap**

In `src/handlers/pages/mod.rs`, in `build_app_layout`, add to the inline `SidebarResponse { ... }` construction:

```rust
        via_forward_auth: auth_user.via_forward_auth,
```

- [ ] **Step 6: Run the test + full suite**

Run: `cargo fmt && RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run`
Expected: the new test PASSES; whole suite green (the new struct field compiles everywhere it's built).

- [ ] **Step 7: Commit**

```bash
git add src/middleware/auth.rs src/handlers/user.rs src/handlers/pages/mod.rs tests/forward_auth_test.rs
git commit -m "feat(auth): expose via_forward_auth on extractors and sidebar payload

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Sidebar SSO pill (frontend)

Render the pill next to the username when `via_forward_auth` is true, and treat the flag as a structural change for live updates.

**Files:**
- Modify: `static/js/components/rdrs-sidebar.js`
- Modify: `static/css/app.css`

**Interfaces:**
- Consumes: `data.via_forward_auth` (Task 2).

- [ ] **Step 1: Read the flag in `render()`**

In `static/js/components/rdrs-sidebar.js`, after the existing destructuring (around lines 154–158, after `const isMasq = ...`), add:

```javascript
        const viaForwardAuth = data ? !!data.via_forward_auth : false;
```

- [ ] **Step 2: Render the pill in the footer**

Replace the `.sidebar-footer` markup (currently the `<span class="sidebar-user">…</span>` + Sign Out link) with:

```javascript
    <div class="sidebar-footer">
        <span class="sidebar-id">
            <span class="sidebar-user">${escapeHtml(username)}</span>
            ${viaForwardAuth ? '<span class="sidebar-auth-pill" data-testid="auth-pill">SSO</span>' : ''}
        </span>
        <a href="#" data-testid="logout-btn" data-rdrs-logout>Sign Out</a>
    </div>`;
```

- [ ] **Step 3: Treat the flag as a structural change**

In `isStructuralChange(prev, next)`, add alongside the existing checks:

```javascript
    if (!!prev.via_forward_auth !== !!next.via_forward_auth) return true;
```

- [ ] **Step 4: Add CSS**

In `static/css/app.css`, near the existing `.sidebar-footer` / `.sidebar-user` rules, add:

```css
.sidebar-id {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    overflow: hidden;
}
.sidebar-auth-pill {
    flex: none;
    font-size: var(--font-xs);
    font-weight: 600;
    color: var(--color-accent);
    background: var(--color-accent-subtle);
    padding: 0.1em 0.5em;
    border-radius: var(--radius-lg);
}
```

- [ ] **Step 5: Rebuild and verify it compiles into the binary**

Run: `cargo build && cargo fmt && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run`
Expected: builds (embedded assets refresh), clippy clean, suite green. (The pill itself is verified by Task 2's data assertion plus a manual check; there is no JS unit-test harness — note this in the report.)

- [ ] **Step 6: Commit**

```bash
git add static/js/components/rdrs-sidebar.js static/css/app.css
git commit -m "feat(ui): show SSO pill in sidebar for forward-auth sessions

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: App page — grouped, reordered config table with forward-auth rows

Reorganize the `/settings` Configuration table into 5 labeled groups, reorder columns to Variable · Current · Default · Description, and add the 7 forward-auth rows.

**Files:**
- Modify: `src/handlers/pages/mod.rs` (`SettingsTemplate` fields + `settings_page` population)
- Modify: `templates/settings.html`
- Modify: `static/css/app.css`
- Test: `tests/pages_test.rs`

**Interfaces:**
- Consumes: `state.config` forward-auth fields (already exist).

- [ ] **Step 1: Write the failing integration test**

Add to `tests/pages_test.rs` (follow that file's existing harness for an authenticated page GET — register the first user, then GET the page with the saved cookie; mirror an existing `/settings` or authenticated-page test in the file). The server config sets forward-auth values so the Current column reflects them:

```rust
#[tokio::test]
async fn test_settings_page_groups_and_forward_auth() {
    let mut config = default_test_config();
    config.auth_proxy_header = "Remote-User".to_string();
    config.trusted_proxy_networks = rdrs::config::parse_trusted_networks("10.0.0.0/8").unwrap();
    config.auth_proxy_admin_group = "admins".to_string();
    let server = create_test_server(config);

    server
        .post("/api/register")
        .json(&serde_json::json!({ "username": "admin", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    server
        .post("/api/session")
        .json(&serde_json::json!({ "username": "admin", "password": "password123" }))
        .await
        .assert_status_ok();

    let res = server.get("/settings").await;
    res.assert_status_ok();
    let body = res.text();
    // group headers present
    assert!(body.contains("Authentication &mdash; Forward-Auth") || body.contains("Forward-Auth"));
    assert!(body.contains("Accounts"));
    // forward-auth rows present with current values reflected
    assert!(body.contains("AUTH_PROXY_HEADER"));
    assert!(body.contains("Remote-User"));
    assert!(body.contains("AUTH_PROXY_LOGOUT_URL"));
}
```

Match `create_test_server` / `default_test_config` / imports to whatever `tests/pages_test.rs` already uses (it has its own server builder). If `pages_test.rs` lacks a cookie-saving server builder, model it on `tests/auth_test.rs::create_test_server` (which uses `.save_cookies()`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test pages_test test_settings_page_groups_and_forward_auth`
Expected: FAIL — forward-auth rows / group headers are not in the template yet.

- [ ] **Step 3: Add forward-auth fields to `SettingsTemplate`**

In `src/handlers/pages/mod.rs`, add to `struct SettingsTemplate` (after `webauthn_rp_name`):

```rust
    pub auth_proxy_header: String,
    pub trusted_proxy_networks: String,
    pub auth_proxy_user_creation: bool,
    pub auth_proxy_groups_header: String,
    pub auth_proxy_admin_group: String,
    pub disable_local_auth: bool,
    pub auth_proxy_logout_url: String,
```

- [ ] **Step 4: Populate them in `settings_page`**

In the `settings_page` handler, where `SettingsTemplate { ... }` is constructed, add:

```rust
        auth_proxy_header: state.config.auth_proxy_header.clone(),
        trusted_proxy_networks: state
            .config
            .trusted_proxy_networks
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        auth_proxy_user_creation: state.config.auth_proxy_user_creation,
        auth_proxy_groups_header: state.config.auth_proxy_groups_header.clone(),
        auth_proxy_admin_group: state.config.auth_proxy_admin_group.clone(),
        disable_local_auth: state.config.disable_local_auth,
        auth_proxy_logout_url: state.config.auth_proxy_logout_url.clone().unwrap_or_default(),
```

- [ ] **Step 5: Rewrite the Configuration table in `templates/settings.html`**

Replace the `<table class="mobile-cards-settings">…</table>` block with the grouped, reordered table below. Column order is **Variable · Current · Default · Description**; group header rows use `<tr class="grouphdr">`; the Current `<td>`/`<th>` carry class `settings-col-current`. A small macro keeps the "set or muted" rendering consistent:

```html
{% macro current_text(value) %}{% if value.is_empty() %}<span class="muted">Not set</span>{% else %}<code>{{ value }}</code>{% endif %}{% endmacro %}

<table class="mobile-cards-settings">
    <thead>
        <tr>
            <th class="settings-th">Variable</th>
            <th class="settings-th settings-col-current">Current</th>
            <th class="settings-th">Default</th>
            <th class="settings-th">Description</th>
        </tr>
    </thead>
    <tbody>
        <tr class="grouphdr"><td colspan="4">Server</td></tr>
        <tr>
            <td><code>DATABASE_URL</code></td>
            <td class="settings-col-current" data-label="Current"><code>{{ database_url }}</code></td>
            <td data-label="Default"><code>rdrs.sqlite3</code></td>
            <td data-label="Description">SQLite database file path</td>
        </tr>
        <tr>
            <td><code>SERVER_PORT</code></td>
            <td class="settings-col-current" data-label="Current"><code>{{ server_port }}</code></td>
            <td data-label="Default"><code>3000</code></td>
            <td data-label="Description">HTTP server port</td>
        </tr>

        <tr class="grouphdr"><td colspan="4">Accounts &amp; Registration</td></tr>
        <tr>
            <td><code>SIGNUP_ENABLED</code></td>
            <td class="settings-col-current" data-label="Current">{% if signup_enabled %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
            <td data-label="Default"><code>false</code></td>
            <td data-label="Description">Allow new user registration</td>
        </tr>
        <tr>
            <td><code>MULTI_USER_ENABLED</code></td>
            <td class="settings-col-current" data-label="Current">{% if multi_user_enabled %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
            <td data-label="Default"><code>false</code></td>
            <td data-label="Description">Allow multiple users</td>
        </tr>

        <tr class="grouphdr"><td colspan="4">Authentication &mdash; Passkeys (WebAuthn)</td></tr>
        <tr>
            <td><code>WEBAUTHN_RP_ID</code></td>
            <td class="settings-col-current" data-label="Current"><code data-testid="webauthn-rp-id">{{ webauthn_rp_id }}</code></td>
            <td data-label="Default"><code>localhost</code></td>
            <td data-label="Description">Relying Party ID for passkeys</td>
        </tr>
        <tr>
            <td><code>WEBAUTHN_RP_ORIGIN</code></td>
            <td class="settings-col-current" data-label="Current"><code data-testid="webauthn-rp-origin">{{ webauthn_rp_origin }}</code></td>
            <td data-label="Default"><code>http://localhost:{port}</code></td>
            <td data-label="Description">Relying Party origin URL (must match the URL you deploy at)</td>
        </tr>
        <tr>
            <td><code>WEBAUTHN_RP_NAME</code></td>
            <td class="settings-col-current" data-label="Current"><code data-testid="webauthn-rp-name">{{ webauthn_rp_name }}</code></td>
            <td data-label="Default"><code>rdrs</code></td>
            <td data-label="Description">Relying Party display name</td>
        </tr>

        <tr class="grouphdr"><td colspan="4">Authentication &mdash; Forward-Auth</td></tr>
        <tr>
            <td><code>AUTH_PROXY_HEADER</code></td>
            <td class="settings-col-current" data-label="Current">{% call current_text(auth_proxy_header) %}</td>
            <td data-label="Default"><em>(unset)</em></td>
            <td data-label="Description">Username header from the forward-auth proxy</td>
        </tr>
        <tr>
            <td><code>TRUSTED_PROXY_NETWORKS</code></td>
            <td class="settings-col-current" data-label="Current">{% call current_text(trusted_proxy_networks) %}</td>
            <td data-label="Default"><em>(unset)</em></td>
            <td data-label="Description">CIDRs the TCP peer must match</td>
        </tr>
        <tr>
            <td><code>AUTH_PROXY_USER_CREATION</code></td>
            <td class="settings-col-current" data-label="Current">{% if auth_proxy_user_creation %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
            <td data-label="Default"><code>false</code></td>
            <td data-label="Description">JIT-create unknown users</td>
        </tr>
        <tr>
            <td><code>AUTH_PROXY_GROUPS_HEADER</code></td>
            <td class="settings-col-current" data-label="Current">{% call current_text(auth_proxy_groups_header) %}</td>
            <td data-label="Default"><em>(unset)</em></td>
            <td data-label="Description">Groups header from the forward-auth proxy</td>
        </tr>
        <tr>
            <td><code>AUTH_PROXY_ADMIN_GROUP</code></td>
            <td class="settings-col-current" data-label="Current">{% call current_text(auth_proxy_admin_group) %}</td>
            <td data-label="Default"><em>(unset)</em></td>
            <td data-label="Description">Group membership that grants admin</td>
        </tr>
        <tr>
            <td><code>DISABLE_LOCAL_AUTH</code></td>
            <td class="settings-col-current" data-label="Current">{% if disable_local_auth %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
            <td data-label="Default"><code>false</code></td>
            <td data-label="Description">Hide the browser password login form</td>
        </tr>
        <tr>
            <td><code>AUTH_PROXY_LOGOUT_URL</code></td>
            <td class="settings-col-current" data-label="Current">{% call current_text(auth_proxy_logout_url) %}</td>
            <td data-label="Default"><em>(unset)</em></td>
            <td data-label="Description">Sign Out redirects here to end the SSO session</td>
        </tr>

        <tr class="grouphdr"><td colspan="4">Content &amp; Media</td></tr>
        <tr>
            <td><code>USER_AGENT</code></td>
            <td class="settings-col-current" data-label="Current"><code>{{ user_agent }}</code> <span class="muted">({% if user_agent_is_default %}default{% else %}custom{% endif %})</span></td>
            <td data-label="Default"><code>RDRS/{version} (...)</code></td>
            <td data-label="Description">User agent for HTTP requests</td>
        </tr>
        <tr>
            <td><code>IMAGE_PROXY_SECRET</code></td>
            <td class="settings-col-current" data-label="Current">{% if image_proxy_secret_generated %}<span class="muted">Auto-generated</span>{% else %}<span class="success-text">Configured</span>{% endif %}</td>
            <td data-label="Default"><em>(auto-generated)</em></td>
            <td data-label="Description">Secret key for image proxy URLs</td>
        </tr>
    </tbody>
</table>
```

If Askama rejects the `{% macro %}` placement inside the block, define it via an `{% import %}` of an existing macros file or inline the "Not set" conditional per row instead — keep the rendered output identical.

- [ ] **Step 6: Add CSS for group headers, balanced header padding, and the Current column**

In `static/css/app.css`:

```css
/* App settings: balanced vertical padding so the header reads even under the tint */
.settings-th {
    padding-top: var(--space-2);
    padding-bottom: var(--space-2);
}
/* group label rows in the configuration table */
.grouphdr td {
    background: var(--color-accent-subtle);
    color: var(--color-accent);
    font-size: var(--font-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: var(--space-2) var(--space-8) var(--space-2) 0;
}
/* anchor the eye on the running value; keep its top/bottom spacing even */
.settings-col-current {
    background: var(--color-accent-subtle);
    vertical-align: middle;
}
```

Verify the existing responsive `mobile-cards-settings` rules render `.grouphdr` acceptably in card view (the `<td colspan="4">` becomes a block). If it looks wrong, add inside the existing mobile `@media` block:

```css
    .page-content table.mobile-cards-settings .grouphdr td {
        display: block;
        width: 100%;
    }
```

- [ ] **Step 7: Rebuild, run the test + full suite**

Run: `cargo build && cargo fmt && RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test pages_test test_settings_page_groups_and_forward_auth && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run`
Expected: the new test PASSES (template compiles with the new fields; rows + group headers present); whole suite green.

- [ ] **Step 8: Commit**

```bash
git add src/handlers/pages/mod.rs templates/settings.html static/css/app.css tests/pages_test.rs
git commit -m "feat(ui): group App config table and surface forward-auth settings

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Documentation + screenshot check

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update `ARCHITECTURE.md`**

In the "Forward-Auth (Trusted-Header) Login" section, add a short note (do not re-document the env vars):

> The sidebar shows an **SSO** pill when the current request is served through forward-auth — computed per request from the trusted proxy header (no stored state), surfaced via `via_forward_auth` on the auth extractors and the sidebar payload. The App page (`/settings`) lists the forward-auth configuration under a grouped Configuration table.

- [ ] **Step 2: Confirm screenshots are unaffected**

Run: `cargo build && cd e2e && npm run screenshots && cd .. && git status --short screenshots/`
Expected: no changes under `screenshots/` — the demo data uses default (local) config, so no SSO pill renders and the sidebar footer is unchanged; `/settings` is not screenshotted. If (unexpectedly) a screenshot changed, investigate before committing; only stage regenerated images if the change is intended.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: note the sidebar SSO pill and App forward-auth config

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Dynamic detection helper + middleware reuse → Task 1. ✓
- `via_forward_auth` on `AuthUser`/`PageAuthUser` + `SidebarResponse` + `build_app_layout`/`get_sidebar` → Task 2. ✓
- Sidebar SSO pill (hugging username, badge style, local = no pill) + rerender trigger → Task 3. ✓
- App page 5 groups + column order Variable·Current·Default·Description + forward-auth rows + balanced header padding + Current tint/centering + mobile card view → Task 4. ✓
- Docs note + screenshot confirmation → Task 5. ✓
- No schema change / no persistence → honored (Task 1/2 compute per request). ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. The two "if Askama/CSS behaves differently" fallbacks name the concrete alternative, not a vague placeholder. ✓

**Type consistency:** `forward_auth_identity(&Config, Option<IpAddr>, &HeaderMap) -> Option<String>`; `via_forward_auth: bool` on `AuthUser`, `PageAuthUser`, `SidebarResponse`; `build_sidebar_response(state, user, session, via_forward_auth)`; JS reads `data.via_forward_auth`; CSS classes `.sidebar-id`, `.sidebar-auth-pill`, `.grouphdr`, `.settings-col-current` used consistently across tasks. ✓

## Notes / risks

- The `forward_auth_identity` test and the via_forward_auth integration test both rely on `tests/forward_auth_test.rs`'s `.http_transport()` server (real loopback `ConnectInfo`). The extractors read `ConnectInfo` from request extensions — present under that transport.
- `tests/pages_test.rs` may not currently save cookies; if its server builder lacks `.save_cookies()`, add a local builder modeled on `tests/auth_test.rs::create_test_server` rather than weakening the test.
- Adding `via_forward_auth` to `AuthUser`/`PageAuthUser` only requires updating the two literal constructions in `src/middleware/auth.rs` (no other literal constructions exist in the codebase).
