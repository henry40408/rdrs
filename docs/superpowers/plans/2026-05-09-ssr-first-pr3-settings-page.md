# SSR-first PR-3: /settings Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/settings` from CSR shell + JSON API to direct SSR rendering — first per-route content migration of the SSR-first redesign. Also clean up two orphan inline functions in `base.html` (`toggleSidebar` / `closeSidebar`) flagged during PR-2 review.

**Architecture:** Two independent commits. Task 1 moves the orphan helpers from `base.html` (where login/register inherit them but never use them) into `static/js/app.js` as global window functions, matching the existing `window.rdrsNavigate` stub pattern. Task 2 SSR-izes the `/settings` page: `templates/settings.html` renders the server config directly from template fields populated by the handler from `state.config`; `<rdrs-settings-page>` element + `static/js/pages/settings.js` + `GET /api/server-config` JSON endpoint all deleted.

**Tech Stack:** Rust + Axum + Askama + vanilla JS — no new deps.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-settings-page` (already created off updated `main` at commit `9b4287c`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-settings-page`, working tree clean, latest commit on main is `9b4287c fix(sidebar): set active-category-id on category and feed pages (#187)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: all tests pass (704 from PR-2 minus the deleted spa-router tests; whatever the count is should match `main` after #186/#187 merged).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify (T1) | `templates/base.html` | Remove the `toggleSidebar()` / `closeSidebar()` function declarations from the inline `<script>` block. The other inline content (theme controller, bfcache restore helper, pageshow listener) stays. |
| Modify (T1) | `static/js/app.js` | Add `window.toggleSidebar` / `window.closeSidebar` as `window.*` assignments alongside the existing `window.rdrsNavigate` stub. They must be globals because `<rdrs-sidebar>`'s render emits `onclick="toggleSidebar()"` / `onclick="closeSidebar()"` inline. |
| Modify (T2) | `templates/settings.html` | Replace `<rdrs-settings-page></rdrs-settings-page>` + the per-page-script block with the full SSR markup that today's `static/js/pages/settings.js::_render(cfg)` produces. Use Askama variable substitution for the four config fields. |
| Modify (T2) | `src/handlers/pages.rs` | `SettingsTemplate` gets four new fields: `pub user_agent: String`, `pub user_agent_is_default: bool`, `pub signup_enabled: bool`, `pub multi_user_enabled: bool`. Handler `settings_page` reads from `state.config` to populate them. |
| Modify (T2) | `src/lib.rs` | Remove `.route("/api/server-config", get(handlers::user::get_server_config))`. |
| Modify (T2) | `src/handlers/user.rs` | Delete `pub async fn get_server_config(...)` and `ServerConfigResponse` struct. Verify no other consumer (greps clean inside `src/`). |
| Delete (T2) | `static/js/pages/settings.js` | Page module no longer needed. |
| Modify (T2) | `src/handlers/static_assets.rs` | Remove the `("js/pages/settings.js", include_str!("../../static/js/pages/settings.js"))` line. |
| Modify (T2) | `tests/pages_test.rs` | (a) Update `test_settings_page_serves_csr_shell` (rename to `test_settings_page_renders_ssr`): drop assertions on `<rdrs-settings-page>` and `/static/js/pages/settings.js`, add assertions on rendered SSR content (e.g. "User Agent" header, version string). (b) Delete `test_api_server_config_returns_signup_status` and `test_api_server_config_with_custom_user_agent` — endpoint is gone. (c) Add a new test asserting custom `user_agent` and `signup_enabled` values render correctly into the SSR settings page. |

---

## Task 1: Move orphan sidebar helpers from base.html to app.js

`base.html` currently defines `toggleSidebar()` and `closeSidebar()` as plain function declarations inside a non-module `<script>` block. After PR-2 these are pre-login dead code (login/register have no sidebar element), but they're still needed for logged-in pages because `<rdrs-sidebar>`'s render emits `onclick="toggleSidebar()"` / `onclick="closeSidebar()"` inline. Move them into `static/js/app.js` (the logged-in JS surface) as `window.*` assignments, matching the existing `window.rdrsNavigate` stub pattern.

**Files:**
- Modify: `templates/base.html`
- Modify: `static/js/app.js`

- [ ] **Step 1: Delete the two function declarations from `templates/base.html`.**

  Edit `templates/base.html`. Remove lines 40-55 (the `// Sidebar toggle` comment + both function declarations). The surrounding inline `<script>` block stays — only those two functions move.

  After this edit, `base.html`'s inline `<script>` contains only: theme controller, bfcache restore helper. `<rdrs-flash.js>` import + closing `<head>`-block remain unchanged.

- [ ] **Step 2: Add the two helpers to `static/js/app.js`.**

  Edit `static/js/app.js`. Append below the existing `window.rdrsNavigate = ...` stub (around line 95-97), BEFORE the `installSwap()` invocation at the end:

  ```js
  // Sidebar mobile-toggle helpers. <rdrs-sidebar>'s render emits
  // inline `onclick="toggleSidebar()"` / `onclick="closeSidebar()"`,
  // which require global functions — assign to `window` because
  // module-scope declarations are not visible to inline event
  // attributes.
  window.toggleSidebar = function() {
      const sidebar = document.getElementById('sidebar');
      const toggle = document.querySelector('.sidebar-toggle');
      if (sidebar) {
          sidebar.classList.toggle('open');
          if (toggle) toggle.style.display = sidebar.classList.contains('open') ? 'none' : '';
      }
  };

  window.closeSidebar = function() {
      const sidebar = document.getElementById('sidebar');
      const toggle = document.querySelector('.sidebar-toggle');
      if (sidebar) sidebar.classList.remove('open');
      if (toggle) toggle.style.display = '';
  };
  ```

- [ ] **Step 3: Compile + test.**

  Run: `cargo nextest run`
  Expected: all tests pass. The chrome contract test (`test_logged_in_page_loads_full_chrome`) does not assert on these helpers, so it stays green. Login/register tests do not assert on them either.

- [ ] **Step 4: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/base.html static/js/app.js
  git commit -m "$(cat <<'EOF'
  refactor(ssr): move sidebar mobile helpers from base.html to app.js

  toggleSidebar() / closeSidebar() were leftovers in base.html after
  PR-2 slimmed it to a pre-login shell — only logged-in pages mount
  <rdrs-sidebar> and call them. Move both to app.js (the logged-in
  JS surface) as window.* assignments matching window.rdrsNavigate.
  base.html is now genuinely minimal: theme + bfcache restore.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: SSR /settings

Replaces `<rdrs-settings-page>` with direct server-rendered settings content. Deletes the JSON API and the page module.

**Files:**
- Modify: `templates/settings.html`
- Modify: `src/handlers/pages.rs` (extend `SettingsTemplate`, populate from `state.config`)
- Modify: `src/lib.rs` (drop `/api/server-config` route)
- Modify: `src/handlers/user.rs` (delete `get_server_config` + `ServerConfigResponse`)
- Modify: `src/handlers/static_assets.rs` (drop allowlist entry)
- Delete: `static/js/pages/settings.js`
- Modify: `tests/pages_test.rs`

### Order: Rust source edits BEFORE deleting `settings.js`

`include_str!("../../static/js/pages/settings.js")` resolves at compile time. Remove the allowlist entry (Step 5) BEFORE deleting the file (Step 6) — same pattern as router.js cleanup in PR-2 Task 4.

- [ ] **Step 1: Update `templates/settings.html`.**

  Replace the file contents with:

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="settings"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Settings</h1>
                  <div id="settings-body">
                      <p class="muted">Version: <code>{{ git_version }}</code></p>

                      <h2>Configuration</h2>
                      <p class="muted">These settings are configured via environment variables and cannot be changed at runtime.</p>

                      <h3>HTTP Client</h3>
                      <table>
                          <tbody>
                              <tr>
                                  <th class="settings-th">User Agent</th>
                                  <td>
                                      <code>{{ user_agent }}</code>
                                      <span class="muted">({% if user_agent_is_default %}default{% else %}custom{% endif %})</span>
                                  </td>
                              </tr>
                          </tbody>
                      </table>

                      <h3>User Registration</h3>
                      <table>
                          <tbody>
                              <tr>
                                  <th class="settings-th">Signup Enabled</th>
                                  <td>{% if signup_enabled %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
                              </tr>
                              <tr>
                                  <th class="settings-th">Multi-User Mode</th>
                                  <td>{% if multi_user_enabled %}<span class="success-text">Yes</span>{% else %}<span class="muted">No</span>{% endif %}</td>
                              </tr>
                          </tbody>
                      </table>

                      <h3>Environment Variables</h3>
                      <p class="muted">Configure these environment variables to customize RDRS:</p>
                      <table class="mobile-cards-settings">
                          <thead>
                              <tr>
                                  <th class="settings-th">Variable</th>
                                  <th class="settings-th">Description</th>
                                  <th class="settings-th">Default</th>
                              </tr>
                          </thead>
                          <tbody>
                              <tr>
                                  <td><code>DATABASE_URL</code></td>
                                  <td data-label="Description">SQLite database file path</td>
                                  <td data-label="Default"><code>rdrs.sqlite3</code></td>
                              </tr>
                              <tr>
                                  <td><code>SERVER_PORT</code></td>
                                  <td data-label="Description">HTTP server port</td>
                                  <td data-label="Default"><code>3000</code></td>
                              </tr>
                              <tr>
                                  <td><code>USER_AGENT</code></td>
                                  <td data-label="Description">User agent for HTTP requests</td>
                                  <td data-label="Default"><code>RDRS/{version} (...)</code></td>
                              </tr>
                              <tr>
                                  <td><code>SIGNUP_ENABLED</code></td>
                                  <td data-label="Description">Allow new user registration</td>
                                  <td data-label="Default"><code>false</code></td>
                              </tr>
                              <tr>
                                  <td><code>MULTI_USER_ENABLED</code></td>
                                  <td data-label="Description">Allow multiple users</td>
                                  <td data-label="Default"><code>false</code></td>
                              </tr>
                              <tr>
                                  <td><code>IMAGE_PROXY_SECRET</code></td>
                                  <td data-label="Description">Secret key for image proxy URLs</td>
                                  <td data-label="Default"><em>(auto-generated)</em></td>
                              </tr>
                          </tbody>
                      </table>
                  </div>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - No `{% block page_script %}` (page module deleted).
  - References `{{ git_version }}`, `{{ user_agent }}`, `{{ user_agent_is_default }}`, `{{ signup_enabled }}`, `{{ multi_user_enabled }}` — all top-level fields on `SettingsTemplate` (Step 2 below adds the four new ones).
  - `<rdrs-sidebar>` and `<rdrs-flash>` stay as CSR custom elements (chrome) — they'll be SSR-ized in a later PR.
  - Askama auto-escapes by default. `git_version` is `&'static str` so safe; the others come from `state.config` and are already safe content. No `|safe` needed.

- [ ] **Step 2: Extend `SettingsTemplate` and rewrite `settings_page` handler in `src/handlers/pages.rs`.**

  Locate the `SettingsTemplate` struct (around line ~415 — the per-route struct added by PR-2 Task 2). Replace its definition + the `settings_page` handler body. New shape:

  ```rust
  #[derive(Template)]
  #[template(path = "settings.html")]
  pub struct SettingsTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub user_agent: String,
      pub user_agent_is_default: bool,
      pub signup_enabled: bool,
      pub multi_user_enabled: bool,
  }

  impl IntoResponse for SettingsTemplate {
      fn into_response(self) -> Response {
          match self.render() {
              Ok(html) => Html(html).into_response(),
              Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
          }
      }
  }
  ```

  And the handler:

  ```rust
  pub async fn settings_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, SettingsTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let user_agent_is_default = state.config.user_agent == crate::config::DEFAULT_USER_AGENT;

      (
          flash,
          SettingsTemplate {
              title: "Settings",
              git_version: crate::GIT_VERSION,
              layout,
              user_agent: state.config.user_agent.clone(),
              user_agent_is_default,
              signup_enabled: state.config.signup_enabled,
              multi_user_enabled: state.config.multi_user_enabled,
          },
      )
  }
  ```

- [ ] **Step 3: Drop the route registration from `src/lib.rs`.**

  Edit `src/lib.rs`. Remove the line:
  ```rust
          .route("/api/server-config", get(handlers::user::get_server_config))
  ```

  No replacement.

- [ ] **Step 4: Delete `get_server_config` + `ServerConfigResponse` from `src/handlers/user.rs`.**

  Locate `get_server_config` handler (around line 133) and the `ServerConfigResponse` struct (above it, around line 120-129). Delete both, including the doc comment block above the handler. Verify zero remaining references:

  ```bash
  grep -n "get_server_config\|ServerConfigResponse" src/
  ```
  Expected: zero matches.

- [ ] **Step 5: Drop the static-assets allowlist entry for `js/pages/settings.js`.**

  Edit `src/handlers/static_assets.rs`. Remove the line:
  ```rust
      (
          "js/pages/settings.js",
          include_str!("../../static/js/pages/settings.js"),
      ),
  ```

- [ ] **Step 6: Delete `static/js/pages/settings.js`.**

  ```bash
  git rm static/js/pages/settings.js
  ```

- [ ] **Step 7: Update `tests/pages_test.rs`.**

  (a) Find `test_settings_page_serves_csr_shell` (around line 203). Rename to `test_settings_page_renders_ssr_content` and replace its body:

  ```rust
  #[tokio::test]
  async fn test_settings_page_renders_ssr_content() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app.server.get("/settings").await;
      response.assert_status_ok();
      let body = response.text();

      // SSR content — no more <rdrs-settings-page> element / page-script.
      assert!(!body.contains("<rdrs-settings-page>"));
      assert!(!body.contains("/static/js/pages/settings.js"));

      // Server-rendered content from default config.
      assert!(body.contains("<h1>Settings</h1>"));
      assert!(body.contains("Configuration"));
      assert!(body.contains("User Agent"));
      assert!(body.contains("Signup Enabled"));
      assert!(body.contains("Environment Variables"));
  }
  ```

  (b) Find `test_api_server_config_returns_signup_status` (around line 514) and `test_api_server_config_with_custom_user_agent` (around line 534) plus the `// /api/server-config tests` comment header above them. Delete both tests and the comment.

  (c) Add a new test that exercises non-default `Config` values — confirms the SSR template renders custom values:

  ```rust
  #[tokio::test]
  async fn test_settings_page_reflects_custom_config() {
      let config = Config {
          user_agent: "Custom-Agent/2.0".to_string(),
          signup_enabled: true,
          multi_user_enabled: true,
          ..default_test_config()
      };
      let app = create_test_app(config);
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app.server.get("/settings").await;
      response.assert_status_ok();
      let body = response.text();

      assert!(body.contains("Custom-Agent/2.0"));
      assert!(body.contains("(custom)"));
      // Both Yes flags rendered for signup + multi-user.
      let yes_count = body.matches("<span class=\"success-text\">Yes</span>").count();
      assert!(yes_count >= 2, "expected ≥2 Yes badges, got {yes_count}");
  }
  ```

- [ ] **Step 8: Compile + test.**

  Run: `cargo nextest run`
  Expected: all tests pass. The total count goes down by 2 (the two `/api/server-config` tests deleted) and up by 1 (new custom-config test). Net -1.

  If `cargo build` fails on Askama template compilation, common causes:
  - Wrong field name in template (e.g. `{{ user_agent_default }}` instead of `{{ user_agent_is_default }}`).
  - Missing `pub` on a new field.
  - Template-struct field type mismatch (e.g. expected `&str`, got `String` in template's literal context — usually fine for Askama).

- [ ] **Step 9: Verify no remaining references leak.**

  ```bash
  grep -rn "rdrs-settings-page\|/api/server-config\|get_server_config\|ServerConfigResponse\|/static/js/pages/settings.js" src/ templates/ static/ tests/
  ```
  Acceptable matches: zero across `src/`, `templates/`, `static/`. In `tests/`, the new SSR-content test asserts `!body.contains("<rdrs-settings-page>")` and `!body.contains("/static/js/pages/settings.js")` — those negative assertions are fine.

- [ ] **Step 10: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/settings.html src/handlers/pages.rs src/lib.rs src/handlers/user.rs src/handlers/static_assets.rs tests/pages_test.rs
  # `git rm static/js/pages/settings.js` from Step 6 already staged.
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /settings — drop CSR element + JSON endpoint

  /settings now renders the server-config table directly from
  state.config in templates/settings.html. The <rdrs-settings-page>
  custom element, static/js/pages/settings.js, and the
  GET /api/server-config JSON endpoint are deleted; SettingsTemplate
  carries user_agent / user_agent_is_default / signup_enabled /
  multi_user_enabled as Askama fields. First per-route content
  migration of the SSR-first redesign — no JS executes for this page.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Wrap-up

- [ ] **Final sweep.**

  Run: `cargo nextest run && cargo fmt --check`
  Expected: tests green, formatting clean.

- [ ] **Push branch.**

  Run: `git push -u origin feat/ssr-settings-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-3 — /settings page + housekeeping" --body "$(cat <<'EOF'
  ## Summary

  First per-route content migration of the SSR-first redesign. `/settings` now renders directly from `state.config` in `templates/settings.html`; the `<rdrs-settings-page>` custom element, `static/js/pages/settings.js`, and the `GET /api/server-config` JSON endpoint are all deleted. No JS executes for this page.

  Bundled housekeeping: `toggleSidebar()` / `closeSidebar()` move from `templates/base.html` (where they were dead code for login/register) into `static/js/app.js` as `window.*` globals matching the `window.rdrsNavigate` pattern. `base.html` is now genuinely minimal.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] `tests/pages_test.rs::test_settings_page_renders_ssr_content` — confirms `/settings` HTML contains the SSR config table and does NOT contain `<rdrs-settings-page>` or the page module path.
  - [x] `tests/pages_test.rs::test_settings_page_reflects_custom_config` — non-default `Config` values render correctly into the SSR template.
  - [x] Existing `test_logged_in_page_loads_full_chrome` still passes — the chrome contract is unchanged.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr3-settings-page.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-4 is `/user-settings` SSR (preserves passkey JS as the only exception).
