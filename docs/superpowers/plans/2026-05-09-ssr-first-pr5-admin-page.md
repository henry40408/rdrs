# SSR-first PR-5: /admin Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/admin` from CSR shell + JSON API to SSR + form-action endpoints. The admin user-list table renders directly from the DB. Each row's actions (toggle role, toggle status, masquerade, delete) become HTML forms POSTing to `/admin/users/{id}/{role,status,masquerade,delete}` endpoints that return `FlashRedirect`.

**Architecture:** Two commits. T1 adds 4 form-action POST endpoints under `/admin/users/{id}/*` server-side. T2 swaps the page over: SSR template with the user table + 4 forms per row, expanded `AdminTemplate`, deletion of `static/js/pages/admin.js`, and removal of 4 now-unused CSR-only JSON endpoints (`GET /api/admin/users`, `PUT /api/admin/users/{id}`, `DELETE /api/admin/users/{id}`, `POST /api/admin/masquerade/{id}`).

**Endpoints kept** (still consumed by other pages or chrome): `POST /api/admin/unmasquerade` is consumed by `static/js/components/rdrs-sidebar.js:253` for the "Stop masquerade" button — leave it alone.

**Tech Stack:** Rust + Axum + Askama + vanilla JS — same patterns as PR-3 / PR-4.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-admin-page` (already created off updated `main` at commit `66ad2be`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-admin-page`, working tree clean, latest commit on main is `66ad2be feat(ssr): SSR-first PR-4 — /user-settings page (#189)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: all tests pass (whatever the current count is on main — should be ~699).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify (T1) | `src/handlers/admin.rs` | Add 4 new form-action handlers + their request structs. |
| Modify (T1) | `src/lib.rs` | Register 4 new POST routes. |
| Modify (T1) | `tests/handlers_test.rs` (or `tests/admin_form_test.rs`) | Add integration tests for the 4 endpoints. |
| Modify (T2) | `templates/admin.html` | Full SSR replacement: user-list table with 4 form-per-row actions. |
| Modify (T2) | `src/handlers/pages.rs` | Extend `AdminTemplate` with `users` list field; rewrite `admin_page` handler to load the user list. |
| Modify (T2) | `src/lib.rs` | Drop the 4 deleted route registrations. |
| Modify (T2) | `src/handlers/admin.rs` | Delete the 4 JSON handlers (`list_users`, `update_user`, `delete_user`, `start_masquerade`) + their request structs. KEEP `stop_masquerade`. |
| Delete (T2) | `static/js/pages/admin.js` | Page module gone. |
| Modify (T2) | `src/handlers/static_assets.rs` | Drop `js/pages/admin.js` allowlist entry. |
| Modify (T2) | `tests/auth_test.rs`, `tests/handlers_test.rs`, `tests/pages_test.rs` | Update / delete obsolete tests; replace assertions on CSR shell with assertions on SSR content. |

---

## Task 1: Add 4 form-action POST endpoints

Each endpoint accepts `application/x-www-form-urlencoded` and returns `FlashRedirect("/admin", ...)` on success/error. Logic mirrors the existing `update_user`, `delete_user`, `start_masquerade` JSON handlers. Existing JSON endpoints stay alive in this task — they're deleted in T2.

**Endpoints to add:**

| Method | Path | Handler | Form fields | Success | Error |
|--------|------|---------|-------------|---------|-------|
| POST | `/admin/users/{id}/role` | `update_role_form` | `role` ("admin" or "user") | Redirect `/admin` with flash | Same |
| POST | `/admin/users/{id}/status` | `update_status_form` | `disabled` ("true" or "false") | Redirect `/admin` with flash | Same |
| POST | `/admin/users/{id}/masquerade` | `start_masquerade_form` | (no body — bare submit) | Redirect `/` with info flash | Redirect `/admin` with error |
| POST | `/admin/users/{id}/delete` | `delete_user_form` | (no body — bare submit) | Redirect `/admin` with success flash | Redirect `/admin` with error |

**Self-protection:** Each handler that mutates state checks `user_id == original_admin_id` (or `admin.user.id` if not masquerading) — same guard as the existing JSON handlers — and returns an error flash redirect.

**Files:**
- Modify: `src/handlers/admin.rs`
- Modify: `src/lib.rs`
- Modify: `tests/handlers_test.rs`

### Steps

- [ ] **Step 1: Add 4 form-action handlers in `src/handlers/admin.rs`.**

  Append at the bottom (after `stop_masquerade`):

  ```rust
  // ============================================================================
  // Form-action handlers for the SSR /admin page (PR-5).
  // Each accepts application/x-www-form-urlencoded bodies and returns a
  // FlashRedirect — i.e. 303 See Other + flash cookie + Location header.
  // The existing JSON endpoints continue to work alongside these until
  // PR-5 Task 2 deletes them.
  // ============================================================================

  use crate::middleware::flash::FlashRedirect;
  use axum::extract::Form;
  use axum::response::IntoResponse;

  #[derive(Debug, Deserialize)]
  pub struct UpdateRoleForm {
      pub role: Role,
  }

  pub async fn update_role_form(
      State(state): State<AppState>,
      admin: AdminUser,
      Path(user_id): Path<i64>,
      Form(req): Form<UpdateRoleForm>,
  ) -> impl IntoResponse {
      let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
      if user_id == original_admin_id {
          return FlashRedirect::error("/admin", "Cannot modify your own account").into_response();
      }
      let role = req.role;
      let result = state
          .db
          .user(move |conn| {
              let target = user::find_by_id(conn, user_id)?.ok_or(AppError::UserNotFound)?;
              if target.role != role {
                  user::update_role(conn, user_id, role)?;
              }
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success("/admin", &format!("Role updated to {role}.")).into_response(),
          _ => FlashRedirect::error("/admin", "Failed to update role").into_response(),
      }
  }

  #[derive(Debug, Deserialize)]
  pub struct UpdateStatusForm {
      pub disabled: bool,
  }

  pub async fn update_status_form(
      State(state): State<AppState>,
      admin: AdminUser,
      Path(user_id): Path<i64>,
      Form(req): Form<UpdateStatusForm>,
  ) -> impl IntoResponse {
      let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
      if user_id == original_admin_id {
          return FlashRedirect::error("/admin", "Cannot modify your own account").into_response();
      }
      let disabled = req.disabled;
      let result = state
          .db
          .user(move |conn| {
              let target = user::find_by_id(conn, user_id)?.ok_or(AppError::UserNotFound)?;
              if disabled && !target.is_disabled() {
                  user::disable_user(conn, user_id)?;
                  session::delete_user_sessions(conn, user_id)?;
              } else if !disabled && target.is_disabled() {
                  user::enable_user(conn, user_id)?;
              }
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success(
              "/admin",
              if disabled { "User disabled." } else { "User enabled." },
          )
          .into_response(),
          _ => FlashRedirect::error("/admin", "Failed to update user status").into_response(),
      }
  }

  pub async fn start_masquerade_form(
      State(state): State<AppState>,
      admin: AdminUser,
      Path(target_user_id): Path<i64>,
  ) -> impl IntoResponse {
      if admin.session.is_masquerading() {
          return FlashRedirect::error("/admin", "Already masquerading").into_response();
      }
      let session_token = admin.session.session_token.clone();
      let result = state
          .db
          .user(move |conn| {
              let target =
                  user::find_by_id(conn, target_user_id)?.ok_or(AppError::UserNotFound)?;
              if target.is_disabled() {
                  return Err(AppError::UserDisabled);
              }
              session::start_masquerade(conn, &session_token, target_user_id)?;
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::info("/", "Now viewing as another user.").into_response(),
          _ => FlashRedirect::error("/admin", "Failed to start masquerade").into_response(),
      }
  }

  pub async fn delete_user_form(
      State(state): State<AppState>,
      admin: AdminUser,
      Path(user_id): Path<i64>,
  ) -> impl IntoResponse {
      let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
      if user_id == original_admin_id {
          return FlashRedirect::error("/admin", "Cannot modify your own account").into_response();
      }
      let result = state
          .db
          .user(move |conn| user::delete_user(conn, user_id))
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success("/admin", "User deleted.").into_response(),
          _ => FlashRedirect::error("/admin", "Failed to delete user").into_response(),
      }
  }
  ```

  Note: `format!("{role}")` requires `Role: Display`. If it doesn't implement Display, switch to `format!("{:?}", role)` or call a method like `role.as_str()`. Check by `grep "impl.*Display.*Role" src/`. Adjust the success message construction to compile.

- [ ] **Step 2: Register 4 routes in `src/lib.rs`.**

  Add adjacent to the existing `/api/admin/...` routes:

  ```rust
          .route("/admin/users/{id}/role", post(handlers::admin::update_role_form))
          .route("/admin/users/{id}/status", post(handlers::admin::update_status_form))
          .route("/admin/users/{id}/masquerade", post(handlers::admin::start_masquerade_form))
          .route("/admin/users/{id}/delete", post(handlers::admin::delete_user_form))
  ```

- [ ] **Step 3: Add integration tests in `tests/handlers_test.rs`.**

  Append at the end of the file. 4 happy-path tests + 1 self-protection test (covering the `Cannot modify your own account` guard for at least one mutating endpoint).

  Use `tests/handlers_test.rs`'s existing `setup_authenticated_user` helper if it works for admin scenarios; otherwise look at how existing admin tests in `tests/auth_test.rs` set up an admin user and adapt.

  Each test:
  1. Set up admin + a target user
  2. Login as admin
  3. POST a form to the new endpoint
  4. Assert `303 SEE_OTHER` + correct `Location`

  ```rust
  // ============================================================================
  // /admin/users/{id}/* form-action endpoints (PR-5)
  // ============================================================================

  // (test bodies — adapt setup helpers per the file's conventions)
  ```

  Aim for 5 tests:
  - `test_update_role_form_promotes_user`
  - `test_update_status_form_disables_user`
  - `test_start_masquerade_form_redirects_to_root`
  - `test_delete_user_form_succeeds`
  - `test_update_role_form_self_protection` — admin tries to change their own role → 303 to `/admin` with error flash

- [ ] **Step 4: Compile + test.**

  Run: `cargo nextest run`
  Expected: baseline + 5 = baseline+5 pass.

- [ ] **Step 5: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add src/handlers/admin.rs src/lib.rs tests/handlers_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): add /admin/users/{id}/* form-action endpoints

  Four new POST endpoints accept application/x-www-form-urlencoded
  bodies and return FlashRedirect responses (303 + flash cookie +
  Location). They mirror the existing JSON handlers in admin.rs but
  are designed for plain HTML form submission. Existing JSON
  endpoints stay alive — they're removed in PR-5 Task 2 once the
  SSR template no longer references them.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: SSR /admin template + handler + cleanup

Replaces `<rdrs-admin-page>` with direct SSR. Each row carries 4 form elements pointing at the form-action endpoints from T1. Self-detection happens server-side (compare `user.id` against `admin.user.id` and `admin.session.original_user_id`). Deletes the 4 CSR-only JSON endpoints.

**Files:**
- Modify: `templates/admin.html`
- Modify: `src/handlers/pages.rs` (extend `AdminTemplate`, rewrite `admin_page`)
- Modify: `src/lib.rs` (drop 4 routes)
- Modify: `src/handlers/admin.rs` (delete 4 JSON handlers + structs)
- Delete: `static/js/pages/admin.js`
- Modify: `src/handlers/static_assets.rs` (drop `js/pages/admin.js` entry)
- Modify: tests

### Endpoints to delete (no consumer remaining)

- `GET /api/admin/users` → handler `list_users`
- `PUT /api/admin/users/{id}` → handler `update_user`, struct `UpdateUserRequest`
- `DELETE /api/admin/users/{id}` → handler `delete_user`
- `POST /api/admin/masquerade/{id}` → handler `start_masquerade`

### Endpoint to KEEP

- `POST /api/admin/unmasquerade` → handler `stop_masquerade` (consumed by `static/js/components/rdrs-sidebar.js`).

### Steps

- [ ] **Step 1: Rewrite `templates/admin.html`.**

  Replace the file's contents:

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="admin"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Admin Panel</h1>
                  <table class="mobile-cards">
                      <thead>
                          <tr>
                              <th>ID</th><th>Username</th><th>Role</th><th>Status</th><th>Created</th><th>Actions</th>
                          </tr>
                      </thead>
                      <tbody>
                          {% for u in users %}
                          <tr>
                              <td data-label="ID">{{ u.id }}</td>
                              <td data-label="Username">{{ u.username }}</td>
                              <td data-label="Role">{{ u.role }}</td>
                              <td data-label="Status">
                                  {% if u.disabled %}<span class="error-text">disabled</span>
                                  {% else %}<span class="success-text">active</span>{% endif %}
                              </td>
                              <td data-label="Created">{{ u.created_at }}</td>
                              <td class="actions">
                                  {% if u.is_self %}
                                      <span class="muted">(you)</span>
                                  {% else %}
                                      <form method="post" action="/admin/users/{{ u.id }}/role" style="display:inline">
                                          <input type="hidden" name="role" value="{% if u.role == 'admin' %}user{% else %}admin{% endif %}">
                                          <button type="submit" class="link-button">{% if u.role == 'admin' %}demote{% else %}promote{% endif %}</button>
                                      </form>
                                      <form method="post" action="/admin/users/{{ u.id }}/status" style="display:inline">
                                          <input type="hidden" name="disabled" value="{% if u.disabled %}false{% else %}true{% endif %}">
                                          <button type="submit" class="link-button">{% if u.disabled %}enable{% else %}disable{% endif %}</button>
                                      </form>
                                      <form method="post" action="/admin/users/{{ u.id }}/masquerade" style="display:inline">
                                          <button type="submit" class="link-button">view as</button>
                                      </form>
                                      <form method="post" action="/admin/users/{{ u.id }}/delete" style="display:inline" onsubmit="return confirm('Delete user &quot;{{ u.username }}&quot;? This cannot be undone.')">
                                          <button type="submit" class="link-button">delete</button>
                                      </form>
                                  {% endif %}
                              </td>
                          </tr>
                          {% endfor %}
                          {% if users.is_empty() %}
                          <tr><td colspan="6" class="muted">No users found.</td></tr>
                          {% endif %}
                      </tbody>
                  </table>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - No `{% block page_script %}` — no per-page JS.
  - Forms use `<button type="submit" class="link-button">…</button>` for visual parity with the old `<a>` elements (existing CSS likely already styles `.link-button` to look like a link; if not, the implementer may need to add a small CSS rule). If `.link-button` doesn't exist, use plain `<button>` and accept a slight visual change (acceptable for an admin page).
  - Inline `onsubmit="return confirm(...)"` on the delete form — same minimum-viable pattern used in PR-4's clear buttons.
  - The `{{ u.username }}` in the inline confirm is HTML-escaped by Askama, but inside a JS string literal it could still introduce a quote-injection risk if the username contains a single quote. The `&quot;` workaround in the JS string isn't perfect. Acceptable risk for admin-only UI; a follow-up could move to CSP-friendly `data-confirm` + a tiny inline script.

- [ ] **Step 2: Extend `AdminTemplate` and rewrite `admin_page` handler in `src/handlers/pages.rs`.**

  Replace the existing `AdminTemplate` (added in PR-2 Task 2):

  ```rust
  pub struct AdminUserView {
      pub id: i64,
      pub username: String,
      pub role: String,
      pub disabled: bool,
      pub created_at: String,
      pub is_self: bool,
  }

  #[derive(Template)]
  #[template(path = "admin.html")]
  pub struct AdminTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub users: Vec<AdminUserView>,
  }

  impl IntoResponse for AdminTemplate {
      fn into_response(self) -> Response {
          match self.render() {
              Ok(html) => Html(html).into_response(),
              Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
          }
      }
  }
  ```

  Rewrite the `admin_page` handler:

  ```rust
  pub async fn admin_page(
      admin: PageAdminUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, AdminTemplate) {
      // Existing PageAdminUser → PageAuthUser conversion for build_app_layout.
      let auth_user = PageAuthUser {
          user: admin.user.clone(),
          session: admin.session.clone(),
      };
      let layout = build_app_layout(&state, &auth_user, &flash).await;

      let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
      let effective_admin_id = admin.user.id;

      let users = state
          .db
          .read_user(crate::models::user::list_all)
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or_default()
          .into_iter()
          .map(|u| AdminUserView {
              id: u.id,
              username: u.username,
              role: format!("{:?}", u.role).to_lowercase(),  // or u.role.as_str() — see VERIFY note
              disabled: u.is_disabled(),
              created_at: u.created_at.format("%Y-%m-%d").to_string(),
              is_self: u.id == effective_admin_id || u.id == original_admin_id,
          })
          .collect();

      (
          flash,
          AdminTemplate {
              title: "Admin Panel",
              git_version: crate::GIT_VERSION,
              layout,
              users,
          },
      )
  }
  ```

  **VERIFY:** Whether `u.created_at` is `chrono::DateTime<Utc>` or `Option<chrono::DateTime<Utc>>` — use the same format you find in existing `format!()` of `User.created_at` (e.g. in the existing JSON `list_users` handler the value is serialized via Serde; here we render it as a string). Use `to_string()` if it's a chrono DateTime field directly, or `format!("%Y-%m-%d", u.created_at)` if you can call `.format()`. The plan above uses `.format("%Y-%m-%d").to_string()` which is the chrono idiom.

  **VERIFY:** `u.role` rendering: if `Role: Display` exists, use `u.role.to_string()` instead of `format!("{:?}", u.role).to_lowercase()`. Check by `grep "impl.*Display.*Role" src/`.

  **VERIFY:** Whether `admin.user.clone()` and `admin.session.clone()` work — depends on if `User` and `Session` derive `Clone`. They probably do; if not, use references or refactor.

  Note: `PageAdminUser` may already have a `From<PageAdminUser> for PageAuthUser` impl after a previous reviewer suggestion; if so, use `auth_user = admin.into()` instead of the manual struct literal.

- [ ] **Step 3: Drop 4 route registrations from `src/lib.rs`.**

  Remove:
  ```rust
          .route("/api/admin/users", get(handlers::admin::list_users))
          .route("/api/admin/users/{id}", put(handlers::admin::update_user))
          .route(
              "/api/admin/users/{id}",
              delete(handlers::admin::delete_user),
          )
          .route(
              "/api/admin/masquerade/{id}",
              post(handlers::admin::start_masquerade),
          )
  ```

  KEEP `/api/admin/unmasquerade` POST.

- [ ] **Step 4: Delete 4 unused handlers from `src/handlers/admin.rs`.**

  Delete (along with their request structs and any doc comments):
  - `pub async fn list_users`
  - `pub async fn update_user` + `UpdateUserRequest`
  - `pub async fn delete_user`
  - `pub async fn start_masquerade`

  KEEP `stop_masquerade` (consumed by sidebar) AND the new `*_form` handlers from T1 (`update_role_form`, `update_status_form`, `start_masquerade_form`, `delete_user_form`).

  Verify:
  ```bash
  grep -n "fn list_users\|fn update_user\b\|fn delete_user\b\|fn start_masquerade\b" src/
  ```
  Expected: zero matches. (Note: `delete_user_form` and `start_masquerade_form` should not match — the `\b` boundaries protect the `_form` suffix.)

- [ ] **Step 5: Drop `js/pages/admin.js` allowlist entry from `src/handlers/static_assets.rs`.**

  Remove:
  ```rust
      (
          "js/pages/admin.js",
          include_str!("../../static/js/pages/admin.js"),
      ),
  ```

- [ ] **Step 6: Delete `static/js/pages/admin.js`.**

  ```bash
  git rm static/js/pages/admin.js
  ```

- [ ] **Step 7: Update tests.**

  - In `tests/auth_test.rs`: tests using `.get("/api/admin/users")`, `.put("/api/admin/users/N")`, `.delete("/api/admin/users/N")`. Update or DELETE these. The form-action tests in T1 cover the mutation endpoints. The `GET /api/admin/users` test purpose should be replaced by checking that `GET /admin` HTML contains the expected user-list. Decision: DELETE the API tests, add a `GET /admin` HTML assertion test if not already present.

  - In `tests/handlers_test.rs`: any tests targeting the deleted endpoints — DELETE.

  - In `tests/pages_test.rs`: `test_admin_page_serves_csr_shell` (around line 884) → rename to `test_admin_page_renders_ssr_content`. Also tests at lines 142 / 177 use `.post("/api/admin/masquerade/N")` — these should be updated to use the new form endpoint OR deleted (T1 has equivalent coverage).

  ```rust
  #[tokio::test]
  async fn test_admin_page_renders_ssr_content() {
      let app = create_test_app(default_test_config());
      setup_users(&app.db).await;
      login(&app.server, "admin").await;

      let response = app.server.get("/admin").await;
      response.assert_status_ok();
      let body = response.text();

      // Old CSR markers gone.
      assert!(!body.contains("<rdrs-admin-page>"));
      assert!(!body.contains("/static/js/pages/admin.js"));

      // SSR content present.
      assert!(body.contains("<h1>Admin Panel</h1>"));
      assert!(body.contains("<th>Username</th>"));
      // The admin user themselves shows the (you) marker.
      assert!(body.contains("(you)"));
  }
  ```

- [ ] **Step 8: Compile + test.**

  Run: `cargo nextest run`
  Expected: full suite green.

- [ ] **Step 9: Verify cleanup.**

  ```bash
  grep -rn "rdrs-admin-page\|/api/admin/users\|/api/admin/masquerade\|/static/js/pages/admin.js" src/ templates/ static/ tests/
  ```
  Acceptable: zero in `src/`, `templates/`, `static/`. In `tests/`, only the negative assertions in the new test.

- [ ] **Step 10: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/admin.html src/handlers/pages.rs src/handlers/admin.rs src/handlers/static_assets.rs src/lib.rs tests/auth_test.rs tests/handlers_test.rs tests/pages_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /admin — drop CSR element + JSON endpoints

  /admin now renders the user table directly from the DB. Each row
  has 4 form elements posting to the new /admin/users/{id}/{role,
  status,masquerade,delete} endpoints (FlashRedirect responses).
  Self-detection moves to the handler.

  Deletes static/js/pages/admin.js, the <rdrs-admin-page> custom
  element, and 4 CSR-only JSON endpoints (GET /api/admin/users,
  PUT/DELETE /api/admin/users/{id}, POST /api/admin/masquerade/{id}).
  POST /api/admin/unmasquerade stays — still consumed by sidebar.

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

  Run: `git push -u origin feat/ssr-admin-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-5 — /admin page" --body "$(cat <<'EOF'
  ## Summary

  Migrates `/admin` to SSR + form-action endpoints. The user table renders directly from the DB with self-detection in the handler. Each row's 4 actions (toggle role, toggle status, masquerade, delete) become forms posting to dedicated `/admin/users/{id}/*` endpoints that return `FlashRedirect`.

  Drops `static/js/pages/admin.js`, the `<rdrs-admin-page>` element, and 4 CSR-only JSON endpoints (`GET /api/admin/users`, `PUT/DELETE /api/admin/users/{id}`, `POST /api/admin/masquerade/{id}`). `POST /api/admin/unmasquerade` stays — consumed by the sidebar's "Stop masquerade" button.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] 5 new endpoint tests covering happy path + self-protection guard.
  - [x] `tests/pages_test.rs::test_admin_page_renders_ssr_content` — SSR markers present, CSR markers absent, `(you)` shown for current admin row.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr5-admin-page.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-6 is `/statistics` SSR.
