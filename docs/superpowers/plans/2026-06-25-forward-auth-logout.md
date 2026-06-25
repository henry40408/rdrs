# Forward-Auth Logout Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Sign Out work under forward-auth — clear the cookie properly, let forward-auth re-authenticate past a stale/invalid cookie, forward `/login` to `/` for authenticated users, and add `AUTH_PROXY_LOGOUT_URL` for real IdP logout.

**Architecture:** Four targeted changes — logout cookie clearing + a JSON redirect target (Bug 1 / Item 4), a forward-auth session-validity check replacing the cookie-presence short-circuit (Bug 2), an authenticated `/login → /` redirect (Item 3), and docs. Behavior matches linkding/Miniflux (Option A): local logout bounces back in via the proxy header unless `AUTH_PROXY_LOGOUT_URL` ends the SSO session.

**Tech Stack:** Rust, axum 0.8, axum-extra cookies, rusqlite, axum-test, vanilla ES module (sidebar web component).

## Global Constraints

- Run tests with `cargo nextest run` (never `cargo test`); prefix local runs with `RDRS_FAST_HASH=1`.
- `cargo fmt` before every commit; `cargo clippy -- -D warnings` must pass.
- Commits GPG-signed (default git config); end messages with the `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` trailer.
- Stage files explicitly by name — never `git add -A` / `git add .`.
- No database schema change.
- The `Config` struct has FOUR literal construction sites that must all stay in sync when a field is added: `src/config.rs` (`test_config`), `src/auth/webauthn.rs` (`test_config`), `tests/common/mod.rs` (`default_test_config`), `tests/statistics_test.rs`.
- Static JS is embedded at compile time; run `cargo build` after editing `static/js/**` before any manual/e2e check. (No e2e logout test exists, so no e2e gate here.)
- The session cookie is set with `.path("/")` (login handler + forward-auth middleware); any removal MUST match that path.

---

### Task 1: Logout cookie clearing + `AUTH_PROXY_LOGOUT_URL` + Sign Out JS

Fixes Bug 1 (cookie never cleared) and adds Item 4 (configurable IdP logout redirect). The config field's only consumer is the logout handler, so it lives here.

**Files:**
- Modify: `src/config.rs` (new field + parse; update `test_config` literal)
- Modify: `src/auth/webauthn.rs` (`test_config` literal)
- Modify: `tests/common/mod.rs` (`default_test_config` literal)
- Modify: `tests/statistics_test.rs` (Config literal)
- Modify: `src/handlers/auth.rs` (`logout` handler + `LogoutResponse`)
- Modify: `static/js/components/rdrs-sidebar.js` (Sign Out handler)
- Test: `tests/auth_test.rs`

**Interfaces:**
- Produces:
  - `Config.auth_proxy_logout_url: Option<String>` (env `AUTH_PROXY_LOGOUT_URL`)
  - `handlers::auth::LogoutResponse { redirect_to: String }`
  - `logout(...) -> AppResult<(CookieJar, Json<LogoutResponse>)>`

- [ ] **Step 1: Write the failing tests**

Add to `tests/auth_test.rs` (uses the file's existing `create_test_server` / `default_test_config` / `json!` / `StatusCode`):

```rust
/// Collect raw Set-Cookie header values from a response.
fn set_cookie_headers(res: &axum_test::TestResponse) -> Vec<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
        .collect()
}

#[tokio::test]
async fn test_logout_clears_cookie_with_path() {
    let server = create_test_server(default_test_config());
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status_ok();

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let removal = set_cookie_headers(&res)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("logout must emit a session_token Set-Cookie");
    assert!(
        removal.contains("Path=/"),
        "removal cookie must carry Path=/ to actually delete the session cookie: {removal}"
    );
}

#[tokio::test]
async fn test_logout_redirect_default_is_login() {
    let server = create_test_server(default_test_config());
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status_ok();

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["redirect_to"], "/login");
}

#[tokio::test]
async fn test_logout_redirect_uses_configured_url() {
    let mut config = default_test_config();
    config.auth_proxy_logout_url = Some("https://auth.example.com/logout".to_string());
    let server = create_test_server(config);
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status_ok();

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["redirect_to"], "https://auth.example.com/logout");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_logout_`
Expected: FAIL — `auth_proxy_logout_url` field doesn't exist / response has no `redirect_to`.

- [ ] **Step 3: Add the config field**

In `src/config.rs`, add to `struct Config` after `auth_proxy_admin_group`:

```rust
    pub auth_proxy_logout_url: Option<String>,
```

In `from_env`, add (mirror the `public_base_url` style):

```rust
            auth_proxy_logout_url: env::var("AUTH_PROXY_LOGOUT_URL").ok().filter(|s| !s.is_empty()),
```

Add `auth_proxy_logout_url: None,` to the `test_config()` literal in `src/config.rs`.

- [ ] **Step 4: Update the other three Config literals**

Add `auth_proxy_logout_url: None,` to the `Config { ... }` literal in each of:
- `src/auth/webauthn.rs` (`test_config`)
- `tests/common/mod.rs` (`default_test_config`)
- `tests/statistics_test.rs`

- [ ] **Step 5: Rewrite the logout handler**

In `src/handlers/auth.rs`, replace the `logout` function with:

```rust
#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub redirect_to: String,
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<(CookieJar, Json<LogoutResponse>)> {
    let token = auth_user.session.session_token.clone();
    state
        .db
        .user(move |conn| session::delete_session(conn, &token))
        .await??;

    // Removal must match the Path=/ the cookie was set with, or the browser
    // keeps the (now-invalid) session_token cookie. Mirrors flash.rs.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();

    let redirect_to = state
        .config
        .auth_proxy_logout_url
        .clone()
        .unwrap_or_else(|| "/login".to_string());

    Ok((jar.remove(removal), Json(LogoutResponse { redirect_to })))
}
```

(`Cookie`, `Json`, `Serialize`, `SESSION_COOKIE_NAME` are already imported in this file.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_logout`
Expected: PASS (including the existing `test_logout`, whose `assert_status_ok` + `/api/user` 401 still hold).

- [ ] **Step 7: Update the Sign Out JS**

In `static/js/components/rdrs-sidebar.js`, in the `[data-rdrs-logout]` click handler, replace the success branch so it uses the server-provided target (preserve the flash for local paths; navigate directly for an external IdP URL):

```js
const r = await fetch('/api/session', { method: 'DELETE' });
if (r.ok) {
    const d = await r.json();
    if (d.redirect_to.startsWith('/')) {
        window.flash.redirect(d.redirect_to, 'info', 'You have been logged out.');
    } else {
        window.location.href = d.redirect_to;
    }
} else {
    window.flash.error('Logout failed');
}
```

- [ ] **Step 8: Rebuild (embedded asset) + full check**

Run: `cargo build && cargo fmt && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run`
Expected: builds, clippy clean, full suite green.

- [ ] **Step 9: Commit**

```bash
git add src/config.rs src/auth/webauthn.rs tests/common/mod.rs tests/statistics_test.rs src/handlers/auth.rs static/js/components/rdrs-sidebar.js tests/auth_test.rs
git commit -m "fix(auth): clear session cookie on logout and add AUTH_PROXY_LOGOUT_URL

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: forward-auth re-authenticates past an invalid cookie

Fixes Bug 2 — the middleware short-circuits on cookie *presence*, so a stale/expired `session_token` (e.g. after logout, or on session expiry) permanently blocks forward-auth. Change it to short-circuit only on a *valid* session.

**Files:**
- Modify: `src/middleware/forward_auth.rs` (the cookie short-circuit)
- Test: `tests/forward_auth_test.rs`

**Interfaces:**
- Consumes: `session::find_by_token`, `Session::is_expired`, `DbPool::read_user` (existing).
- Produces: no new public API.

- [ ] **Step 1: Write the failing tests**

Add to `tests/forward_auth_test.rs` (follow the file's existing helpers: `create_server`, `seed_user`, `open_shared_memory`, per-test unique DB name, `parse_trusted_networks`, `add_header`; the server is built with `.http_transport()`):

```rust
#[tokio::test]
async fn test_invalid_session_cookie_still_forward_auths() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user("erin", rdrs::models::user::Role::User);

    // A stale/garbage session_token cookie must NOT block forward-auth.
    let res = server
        .get("/")
        .add_header("Remote-User", "erin")
        .add_cookie(cookie::Cookie::new("session_token", "stale-invalid"))
        .await;

    assert!(res.status_code().is_redirection());
    let fresh = res
        .maybe_cookie("session_token")
        .expect("a fresh session cookie should be minted");
    assert_ne!(fresh.value(), "stale-invalid");
    assert!(!fresh.value().is_empty());
}

#[tokio::test]
async fn test_valid_session_cookie_not_reminted() {
    let server = create_server(|c| {
        c.trusted_proxy_networks = parse_trusted_networks("127.0.0.0/8").unwrap();
    });
    seed_user("frank", rdrs::models::user::Role::User);

    // First login mints a valid session (saved by the client jar).
    server.get("/").add_header("Remote-User", "frank").await;

    // Second request carries the now-valid cookie: middleware must pass through
    // and NOT mint a new session_token.
    let res = server.get("/").add_header("Remote-User", "frank").await;
    let reminted = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|s| s.starts_with("session_token="));
    assert!(!reminted, "a valid session must not be re-minted on every request");
}
```

Note: `create_server` uses `.save_cookies()`, so the first request's cookie is reused on the second.

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test test_invalid_session test_valid_session`
Expected: FAIL — `test_invalid_session_cookie_still_forward_auths` fails because the present cookie currently short-circuits forward-auth (no fresh cookie, not a redirect).

- [ ] **Step 3: Replace the cookie short-circuit**

In `src/middleware/forward_auth.rs`, find:

```rust
    // Feature off, or already carrying a session cookie → nothing to do.
    if !config.auth_proxy_enabled() || jar.get(SESSION_COOKIE_NAME).is_some() {
        return next.run(req).await;
    }
```

Replace with:

```rust
    // Feature off → nothing to do.
    if !config.auth_proxy_enabled() {
        return next.run(req).await;
    }

    // Already carrying a VALID (non-expired) session → leave it to the normal
    // flow. A present-but-invalid cookie (e.g. after logout or expiry) must NOT
    // block forward-auth, or the user is locked out.
    if let Some(token) = jar.get(SESSION_COOKIE_NAME).map(|c| c.value().to_string()) {
        let valid = state
            .db
            .read_user(move |conn| {
                Ok::<bool, AppError>(
                    session::find_by_token(conn, &token)?
                        .map(|s| !s.is_expired())
                        .unwrap_or(false),
                )
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false);
        if valid {
            return next.run(req).await;
        }
    }
```

(`session`, `AppError` are already imported in this file; `read_user` exists on `DbPool`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test forward_auth_test`
Expected: PASS — all forward_auth tests, including the two new ones and the existing 6.

- [ ] **Step 5: Lint + commit**

```bash
cargo fmt && cargo clippy -- -D warnings
git add src/middleware/forward_auth.rs tests/forward_auth_test.rs
git commit -m "fix(auth): forward-auth re-authenticates past an invalid session cookie

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `/login` redirects authenticated users to `/`

Item 3 — so that after forward-auth re-authenticates on `/login`, the user lands in the app instead of staring at the login form (matches Django's login view).

**Files:**
- Modify: `src/handlers/pages/mod.rs` (`login_page`)
- Test: `tests/auth_test.rs`

**Interfaces:**
- Consumes: `PageAuthUser` (already imported), `Redirect`, `Response` (already imported).

- [ ] **Step 1: Write the failing tests**

Add to `tests/auth_test.rs`:

```rust
#[tokio::test]
async fn test_login_page_redirects_authenticated_to_root() {
    let server = create_test_server(default_test_config());
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status_ok();

    let res = server.get("/login").await;
    assert!(res.status_code().is_redirection());
    assert_eq!(res.header("location"), "/");
}

#[tokio::test]
async fn test_login_page_renders_when_anonymous() {
    let server = create_test_server(default_test_config());
    let res = server.get("/login").await;
    res.assert_status_ok();
    assert!(res.text().contains("login-form") || res.text().contains("rdrs"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_login_page`
Expected: FAIL — `test_login_page_redirects_authenticated_to_root` gets 200 (form), not a redirect.

- [ ] **Step 3: Add the authenticated redirect**

In `src/handlers/pages/mod.rs`, replace the `login_page` signature and body so it redirects authenticated users:

```rust
pub async fn login_page(
    State(state): State<AppState>,
    auth: Option<PageAuthUser>,
    flash: Flash,
) -> Response {
    if auth.is_some() {
        return Redirect::to("/").into_response();
    }

    let signup_enabled = state
        .db
        .read_user(|c| crate::models::user::count(c).ok())
        .await
        .ok()
        .flatten()
        .map(|count| state.config.can_register(count))
        .unwrap_or(false);

    (
        flash.clone(),
        LoginTemplate {
            signup_enabled,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            local_auth_enabled: !state.config.disable_local_auth,
        },
    )
        .into_response()
}
```

(`Option<PageAuthUser>` returns `None` when there is no valid session — axum implements `FromRequestParts` for `Option<T>`. The return type changes from `(Flash, LoginTemplate)` to `Response`; `(Flash, LoginTemplate)` implements `IntoResponse`, so `.into_response()` is valid.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test auth_test test_login_page`
Expected: PASS.

- [ ] **Step 5: Full check + commit**

```bash
cargo fmt && cargo clippy -- -D warnings && RDRS_FAST_HASH=1 cargo nextest run
git add src/handlers/pages/mod.rs tests/auth_test.rs
git commit -m "fix(auth): redirect authenticated users away from /login to /

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

Document `AUTH_PROXY_LOGOUT_URL` and the forward-auth logout behavior.

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `README.md`

**Interfaces:** none.

- [ ] **Step 1: Update `README.md`**

Add a row to the environment-variable table:

| Variable | Default | Description |
|---|---|---|
| `AUTH_PROXY_LOGOUT_URL` | (unset) | When set, Sign Out redirects the browser here (e.g. the Authelia logout URL) to end the SSO session. When unset, Sign Out clears the local session and the proxy header re-authenticates on the next request (you return to the app). |

- [ ] **Step 2: Update `ARCHITECTURE.md`**

In the "Forward-Auth (Trusted-Header) Login" section, add a short "Logout" note:

- Sign Out clears the local `session_token` cookie (with `Path=/`) and deletes the server-side session.
- Forward-auth re-authenticates whenever there is no *valid* session cookie (a stale/expired cookie does not block it), so under forward-auth a local Sign Out bounces the user back in via the proxy header (matches linkding/Miniflux) — unless `AUTH_PROXY_LOGOUT_URL` is set, in which case Sign Out redirects to that URL to end the IdP/SSO session.
- `/login` redirects an already-authenticated user to `/`.

Also add `AUTH_PROXY_LOGOUT_URL` to the configuration variable list in this document.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md README.md
git commit -m "docs: document AUTH_PROXY_LOGOUT_URL and forward-auth logout behavior

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Item 1 (Bug 1, cookie clearing) → Task 1 (Step 5, `.path("/")` removal). ✓
- Item 2 (Bug 2, validity check) → Task 2. ✓
- Item 3 (`/login` → `/`) → Task 3. ✓
- Item 4 (`AUTH_PROXY_LOGOUT_URL` + JSON redirect + JS) → Task 1 (config, handler, JS). ✓
- Resulting-behavior table → covered by Tasks 1–3 together; documented in Task 4. ✓
- Testing bullets → Task 1 (cookie path, redirect default/configured), Task 2 (invalid/valid cookie), Task 3 (authed redirect, anon render). ✓
- 4 Config literals kept in sync → Task 1 Steps 3–4. ✓
- Docs → Task 4. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✓

**Type consistency:** `auth_proxy_logout_url: Option<String>`, `LogoutResponse { redirect_to: String }`, `logout -> AppResult<(CookieJar, Json<LogoutResponse>)>`, `login_page -> Response`, and the middleware validity block use consistent names/signatures across tasks and match the existing imports verified in the codebase. ✓

## Notes / risks

- `Option<PageAuthUser>` does its own DB lookup; combined with the forward-auth middleware's new validity lookup, an authenticated `/login` hit does two reads. Acceptable (single indexed `find_by_token` each; same lookup `PageAuthUser` already performs).
- Use `cookie::Cookie::new(...)` with `.add_cookie(...)` and `res.maybe_cookie("session_token")` — exactly as `tests/auth_test.rs` (`cookie` is a dev-dependency, `cookie = "0.18"`) and `tests/forward_auth_test.rs` already do.
- No e2e logout test exists, so the JS change is covered by the Rust handler tests + `cargo build`; verify manually if a browser is available.
