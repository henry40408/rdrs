# SSR-first PR-4: /user-settings Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/user-settings` from CSR shell + JSON API to SSR + form-action endpoints. Account info / GReader URLs render directly. Password / preferences / Linkding / Kagi are HTML forms POSTing to dedicated endpoints that redirect with flash. Passkey UI gets extracted into a standalone `<rdrs-passkeys>` custom element module — WebAuthn requires JS, this is the planned exception.

**Architecture:** Three commits. T1 adds the four form-action POST endpoints server-side (with their own integration tests); template still mounts the CSR `<rdrs-user-settings-page>` so the page keeps working. T2 ships `static/js/passkey.js` as a standalone `<rdrs-passkeys>` custom element. T3 swaps the page over: SSR template with forms + `<rdrs-passkeys>` mount, expanded `UserSettingsTemplate`, deletion of `static/js/pages/user-settings.js`, and removal of the 6 now-unused CSR-only JSON endpoints (`PUT /api/user/password`, `PUT /api/user/settings`, `GET/PUT /api/user/settings/linkding`, `GET/PUT /api/user/settings/kagi`).

**Endpoints kept** (still consumed by other pages or chrome): `/api/me`, `/api/user-settings`, `GET/PUT /api/user/settings/theme`, all `/api/passkey*` and `/api/passkeys*`.

**Tech Stack:** Rust + Axum + Askama + vanilla JS. `axum::extract::Form` for url-encoded form bodies; existing `FlashRedirect` helper for redirect-with-flash responses.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-user-settings-page` (already created off updated `main` at commit `7165d45`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-user-settings-page`, working tree clean, latest commit on main is `7165d45 feat(ssr): SSR-first PR-3 — /settings page + housekeeping (#188)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: 703/703 pass.

---

## Task 1: Add 4 form-action POST endpoints

Adds dedicated endpoints that accept `application/x-www-form-urlencoded` bodies and return a `FlashRedirect` (`303 See Other` + flash cookie + `Location: /user-settings`). Each endpoint mirrors the existing PUT JSON handler's logic but uses `Form` extraction and produces redirect responses suitable for plain HTML form submission.

These endpoints land in this task. Existing PUT JSON endpoints stay in place — they're deleted in T3 once the SSR template no longer references them.

**Files:**
- Modify: `src/handlers/user.rs` — add 4 new handlers + their `Form` request structs.
- Modify: `src/lib.rs` — register the 4 new POST routes.
- Modify: `tests/handlers_test.rs` (or add `tests/user_settings_form_test.rs`) — integration tests for the 4 endpoints.

**Endpoints to add:**

| Method | Path | Handler | Request body | Success behavior | Error behavior |
|--------|------|---------|--------------|------------------|----------------|
| POST | `/user-settings/password` | `change_password_form` | `current_password`, `new_password`, `confirm_password` | Logout + redirect to `/login` with success flash | Redirect to `/user-settings` with error flash |
| POST | `/user-settings/preferences` | `update_preferences_form` | `theme` (`system`/`light`/`dark`), `entries_per_page` (i64) | Redirect to `/user-settings` with success flash | Redirect to `/user-settings` with error flash |
| POST | `/user-settings/linkding` | `update_linkding_form` | `api_url` (Option<String>), `api_token` (Option<String>), `_clear` (Option<String> — present means clear) | Redirect to `/user-settings` with success flash | Redirect to `/user-settings` with error flash |
| POST | `/user-settings/kagi` | `update_kagi_form` | `session_link` (Option<String>), `language` (Option<String>), `_clear` (Option<String>) | Redirect to `/user-settings` with success flash | Redirect to `/user-settings` with error flash |

The `_clear` hidden field is set by a "Clear" button in the form (rendered as a separate `<form method="post"><input type="hidden" name="_clear" value="1"></form>` or as a button with that hidden input). Concrete template details land in T3.

- [ ] **Step 1: Add 4 form-action handlers in `src/handlers/user.rs`.**

  At the bottom of the file (or in a logical spot near the existing `change_password` / `update_settings` handlers), add:

  ```rust
  // ============================================================================
  // Form-action handlers for the SSR /user-settings page (PR-4).
  // Each accepts application/x-www-form-urlencoded bodies and returns a
  // FlashRedirect — i.e. 303 See Other + flash cookie + Location header.
  // The existing JSON PUT endpoints continue to work alongside these until
  // PR-4 Task 3 deletes them.
  // ============================================================================

  use crate::middleware::flash::FlashRedirect;
  use axum::extract::Form;

  #[derive(Debug, Deserialize)]
  pub struct ChangePasswordForm {
      pub current_password: String,
      pub new_password: String,
      pub confirm_password: String,
  }

  pub async fn change_password_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Form(req): Form<ChangePasswordForm>,
  ) -> impl IntoResponse {
      if req.new_password != req.confirm_password {
          return FlashRedirect::error("/user-settings", "New passwords do not match").into_response();
      }
      if req.new_password.len() < 6 {
          return FlashRedirect::error(
              "/user-settings",
              "New password must be at least 6 characters",
          )
          .into_response();
      }
      if !verify_password(&req.current_password, &auth_user.user.password_hash) {
          return FlashRedirect::error("/user-settings", "Current password is incorrect")
              .into_response();
      }

      let new_hash = match hash_password(&req.new_password) {
          Ok(h) => h,
          Err(_) => {
              return FlashRedirect::error("/user-settings", "Failed to hash password")
                  .into_response();
          }
      };
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| {
              user::update_password(conn, user_id, &new_hash)?;
              session::delete_user_sessions(conn, user_id)?;
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success(
              "/login",
              "Password changed successfully. Please login with your new password.",
          )
          .into_response(),
          _ => FlashRedirect::error("/user-settings", "Failed to update password").into_response(),
      }
  }

  #[derive(Debug, Deserialize)]
  pub struct UpdatePreferencesForm {
      pub theme: Option<String>,
      pub entries_per_page: i64,
  }

  pub async fn update_preferences_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Form(req): Form<UpdatePreferencesForm>,
  ) -> impl IntoResponse {
      if !(10..=100).contains(&req.entries_per_page) {
          return FlashRedirect::error(
              "/user-settings",
              "Entries per page must be between 10 and 100",
          )
          .into_response();
      }
      let user_id = auth_user.user.id;
      let epp = req.entries_per_page;
      // Normalize theme: empty / "system" / unknown → None
      let theme_value = match req.theme.as_deref() {
          Some("light") => Some("light".to_string()),
          Some("dark") => Some("dark".to_string()),
          _ => None,
      };

      let result = state
          .db
          .user(move |conn| {
              user_settings::upsert(conn, user_id, epp)?;
              user_settings::set_theme(conn, user_id, theme_value.as_deref())?;
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => {
              FlashRedirect::success("/user-settings", "Preferences saved successfully.")
                  .into_response()
          }
          _ => FlashRedirect::error("/user-settings", "Failed to save preferences").into_response(),
      }
  }

  #[derive(Debug, Deserialize)]
  pub struct UpdateLinkdingForm {
      pub api_url: Option<String>,
      pub api_token: Option<String>,
      #[serde(rename = "_clear")]
      pub clear: Option<String>,
  }

  pub async fn update_linkding_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Form(req): Form<UpdateLinkdingForm>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      let clear = req.clear.is_some();
      let api_url = req.api_url.filter(|s| !s.is_empty());
      let api_token = req.api_token.filter(|s| !s.is_empty());

      let result = state
          .db
          .user(move |conn| {
              let mut config = user_settings::get_save_services_config(conn, user_id)?;
              if clear {
                  config.linkding = None;
              } else if api_url.is_some() || api_token.is_some() {
                  let existing = config.linkding.clone();
                  let final_url = api_url.or_else(|| existing.as_ref().map(|c| c.api_url.clone()));
                  let final_token =
                      api_token.or_else(|| existing.as_ref().map(|c| c.api_token.clone()));
                  if let (Some(u), Some(t)) = (final_url, final_token) {
                      config.linkding = Some(crate::services::LinkdingConfig {
                          api_url: u,
                          api_token: t,
                      });
                  }
              }
              user_settings::set_save_services_config(conn, user_id, &config)?;
              Ok::<_, AppError>(clear)
          })
          .await;
      match result {
          Ok(Ok(true)) => FlashRedirect::success("/user-settings", "Linkding settings cleared.")
              .into_response(),
          Ok(Ok(false)) => {
              FlashRedirect::success("/user-settings", "Linkding settings saved successfully.")
                  .into_response()
          }
          _ => FlashRedirect::error("/user-settings", "Failed to save Linkding settings")
              .into_response(),
      }
  }

  #[derive(Debug, Deserialize)]
  pub struct UpdateKagiForm {
      pub session_link: Option<String>,
      pub language: Option<String>,
      #[serde(rename = "_clear")]
      pub clear: Option<String>,
  }

  pub async fn update_kagi_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Form(req): Form<UpdateKagiForm>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      let clear = req.clear.is_some();
      let session_link = req.session_link.filter(|s| !s.is_empty());
      let language = req.language.filter(|s| !s.is_empty());

      let result = state
          .db
          .user(move |conn| {
              let mut kagi = user_settings::get_kagi_config(conn, user_id)?;
              if clear {
                  kagi = crate::services::summarize::KagiConfig::default();
              } else {
                  if let Some(link) = session_link {
                      kagi.session_link = Some(link);
                  }
                  kagi.language = language;
              }
              user_settings::set_kagi_config(conn, user_id, &kagi)?;
              Ok::<_, AppError>(clear)
          })
          .await;
      match result {
          Ok(Ok(true)) => FlashRedirect::success("/user-settings", "Kagi settings cleared.")
              .into_response(),
          Ok(Ok(false)) => {
              FlashRedirect::success("/user-settings", "Kagi settings saved successfully.")
                  .into_response()
          }
          _ => FlashRedirect::error("/user-settings", "Failed to save Kagi settings")
              .into_response(),
      }
  }
  ```

  **VERIFY** before writing: open `src/models/user_settings.rs` and confirm the existence + signatures of `set_theme`, `set_save_services_config`, `get_kagi_config`, `set_kagi_config`. Also confirm `crate::services::LinkdingConfig` and `crate::services::summarize::KagiConfig` exist with the expected fields. If any of these are named differently, ADAPT the handlers accordingly — keep the same overall logic but use the correct names. Do NOT invent functions.

  If `user_settings::set_save_services_config` doesn't exist but `set_save_services` does, use that. The current `update_linkding_settings` (around line 282 of `user.rs`) is the source of truth for the correct names — copy from there.

  If after reading `update_linkding_settings` and `update_kagi_settings` you find the existing field/function names don't quite match, simplify the new handlers to mirror the existing ones — call the same DB-level functions, just with `Form` instead of `Json` and producing `FlashRedirect` instead of `Json`.

- [ ] **Step 2: Register the 4 routes in `src/lib.rs`.**

  Add these 4 lines (after the existing `/api/user/...` routes, before the `/api/admin/...` routes is a good spot):

  ```rust
          .route("/user-settings/password", post(handlers::user::change_password_form))
          .route("/user-settings/preferences", post(handlers::user::update_preferences_form))
          .route("/user-settings/linkding", post(handlers::user::update_linkding_form))
          .route("/user-settings/kagi", post(handlers::user::update_kagi_form))
  ```

- [ ] **Step 3: Add integration tests in `tests/handlers_test.rs`.**

  Add a section at the end of the file with these tests. Each authenticates via `setup_users` + `login`, POSTs a form body, and asserts a 303 + flash cookie + Location header.

  ```rust
  // ============================================================================
  // /user-settings/* form-action endpoints (PR-4)
  // ============================================================================

  #[tokio::test]
  async fn test_change_password_form_success() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/password")
          .form(&serde_json::json!({
              "current_password": "admin",
              "new_password": "newpass123",
              "confirm_password": "newpass123",
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      let location = response.headers().get(http::header::LOCATION).unwrap();
      assert_eq!(location.to_str().unwrap(), "/login");
  }

  #[tokio::test]
  async fn test_change_password_form_mismatch() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/password")
          .form(&serde_json::json!({
              "current_password": "admin",
              "new_password": "newpass123",
              "confirm_password": "different",
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      assert_eq!(
          response.headers().get(http::header::LOCATION).unwrap().to_str().unwrap(),
          "/user-settings"
      );
  }

  #[tokio::test]
  async fn test_update_preferences_form() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/preferences")
          .form(&serde_json::json!({
              "theme": "dark",
              "entries_per_page": 50,
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      assert_eq!(
          response.headers().get(http::header::LOCATION).unwrap().to_str().unwrap(),
          "/user-settings"
      );
  }

  #[tokio::test]
  async fn test_update_preferences_form_validation() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/preferences")
          .form(&serde_json::json!({
              "theme": "system",
              "entries_per_page": 5,  // out of range
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      assert_eq!(
          response.headers().get(http::header::LOCATION).unwrap().to_str().unwrap(),
          "/user-settings"
      );
  }

  #[tokio::test]
  async fn test_update_linkding_form() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/linkding")
          .form(&serde_json::json!({
              "api_url": "https://linkding.example.com",
              "api_token": "tok123",
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      assert_eq!(
          response.headers().get(http::header::LOCATION).unwrap().to_str().unwrap(),
          "/user-settings"
      );
  }

  #[tokio::test]
  async fn test_update_kagi_form() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app
          .server
          .post("/user-settings/kagi")
          .form(&serde_json::json!({
              "session_link": "https://kagi.com/summarizer/?token=abc",
              "language": "EN",
          }))
          .await;

      response.assert_status(http::StatusCode::SEE_OTHER);
      assert_eq!(
          response.headers().get(http::header::LOCATION).unwrap().to_str().unwrap(),
          "/user-settings"
      );
  }
  ```

  Note: `axum_test::TestServer::post(...).form(&...)` accepts any `Serialize`. `serde_json::json!` gives us a value with field names matching the Rust `Form` struct fields.

- [ ] **Step 4: Compile + test.**

  Run: `cargo nextest run`
  Expected: 703 baseline + 6 new tests = 709 pass.

  Common failures:
  - `set_save_services_config` / `get_kagi_config` etc. names don't match — fix per the VERIFY note in Step 1 (consult the existing PUT handlers).
  - Form-encoded login flows: `app.server.post(url).form(&body)` syntax requires `axum-test` to support form encoding — it does in v20.

- [ ] **Step 5: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add src/handlers/user.rs src/lib.rs tests/handlers_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): add /user-settings/* form-action endpoints

  Four new POST endpoints accept application/x-www-form-urlencoded
  bodies and return FlashRedirect responses (303 + flash cookie +
  Location). They mirror the existing PUT JSON handlers but are
  designed for plain HTML form submission. Existing PUT endpoints
  stay alive — they're removed in PR-4 Task 3 once the SSR template
  no longer references them.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: Extract `passkey.js` as a standalone `<rdrs-passkeys>` custom element

`user-settings.js` currently has ~150 lines of WebAuthn / passkey CRUD logic. Extract that logic into `static/js/passkey.js` as a `<rdrs-passkeys>` custom element. The SSR template (T3) mounts `<rdrs-passkeys></rdrs-passkeys>` and the element handles its own lifecycle: load list, register, rename, delete.

**Files:**
- Create: `static/js/passkey.js`
- Modify: `src/handlers/static_assets.rs` — register `js/passkey.js` in the FILES allowlist.

- [ ] **Step 1: Create `static/js/passkey.js`.**

  ```js
  // static/js/passkey.js — <rdrs-passkeys> custom element.
  //
  // Self-contained WebAuthn UI: lists registered passkeys, registers
  // new ones, supports rename + delete. The SSR /user-settings page
  // mounts <rdrs-passkeys></rdrs-passkeys> in the passkey section;
  // this element handles all the in-page UX while the underlying
  // /api/passkey* and /api/passkeys/* JSON endpoints remain in place
  // (WebAuthn requires JS, this is the planned exception).

  import { escapeHtml } from '/static/js/utils.js';

  function base64urlToBuffer(base64url) {
      const padding = '='.repeat((4 - base64url.length % 4) % 4);
      const base64 = base64url.replace(/-/g, '+').replace(/_/g, '/') + padding;
      const binary = atob(base64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      return bytes.buffer;
  }

  function bufferToBase64url(buffer) {
      const bytes = new Uint8Array(buffer);
      let binary = '';
      for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
      return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
  }

  class RdrsPasskeys extends HTMLElement {
      connectedCallback() {
          if (window.PublicKeyCredential === undefined) {
              this.innerHTML = '<p class="error">Your browser does not support passkeys.</p>';
              return;
          }
          this.innerHTML = `
              <div id="passkey-error" class="error" style="display:none"></div>
              <h3>Registered Passkeys</h3>
              <div id="passkeys-list"><p class="muted">Loading...</p></div>
              <h3>Register New Passkey</h3>
              <form id="register-passkey-form">
                  <div class="form-group">
                      <label for="passkey-name">Passkey Name</label>
                      <input type="text" id="passkey-name" required placeholder="e.g., MacBook Touch ID">
                  </div>
                  <button type="submit" id="register-passkey-btn">Register Passkey</button>
              </form>
          `;
          this.querySelector('#register-passkey-form').addEventListener('submit', (e) => this._onRegister(e));
          this._loadList();
      }

      async _loadList() {
          const list = this.querySelector('#passkeys-list');
          try {
              const r = await fetch('/api/passkeys', { credentials: 'same-origin' });
              if (!r.ok) throw new Error();
              const data = await r.json();
              if (data.passkeys.length === 0) {
                  list.innerHTML = '<p class="muted">No passkeys registered yet.</p>';
                  return;
              }
              list.innerHTML = '<table><thead><tr><th>Name</th><th>Created</th><th>Last Used</th><th>Actions</th></tr></thead><tbody>'
                  + data.passkeys.map(p => `
                  <tr id="passkey-row-${p.id}">
                      <td><span id="passkey-name-${p.id}">${escapeHtml(p.name)}</span></td>
                      <td>${escapeHtml(p.created_at)}</td>
                      <td>${escapeHtml(p.last_used_at || 'Never')}</td>
                      <td class="actions">
                          <a href="#" data-passkey-action="rename" data-passkey-id="${p.id}">Rename</a>
                          <a href="#" data-passkey-action="delete" data-passkey-id="${p.id}">Delete</a>
                      </td>
                  </tr>`).join('')
                  + '</tbody></table>';
              list.querySelectorAll('[data-passkey-action]').forEach(el => {
                  el.addEventListener('click', (e) => {
                      e.preventDefault();
                      const id = parseInt(el.dataset.passkeyId, 10);
                      if (el.dataset.passkeyAction === 'rename') this._rename(id);
                      else if (el.dataset.passkeyAction === 'delete') this._delete(id);
                  });
              });
          } catch {
              list.innerHTML = '<p class="error">Failed to load passkeys.</p>';
          }
      }

      async _rename(id) {
          const cur = this.querySelector(`#passkey-name-${id}`).textContent;
          const next = prompt('Enter new name:', cur);
          if (!next || next === cur) return;
          try {
              const r = await fetch(`/api/passkeys/${id}`, {
                  method: 'PUT',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({ name: next }),
              });
              if (r.ok) {
                  window.flash.success('Passkey renamed successfully.');
                  this._loadList();
              } else {
                  const data = await r.json().catch(() => ({}));
                  window.flash.error(data.error || 'Failed to rename passkey');
              }
          } catch {
              window.flash.error('An error occurred. Please try again.');
          }
      }

      async _delete(id) {
          if (!confirm('Are you sure you want to delete this passkey?')) return;
          try {
              const r = await fetch(`/api/passkeys/${id}`, { method: 'DELETE' });
              if (r.ok) {
                  window.flash.success('Passkey deleted successfully.');
                  this._loadList();
              } else {
                  const data = await r.json().catch(() => ({}));
                  window.flash.error(data.error || 'Failed to delete passkey');
              }
          } catch {
              window.flash.error('An error occurred. Please try again.');
          }
      }

      async _onRegister(e) {
          e.preventDefault();
          const errorDiv = this.querySelector('#passkey-error');
          const btn = this.querySelector('#register-passkey-btn');
          const nameInput = this.querySelector('#passkey-name');
          errorDiv.style.display = 'none';
          const name = nameInput.value.trim();
          if (!name) {
              errorDiv.textContent = 'Passkey name is required';
              errorDiv.style.display = 'block';
              return;
          }
          try {
              btn.disabled = true;
              btn.textContent = 'Registering...';
              const startR = await fetch('/api/passkey/register/start', {
                  method: 'POST', headers: { 'Content-Type': 'application/json' },
              });
              if (!startR.ok) {
                  const data = await startR.json().catch(() => ({}));
                  throw new Error(data.error || 'Failed to start registration');
              }
              const { options } = await startR.json();
              const publicKey = {
                  ...options.publicKey,
                  challenge: base64urlToBuffer(options.publicKey.challenge),
                  user: { ...options.publicKey.user, id: base64urlToBuffer(options.publicKey.user.id) },
                  excludeCredentials: options.publicKey.excludeCredentials?.map(c => ({
                      ...c, id: base64urlToBuffer(c.id),
                  })) || [],
              };
              const credential = await navigator.credentials.create({ publicKey });
              const credForServer = {
                  id: credential.id,
                  rawId: bufferToBase64url(credential.rawId),
                  type: credential.type,
                  response: {
                      attestationObject: bufferToBase64url(credential.response.attestationObject),
                      clientDataJSON: bufferToBase64url(credential.response.clientDataJSON),
                  },
              };
              if (credential.response.getTransports) {
                  credForServer.response.transports = credential.response.getTransports();
              }
              const finishR = await fetch('/api/passkey/register/finish', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({ name, credential: credForServer }),
              });
              if (finishR.ok) {
                  window.flash.success('Passkey registered successfully.');
                  nameInput.value = '';
                  this._loadList();
              } else {
                  const data = await finishR.json().catch(() => ({}));
                  throw new Error(data.error || 'Registration failed');
              }
          } catch (err) {
              if (err.name === 'NotAllowedError') {
                  errorDiv.textContent = 'Registration was cancelled or timed out.';
              } else if (err.name === 'InvalidStateError') {
                  errorDiv.textContent = 'This passkey is already registered.';
              } else {
                  errorDiv.textContent = err.message || 'An error occurred. Please try again.';
              }
              errorDiv.style.display = 'block';
          } finally {
              btn.disabled = false;
              btn.textContent = 'Register Passkey';
          }
      }
  }

  customElements.define('rdrs-passkeys', RdrsPasskeys);
  ```

- [ ] **Step 2: Register `js/passkey.js` in the static-assets allowlist.**

  Edit `src/handlers/static_assets.rs`. Add this entry to the `FILES` array (alphabetically — after `js/keyboard.js`, before `js/router.js` would have been; place it next to the other `js/*.js` top-level entries):

  ```rust
      ("js/passkey.js", include_str!("../../static/js/passkey.js")),
  ```

- [ ] **Step 3: Compile + test.**

  Run: `cargo nextest run`
  Expected: still 709 pass (passkey.js has no Rust-test impact yet — it's static content).

- [ ] **Step 4: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add static/js/passkey.js src/handlers/static_assets.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): extract passkey UI into <rdrs-passkeys> custom element

  Moves the WebAuthn passkey list / register / rename / delete
  logic from user-settings.js into a dedicated static/js/passkey.js
  module. The element auto-mounts when present in the DOM —
  PR-4 Task 3's SSR /user-settings template will include
  <rdrs-passkeys></rdrs-passkeys> in the passkey section. WebAuthn
  is the only piece of /user-settings that needs JS; per spec.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 3: SSR template + handler + delete CSR-only artifacts

Replaces the page module with direct SSR. Uses the form-action endpoints from T1 and the passkey element from T2. Deletes the CSR-only PUT/GET endpoints that are no longer reachable from the page.

**Files:**
- Modify: `templates/user_settings.html` — full SSR replacement
- Modify: `src/handlers/pages.rs` — extend `UserSettingsTemplate`, rewrite `user_settings_page` handler
- Modify: `src/lib.rs` — remove the 6 deleted route registrations
- Modify: `src/handlers/user.rs` — delete the 6 deleted handlers + their request structs
- Delete: `static/js/pages/user-settings.js`
- Modify: `src/handlers/static_assets.rs` — drop the `js/pages/user-settings.js` entry
- Modify: `tests/pages_test.rs` + `tests/handlers_test.rs` + `tests/auth_test.rs` — update / delete obsolete tests

**Endpoints to delete (no consumer remaining after T3):**
- `PUT /api/user/password` → handler `change_password`, struct `ChangePasswordRequest`
- `PUT /api/user/settings` → handler `update_settings`, structs `UpdateSettingsRequest`, `UpdateSettingsResponse`
- `GET /api/user/settings/linkding` → handler `get_linkding_settings`, struct `GetLinkdingResponse` (or whatever the response type is)
- `PUT /api/user/settings/linkding` → handler `update_linkding_settings`, structs `UpdateLinkdingRequest`, `UpdateLinkdingResponse`
- `GET /api/user/settings/kagi` → handler `get_kagi_settings`
- `PUT /api/user/settings/kagi` → handler `update_kagi_settings`, struct `UpdateKagiRequest`

**Endpoints to KEEP** (still consumed elsewhere):
- `/api/me` (admin.js, entries.js)
- `/api/user-settings` (entries.js)
- `GET/PUT /api/user/settings/theme` (base.html theme controller)
- All `/api/passkey*` and `/api/passkeys*` (passkey.js)

### Steps

- [ ] **Step 1: Rewrite `templates/user_settings.html`.**

  Replace the file's contents entirely:

  ```html
  {% extends "app_layout.html" %}

  {% block page_script %}
      <script type="module" src="/static/js/passkey.js?v={{ layout.git_version }}"></script>
  {% endblock %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="user-settings"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>User Settings</h1>

                  <h2>Account Information</h2>
                  <table>
                      <tr><th class="settings-th">Username</th><td>{{ username }}</td></tr>
                      <tr><th class="settings-th">Role</th><td>{{ role }}</td></tr>
                      <tr><th class="settings-th">Registered</th><td>{{ created_at }}</td></tr>
                      <tr><th class="settings-th">Logged In</th><td>{{ session_created_at }}</td></tr>
                  </table>

                  <h2>RSS Client (Google Reader API)</h2>
                  <p class="muted">Connect any RSS reader that supports the Google Reader API (e.g., Reeder, NetNewsWire, FeedMe, Read You, FreshRSS).</p>
                  <table>
                      <tr><th class="settings-th">Server URL</th><td><code>{{ public_base_url }}</code></td></tr>
                      <tr><th class="settings-th">Username</th><td><code>{{ username }}</code></td></tr>
                      <tr><th class="settings-th">Password</th><td class="muted">Your RDRS password</td></tr>
                  </table>
                  <p class="muted">
                      In your RSS client, choose "Google Reader" or "FreshRSS" as the account type and enter the server URL above with your credentials.
                      Some clients may require the FreshRSS-compatible URL: <code>{{ public_base_url }}/api/greader.php</code>
                  </p>

                  <hr>

                  <h2>Change Password</h2>
                  <form method="post" action="/user-settings/password">
                      <div class="form-group">
                          <label for="current-password">Current Password</label>
                          <input type="password" id="current-password" name="current_password" required autocomplete="current-password">
                      </div>
                      <div class="form-group">
                          <label for="new-password">New Password</label>
                          <input type="password" id="new-password" name="new_password" required minlength="6" autocomplete="new-password">
                      </div>
                      <div class="form-group">
                          <label for="confirm-password">Confirm New Password</label>
                          <input type="password" id="confirm-password" name="confirm_password" required minlength="6" autocomplete="new-password">
                      </div>
                      <button type="submit">Change Password</button>
                  </form>

                  <hr>

                  <h2>Passkeys</h2>
                  <p class="muted">Passkeys let you sign in without a password using your device's biometrics or security key.</p>
                  <rdrs-passkeys></rdrs-passkeys>

                  <hr>

                  <h2>Display Preferences</h2>
                  <form method="post" action="/user-settings/preferences">
                      <div class="form-group">
                          <label for="theme-select">Theme</label>
                          <select id="theme-select" name="theme" data-testid="theme-select">
                              <option value="system"{% if theme.is_none() %} selected{% endif %}>System (auto)</option>
                              <option value="light"{% if let Some(t) = theme %}{% if t == "light" %} selected{% endif %}{% endif %}>Light</option>
                              <option value="dark"{% if let Some(t) = theme %}{% if t == "dark" %} selected{% endif %}{% endif %}>Dark</option>
                          </select>
                      </div>
                      <div class="form-group">
                          <label for="entries-per-page">Entries per page</label>
                          <input type="number" id="entries-per-page" name="entries_per_page" value="{{ entries_per_page }}" min="10" max="100" required>
                          <span class="muted" style="font-size:var(--font-xs);">(10-100)</span>
                      </div>
                      <button type="submit">Save Preferences</button>
                  </form>

                  <hr>

                  <h2>Integrations</h2>
                  <p class="muted">Connect external services to save articles.</p>

                  <h3>Linkding</h3>
                  <p class="muted">
                      <a href="https://github.com/sissbruecker/linkding" target="_blank" rel="noopener noreferrer">Linkding</a>
                      is a self-hosted bookmark manager.
                      {% if linkding_configured %}<span class="success-text">Configured</span>{% endif %}
                  </p>
                  <form method="post" action="/user-settings/linkding">
                      <div class="form-group">
                          <label for="linkding-api-url">API URL</label>
                          <input type="url" id="linkding-api-url" name="api_url" value="{{ linkding_api_url }}" placeholder="https://linkding.example.com">
                      </div>
                      <div class="form-group">
                          <label for="linkding-api-token">API Token</label>
                          <input type="password" id="linkding-api-token" name="api_token" placeholder="{% if linkding_configured %}(unchanged){% else %}Enter your API token{% endif %}">
                      </div>
                      <button type="submit">Save Linkding Settings</button>
                  </form>
                  {% if linkding_configured %}
                  <form method="post" action="/user-settings/linkding" style="display:inline-block;margin-top:0.5rem;">
                      <input type="hidden" name="_clear" value="1">
                      <button type="submit" class="btn-secondary" onclick="return confirm('Clear Linkding settings?')">Clear Linkding</button>
                  </form>
                  {% endif %}

                  <h3>Kagi Universal Summarizer</h3>
                  <p class="muted">
                      <a href="https://kagi.com/summarizer" target="_blank" rel="noopener noreferrer">Kagi Universal Summarizer</a>
                      provides AI-powered article summaries.
                      {% if kagi_configured %}<span class="success-text">Configured</span>{% endif %}
                  </p>
                  <form method="post" action="/user-settings/kagi">
                      <div class="form-group">
                          <label for="kagi-session-link">Session Link</label>
                          <input type="text" id="kagi-session-link" name="session_link" placeholder="{% if kagi_configured %}(unchanged){% else %}Paste your session link{% endif %}">
                      </div>
                      <div class="form-group">
                          <label for="kagi-language">Target Language</label>
                          <select id="kagi-language" name="language">
                              <option value=""{% if kagi_language.is_none() %} selected{% endif %}>Auto-detect</option>
                              <option value="EN"{% if let Some(l) = kagi_language %}{% if l == "EN" %} selected{% endif %}{% endif %}>English</option>
                              <option value="ZH-HANT"{% if let Some(l) = kagi_language %}{% if l == "ZH-HANT" %} selected{% endif %}{% endif %}>繁體中文</option>
                              <option value="ZH-CN"{% if let Some(l) = kagi_language %}{% if l == "ZH-CN" %} selected{% endif %}{% endif %}>简体中文</option>
                              <option value="JA"{% if let Some(l) = kagi_language %}{% if l == "JA" %} selected{% endif %}{% endif %}>日本語</option>
                              <option value="KO"{% if let Some(l) = kagi_language %}{% if l == "KO" %} selected{% endif %}{% endif %}>한국어</option>
                              <option value="DE"{% if let Some(l) = kagi_language %}{% if l == "DE" %} selected{% endif %}{% endif %}>Deutsch</option>
                              <option value="FR"{% if let Some(l) = kagi_language %}{% if l == "FR" %} selected{% endif %}{% endif %}>Français</option>
                              <option value="ES"{% if let Some(l) = kagi_language %}{% if l == "ES" %} selected{% endif %}{% endif %}>Español</option>
                              <option value="PT"{% if let Some(l) = kagi_language %}{% if l == "PT" %} selected{% endif %}{% endif %}>Português</option>
                          </select>
                      </div>
                      <button type="submit">Save Kagi Settings</button>
                  </form>
                  {% if kagi_configured %}
                  <form method="post" action="/user-settings/kagi" style="display:inline-block;margin-top:0.5rem;">
                      <input type="hidden" name="_clear" value="1">
                      <button type="submit" class="btn-secondary" onclick="return confirm('Clear Kagi settings?')">Clear Kagi</button>
                  </form>
                  {% endif %}
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  **Notes:**
  - The single inline `onclick="return confirm('...')"` on each Clear button is an inline browser API call (no JS module). This is the minimum-viable confirmation; an alternative is to drop the confirm and trust the user.
  - Uses Askama field accesses on the new `UserSettingsTemplate` fields populated by the handler in Step 2.
  - `theme` is `Option<String>` rendered with `{% if let Some(t) = theme %}{% if t == "dark" %} selected{% endif %}{% endif %}` — preserves existing UX (no theme = "System (auto)").
  - `kagi_language` likewise.

- [ ] **Step 2: Extend `UserSettingsTemplate` and rewrite `user_settings_page` handler.**

  In `src/handlers/pages.rs`, find `UserSettingsTemplate` (added in PR-2 Task 2). Replace it with:

  ```rust
  #[derive(Template)]
  #[template(path = "user_settings.html")]
  pub struct UserSettingsTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub username: String,
      pub role: String,
      pub created_at: String,
      pub session_created_at: String,
      pub public_base_url: String,
      pub theme: Option<String>,
      pub entries_per_page: i64,
      pub linkding_configured: bool,
      pub linkding_api_url: String,
      pub kagi_configured: bool,
      pub kagi_language: Option<String>,
  }

  impl IntoResponse for UserSettingsTemplate {
      fn into_response(self) -> Response {
          match self.render() {
              Ok(html) => Html(html).into_response(),
              Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
          }
      }
  }
  ```

  Rewrite the `user_settings_page` handler. The implementer SHOULD read the existing `get_me` handler (~line 32 of `user.rs`) and `get_user_settings` handler (~line 85 of `user.rs`) to understand what fields and DB calls produce the same data. Pseudocode:

  ```rust
  pub async fn user_settings_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, UserSettingsTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;

      let user_id = auth_user.user.id;
      // theme + entries_per_page from user_settings table
      let (theme, epp) = state
          .db
          .read_user(move |c| {
              let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
              let s = user_settings::get_or_default(c, user_id).ok();
              let epp = s.map(|x| x.entries_per_page).unwrap_or(20);
              Ok::<_, AppError>((theme, epp))
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or((None, 20));

      // save_services_config (linkding) + kagi_config
      let (linkding, kagi) = state
          .db
          .read_user(move |c| {
              let lk = user_settings::get_save_services_config(c, user_id).unwrap_or_default();
              let kg = user_settings::get_kagi_config(c, user_id).unwrap_or_default();
              Ok::<_, AppError>((lk, kg))
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or_default();

      let public_base_url = state
          .config
          .public_base_url
          .clone()
          .unwrap_or_else(|| format!("http://localhost:{}", state.config.server_port));

      let session_created_at = auth_user
          .session
          .created_at
          .to_rfc3339();
      let created_at = auth_user.user.created_at.to_rfc3339();

      (
          flash,
          UserSettingsTemplate {
              title: "User Settings",
              git_version: crate::GIT_VERSION,
              layout,
              username: auth_user.user.username.clone(),
              role: format!("{:?}", auth_user.user.role).to_lowercase(),
              created_at,
              session_created_at,
              public_base_url,
              theme,
              entries_per_page: epp,
              linkding_configured: linkding.linkding.is_some(),
              linkding_api_url: linkding
                  .linkding
                  .as_ref()
                  .map(|c| c.api_url.clone())
                  .unwrap_or_default(),
              kagi_configured: kagi.session_link.is_some(),
              kagi_language: kagi.language.clone(),
          },
      )
  }
  ```

  **VERIFY** the field names against the existing `user_settings::get_or_default`, `get_save_services_config`, `get_kagi_config`, `KagiConfig`, `LinkdingConfig`, `Session::created_at`, `User::created_at`, `User::role`. Adjust to match. The handler exists primarily to populate the template fields — keep the handler readable and idiomatic, even if the DB layer requires multiple round-trips.

- [ ] **Step 3: Drop the 6 route registrations from `src/lib.rs`.**

  Remove these lines from the router:

  ```rust
          .route("/api/user/password", put(handlers::user::change_password))
          .route("/api/user/settings", put(handlers::user::update_settings))
          .route(
              "/api/user/settings/linkding",
              get(handlers::user::get_linkding_settings),
          )
          .route(
              "/api/user/settings/linkding",
              put(handlers::user::update_linkding_settings),
          )
          .route(
              "/api/user/settings/kagi",
              get(handlers::user::get_kagi_settings),
          )
          .route(
              "/api/user/settings/kagi",
              put(handlers::user::update_kagi_settings),
          )
  ```

  KEEP these (they have other consumers):
  - `/api/user`, `/api/me`, `/api/user-settings`
  - `GET/PUT /api/user/settings/theme`

- [ ] **Step 4: Delete the 6 unused handlers from `src/handlers/user.rs`.**

  Delete (along with their request/response structs and any doc comments above each):
  - `pub async fn change_password` + `ChangePasswordRequest`
  - `pub async fn update_settings` + `UpdateSettingsRequest` + `UpdateSettingsResponse`
  - `pub async fn update_linkding_settings` + `UpdateLinkdingRequest` + `UpdateLinkdingResponse`
  - `pub async fn get_linkding_settings` + its response struct (whatever it's named)
  - `pub async fn update_kagi_settings` + `UpdateKagiRequest` + response struct
  - `pub async fn get_kagi_settings` + response struct

  KEEP `get_me`, `get_user_settings`, `get_current_user`, `get_theme`, `update_theme`, and the new `*_form` handlers from T1.

  Verify no leftover references:
  ```bash
  grep -n "change_password\b\|update_settings\b\|update_linkding_settings\|get_linkding_settings\|update_kagi_settings\|get_kagi_settings" src/
  ```
  Expected: zero matches. (Note: `change_password_form` should still exist — the regex above uses `\b` to avoid matching the `_form` variant.)

- [ ] **Step 5: Drop the `js/pages/user-settings.js` allowlist entry.**

  Edit `src/handlers/static_assets.rs`. Remove:
  ```rust
      (
          "js/pages/user-settings.js",
          include_str!("../../static/js/pages/user-settings.js"),
      ),
  ```

- [ ] **Step 6: Delete `static/js/pages/user-settings.js`.**

  ```bash
  git rm static/js/pages/user-settings.js
  ```

- [ ] **Step 7: Update tests.**

  - In `tests/auth_test.rs`: there are two tests using `.put("/api/user/password")` (around lines 328 and 370). UPDATE them to POST `/user-settings/password` with form bodies, OR DELETE if they duplicate T1's coverage. Default: DELETE both — T1 added `test_change_password_form_success` and `test_change_password_form_mismatch` covering the same surface.

  - In `tests/handlers_test.rs`: tests using `.put("/api/user/settings")`, `.get("/api/user/settings/linkding")`, `.put("/api/user/settings/linkding")`, plus any kagi tests. DELETE these — endpoints are gone.

  - In `tests/pages_test.rs`: locate `test_user_settings_page_serves_csr_shell` (around line 189). Rename to `test_user_settings_page_renders_ssr_content` and replace the body to assert the SSR content + the absence of `<rdrs-user-settings-page>` and `/static/js/pages/user-settings.js`:

    ```rust
    #[tokio::test]
    async fn test_user_settings_page_renders_ssr_content() {
        let app = create_test_app(default_test_config());
        setup_users(&app.db).await;
        login(&app.server, "admin").await;

        let response = app.server.get("/user-settings").await;
        response.assert_status_ok();
        let body = response.text();

        // Old CSR markers gone.
        assert!(!body.contains("<rdrs-user-settings-page>"));
        assert!(!body.contains("/static/js/pages/user-settings.js"));

        // SSR content present.
        assert!(body.contains("<h1>User Settings</h1>"));
        assert!(body.contains("Account Information"));
        assert!(body.contains("<form method=\"post\" action=\"/user-settings/password\">"));
        assert!(body.contains("<form method=\"post\" action=\"/user-settings/preferences\">"));
        assert!(body.contains("<form method=\"post\" action=\"/user-settings/linkding\">"));
        assert!(body.contains("<form method=\"post\" action=\"/user-settings/kagi\">"));
        assert!(body.contains("<rdrs-passkeys>"));
        assert!(body.contains("/static/js/passkey.js"));
    }
    ```

  - Look for `test_user_settings_page_with_flash` (around line 377) and verify it still applies. It probably works unchanged since flash still ships in the SSR template.

- [ ] **Step 8: Compile + test.**

  Run: `cargo nextest run`
  Expected: net delta is roughly 0-2 tests (T1 added 6, T3 deletes/rewrites several). Just confirm zero failures.

  If a template render fails, common causes:
  - Wrong field name (template `{{ kagi_language }}` vs struct `kagi_lang`).
  - Wrong type (`Option<String>` vs `String`).
  - Askama syntax issue with nested `{% if let Some(...) %}`.

- [ ] **Step 9: Verify cleanup completeness.**

  ```bash
  grep -rn "rdrs-user-settings-page\|/api/user/password\|/api/user/settings\b\|/api/user/settings/linkding\|/api/user/settings/kagi\|/static/js/pages/user-settings.js" src/ templates/ static/ tests/
  ```
  Acceptable matches: zero in `src/`, `templates/`, `static/`. In `tests/`, only the new test's negative assertions for `<rdrs-user-settings-page>` and `/static/js/pages/user-settings.js`.

- [ ] **Step 10: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/user_settings.html src/handlers/pages.rs src/handlers/user.rs src/handlers/static_assets.rs src/lib.rs tests/auth_test.rs tests/handlers_test.rs tests/pages_test.rs
  # `git rm static/js/pages/user-settings.js` from Step 6 already staged.
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /user-settings — drop CSR element + JSON PUT endpoints

  /user-settings now renders directly from state.config + DB:
  account info, GReader URLs, password / preferences / linkding /
  kagi forms targeting the new /user-settings/* form-action
  endpoints, and a <rdrs-passkeys> mount for the WebAuthn UI.

  Deletes static/js/pages/user-settings.js, the
  <rdrs-user-settings-page> custom element, and 6 CSR-only JSON
  endpoints (PUT /api/user/password, PUT /api/user/settings,
  GET/PUT /api/user/settings/linkding, GET/PUT /api/user/settings/kagi).
  Endpoints kept (still consumed by other pages): /api/me,
  /api/user-settings, GET/PUT /api/user/settings/theme, all passkey
  endpoints.

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

  Run: `git push -u origin feat/ssr-user-settings-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-4 — /user-settings page" --body "$(cat <<'EOF'
  ## Summary

  Migrates `/user-settings` to SSR + form-action endpoints. Account info / GReader URLs render directly. Password / preferences / Linkding / Kagi are HTML forms POSTing to dedicated `/user-settings/*` endpoints that redirect with flash. WebAuthn passkey UI is extracted to `static/js/passkey.js` as `<rdrs-passkeys>` — the planned JS exception.

  Drops `static/js/pages/user-settings.js`, the `<rdrs-user-settings-page>` element, and 6 CSR-only JSON endpoints (`PUT /api/user/password`, `PUT /api/user/settings`, `GET/PUT /api/user/settings/linkding`, `GET/PUT /api/user/settings/kagi`). Endpoints consumed by other pages — `/api/me`, `/api/user-settings`, `GET/PUT /api/user/settings/theme`, all passkey endpoints — stay.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] 6 new endpoint tests (`tests/handlers_test.rs::test_*_form_*`).
  - [x] `tests/pages_test.rs::test_user_settings_page_renders_ssr_content` — SSR markers present, CSR markers absent, all four form actions reachable.
  - [x] Existing `test_logged_in_page_loads_full_chrome` still passes — chrome contract unchanged.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr4-user-settings.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-5 is `/admin` SSR.
