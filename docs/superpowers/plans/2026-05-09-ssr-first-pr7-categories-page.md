# SSR-first PR-7: /categories Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/categories` from CSR shell (which used the GReader endpoints `/reader/api/0/{tag/list,subscription/list,rename-tag,disable-tag}`) to direct SSR + 3 form-action endpoints under `/categories/*`. The GReader endpoints stay alive — they're used by external clients (FreshRSS, Reeder, etc.). Each row in the SSR table has an inline name input + Save button + Delete button. The "click-to-edit" mode of the CSR page is replaced by always-visible inputs (acceptable UX simplification for an admin page).

**Architecture:** Two commits. T1 adds the 3 form-action endpoints (`POST /categories`, `POST /categories/{id}/rename`, `POST /categories/{id}/delete`). T2 swaps the page to SSR + deletes `static/js/pages/categories.js`. No JSON endpoint deletions in this PR — the page never had a dedicated `/api/categories` endpoint; CSR consumed the public GReader API which stays.

**Tech Stack:** Rust + Axum + Askama. `crate::models::category::*` and `crate::models::feed::*` have all the needed helpers.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-categories-page` (already created off updated `main` at commit `c910343e`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-categories-page`, working tree clean modulo untracked `test-results/`, latest commit on main is `c910343e feat(ssr): SSR-first PR-6 — /statistics page (#191)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: 695/695 pass.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify (T1) | `src/handlers/feeds.rs` OR new `src/handlers/categories.rs` | Add 3 form-action handlers + their request structs. NOTE: choose the file based on where existing category handler logic lives — see Step 1. |
| Modify (T1) | `src/lib.rs` | Register the 3 new POST routes. |
| Modify (T1) | `tests/handlers_test.rs` | Add 4 integration tests (create, rename, delete, validation/duplicate-name). |
| Modify (T2) | `templates/categories.html` | Full SSR replacement — table with inline form per row + create form. |
| Modify (T2) | `src/handlers/pages.rs` | Extend `CategoriesTemplate` with `categories` list (with feed counts), rewrite `categories_page` handler. |
| Delete (T2) | `static/js/pages/categories.js` | Page module gone. |
| Modify (T2) | `src/handlers/static_assets.rs` | Drop `js/pages/categories.js` allowlist entry. |
| Modify (T2) | `tests/pages_test.rs` | Update `test_categories_page_*` to assert SSR content; drop CSR-shell assertions. |

**Endpoints kept** (used by external clients): all `/reader/api/0/*` GReader endpoints. No deletions.

---

## Task 1: Add 3 form-action POST endpoints under `/categories/*`

| Method | Path | Handler | Form fields | Success | Error |
|--------|------|---------|-------------|---------|-------|
| POST | `/categories` | `create_category_form` | `name` | Redirect `/categories` with success | Redirect `/categories` with error |
| POST | `/categories/{id}/rename` | `rename_category_form` | `name` | Redirect `/categories` with success | Redirect `/categories` with error |
| POST | `/categories/{id}/delete` | `delete_category_form` | (no body) | Redirect `/categories` with success | Redirect `/categories` with error |

**Where to put the handlers:** read `src/handlers/feed.rs` and `src/handlers/feeds.rs` to see the project layout. There's no existing `src/handlers/categories.rs`. Two options:

A. Create a new `src/handlers/categories.rs` module (cleanest separation).
B. Add the handlers to `src/handlers/feeds.rs` (since feeds + categories share concerns).

**Decision: A — create a new `categories.rs`.** It keeps category-specific logic in one place and makes T2's deletion of unused module-level imports easier.

### Steps

- [ ] **Step 1: Create `src/handlers/categories.rs`.**

  Create the file with the 3 handlers + their request structs. Read `src/models/category.rs` for the helper signatures: `create_category(conn, user_id, name)`, `update_name(conn, id, user_id, new_name)`, `delete_category(conn, id, user_id)`, `find_by_id_and_user(conn, id, user_id)`.

  ```rust
  use axum::{
      extract::{Form, Path, State},
      response::IntoResponse,
  };
  use serde::Deserialize;

  use crate::error::AppError;
  use crate::middleware::auth::AuthUser;
  use crate::middleware::flash::FlashRedirect;
  use crate::models::category;
  use crate::AppState;

  #[derive(Debug, Deserialize)]
  pub struct CategoryNameForm {
      pub name: String,
  }

  pub async fn create_category_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Form(req): Form<CategoryNameForm>,
  ) -> impl IntoResponse {
      let name = req.name.trim().to_string();
      if name.is_empty() {
          return FlashRedirect::error("/categories", "Category name cannot be empty").into_response();
      }
      if name.len() > 100 {
          return FlashRedirect::error("/categories", "Category name is too long (max 100)")
              .into_response();
      }
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| category::create_category(conn, user_id, &name))
          .await;
      match result {
          Ok(Ok(_)) => FlashRedirect::success("/categories", "Category created.").into_response(),
          Ok(Err(AppError::Validation(msg))) => {
              FlashRedirect::error("/categories", msg).into_response()
          }
          _ => FlashRedirect::error("/categories", "Failed to create category").into_response(),
      }
  }

  pub async fn rename_category_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Path(id): Path<i64>,
      Form(req): Form<CategoryNameForm>,
  ) -> impl IntoResponse {
      let name = req.name.trim().to_string();
      if name.is_empty() {
          return FlashRedirect::error("/categories", "Category name cannot be empty").into_response();
      }
      if name.len() > 100 {
          return FlashRedirect::error("/categories", "Category name is too long (max 100)")
              .into_response();
      }
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| category::update_name(conn, id, user_id, &name))
          .await;
      match result {
          Ok(Ok(_)) => FlashRedirect::success("/categories", "Category renamed.").into_response(),
          Ok(Err(AppError::Validation(msg))) => {
              FlashRedirect::error("/categories", msg).into_response()
          }
          _ => FlashRedirect::error("/categories", "Failed to rename category").into_response(),
      }
  }

  pub async fn delete_category_form(
      State(state): State<AppState>,
      auth_user: AuthUser,
      Path(id): Path<i64>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| category::delete_category(conn, id, user_id))
          .await;
      match result {
          Ok(Ok(_)) => FlashRedirect::success("/categories", "Category deleted.").into_response(),
          Ok(Err(AppError::Validation(msg))) => {
              FlashRedirect::error("/categories", msg).into_response()
          }
          _ => FlashRedirect::error("/categories", "Failed to delete category").into_response(),
      }
  }
  ```

  **VERIFY before writing:**
  - `category::update_name` signature — currently `update_name(conn, id, user_id, new_name)`. If `category::update_name` returns `AppResult<()>` (not `AppResult<Category>`), adjust the match arm shape.
  - `category::create_category` returns `AppResult<Category>`.
  - `category::delete_category` returns `AppResult<()>`.
  - The error types each function returns — if there are ownership-related variants like `AppError::CategoryNotFound`, those should be matched separately and produce specific messages.

  Read `src/models/category.rs` lines 36-160 for the full signatures and error types.

- [ ] **Step 2: Add `pub mod categories;` to `src/handlers/mod.rs`.**

  Append the line, alphabetically.

- [ ] **Step 3: Register routes in `src/lib.rs`.**

  Add adjacent to the existing logged-in routes (around the `/admin/users/...` block):

  ```rust
          .route("/categories", post(handlers::categories::create_category_form))
          .route("/categories/{id}/rename", post(handlers::categories::rename_category_form))
          .route("/categories/{id}/delete", post(handlers::categories::delete_category_form))
  ```

  **IMPORTANT:** the GET route `/categories` (which goes to `handlers::pages::categories_page`) ALREADY exists. You're adding a POST handler to the SAME path. Axum allows this (different methods on same path). Don't remove the GET route.

- [ ] **Step 4: Add 4 integration tests in `tests/handlers_test.rs`.**

  Append at the end. Use the project's existing `setup_authenticated_user` helper or whatever pattern recent admin form tests used.

  4 tests:
  1. `test_create_category_form_succeeds` — POST with name=`"Tech"` → 303 to `/categories`. Verify category created in DB.
  2. `test_create_category_form_empty_name` — POST with name=`""` → 303 to `/categories` with error flash.
  3. `test_rename_category_form_succeeds` — create category, then POST `/categories/{id}/rename` with name=`"NewName"` → 303 to `/categories`.
  4. `test_delete_category_form_succeeds` — create category, then POST `/categories/{id}/delete` → 303 to `/categories`. Verify category gone from DB.

  Use `app.server.post("/categories").form(&serde_json::json!({"name": "Tech"}))`.

- [ ] **Step 5: Compile + test.**

  Run: `cargo nextest run`
  Expected: 695 baseline + 4 new tests = 699 pass.

- [ ] **Step 6: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add src/handlers/categories.rs src/handlers/mod.rs src/lib.rs tests/handlers_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): add /categories form-action endpoints

  Three new POST endpoints accept application/x-www-form-urlencoded
  bodies and return FlashRedirect responses (303 + flash cookie +
  Location). GReader /reader/api/0/* endpoints stay alive — used by
  external clients.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: SSR /categories template + handler + delete categories.js

### Steps

- [ ] **Step 1: Rewrite `templates/categories.html`.**

  Replace the file's contents:

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="categories"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Categories</h1>

                  <form method="post" action="/categories">
                      <div class="form-group">
                          <label for="name">New Category</label>
                          <input type="text" id="name" name="name" placeholder="Category name" required maxlength="100" data-testid="category-name-input">
                      </div>
                      <button type="submit" data-testid="add-category-btn">Add Category</button>
                  </form>

                  <hr>

                  <table class="mobile-cards">
                      <thead>
                          <tr>
                              <th>Name</th>
                              <th>Feeds</th>
                              <th>Actions</th>
                          </tr>
                      </thead>
                      <tbody data-testid="categories-table">
                          {% for c in categories %}
                          <tr>
                              <td data-label="Name">
                                  <form method="post" action="/categories/{{ c.id }}/rename" style="display:inline">
                                      <input type="text" name="name" value="{{ c.name }}" maxlength="100" required>
                                      <button type="submit" class="link-button">save</button>
                                  </form>
                              </td>
                              <td data-label="Feeds">{{ c.feed_count }}</td>
                              <td class="actions">
                                  <a href="/feeds?category={{ c.id }}">feeds</a>
                                  <a href="/categories/{{ c.id }}/entries">entries</a>
                                  <form method="post" action="/categories/{{ c.id }}/delete" style="display:inline" onsubmit="return confirm('Delete category? This cannot be undone.')">
                                      <button type="submit" class="link-button">delete</button>
                                  </form>
                              </td>
                          </tr>
                          {% endfor %}
                          {% if categories.is_empty() %}
                          <tr><td colspan="3" class="muted">No categories yet.</td></tr>
                          {% endif %}
                      </tbody>
                  </table>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - Inline rename: each row has an always-visible input + Save button. UX simplification vs the old "click rename → edit → save" flow. Acceptable for an admin page.
  - Delete uses inline `onsubmit="return confirm(...)"` — same pattern as PR-4/PR-5.
  - `link-button` CSS class: from PR-5 admin page. If still missing, the buttons just look like default `<button>`. Acceptable.

- [ ] **Step 2: Extend `CategoriesTemplate` and rewrite `categories_page` handler.**

  In `src/handlers/pages.rs`, find `CategoriesTemplate` (added in PR-2). Replace:

  ```rust
  pub struct CategoryRowView {
      pub id: i64,
      pub name: String,
      pub feed_count: i64,
  }

  #[derive(Template)]
  #[template(path = "categories.html")]
  pub struct CategoriesTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub categories: Vec<CategoryRowView>,
  }

  impl IntoResponse for CategoriesTemplate {
      fn into_response(self) -> Response {
          match self.render() {
              Ok(html) => Html(html).into_response(),
              Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
          }
      }
  }
  ```

  Rewrite `categories_page`:

  ```rust
  pub async fn categories_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, CategoriesTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let user_id = auth_user.user.id;

      let categories = state
          .db
          .read_user(move |conn| {
              let cats = crate::models::category::list_by_user(conn, user_id)?;
              let feeds = crate::models::feed::list_by_user(conn, user_id)?;
              let mut counts: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
              for f in &feeds {
                  *counts.entry(f.category_id).or_insert(0) += 1;
              }
              Ok::<_, AppError>(
                  cats.into_iter()
                      .map(|c| CategoryRowView {
                          feed_count: *counts.get(&c.id).unwrap_or(&0),
                          id: c.id,
                          name: c.name,
                      })
                      .collect::<Vec<_>>(),
              )
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or_default();

      (
          flash,
          CategoriesTemplate {
              title: "Categories",
              git_version: crate::GIT_VERSION,
              layout,
              categories,
          },
      )
  }
  ```

  **VERIFY:**
  - `feed::list_by_user` returns `Vec<Feed>` with a `category_id: i64` field on each. Check by reading `src/models/feed.rs` lines 161+.
  - `category::list_by_user(conn, user_id)` returns `AppResult<Vec<Category>>`.

- [ ] **Step 3: Drop `js/pages/categories.js` allowlist entry from `src/handlers/static_assets.rs`.**

  Remove:
  ```rust
      (
          "js/pages/categories.js",
          include_str!("../../static/js/pages/categories.js"),
      ),
  ```

- [ ] **Step 4: Delete `static/js/pages/categories.js`.**

  ```bash
  git rm static/js/pages/categories.js
  ```

- [ ] **Step 5: Update `tests/pages_test.rs`.**

  Find the existing `test_categories_page_*` test(s). Rename / rewrite to assert SSR content. Look for assertions on `<rdrs-categories-page>` or `/static/js/pages/categories.js` and replace with assertions on:
  - `<h1>Categories</h1>`
  - `<form method="post" action="/categories">` (create form)
  - `data-testid="categories-table"`
  - `<form method="post" action="/categories/{id}/rename">` (rename form)
  - When categories exist, the category names appear in the rendered HTML.

- [ ] **Step 6: Compile + test.**

  Run: `cargo nextest run`
  Expected: full suite green.

- [ ] **Step 7: Verify cleanup.**

  ```bash
  grep -rn "rdrs-categories-page\|/static/js/pages/categories.js" src/ templates/ static/ tests/
  ```
  Acceptable: zero in `src/`, `templates/`, `static/`. In `tests/`, only negative assertions in renamed/added tests.

- [ ] **Step 8: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/categories.html src/handlers/pages.rs src/handlers/static_assets.rs tests/pages_test.rs
  # git rm of categories.js already staged.
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /categories — drop CSR element

  /categories now renders the category table directly from the DB
  with inline rename forms per row + a create form at the top + a
  delete form with confirm prompt. Each row has its own POST form
  to /categories/{id}/rename or /categories/{id}/delete.

  Deletes static/js/pages/categories.js and the
  <rdrs-categories-page> custom element. The /reader/api/0/*
  GReader endpoints stay — they're consumed by external clients
  like FreshRSS / Reeder.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Wrap-up

- [ ] **Final sweep.**

  Run: `cargo nextest run && cargo fmt --check`

- [ ] **Push branch.**

  Run: `git push -u origin feat/ssr-categories-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-7 — /categories page" --body "$(cat <<'EOF'
  ## Summary

  Migrates `/categories` to SSR + 3 form-action endpoints under `/categories/*` (create, rename, delete). Each row has an always-visible inline rename form (no more "click-to-edit"). The GReader API (`/reader/api/0/*`) stays alive — external clients use it. The CSR `<rdrs-categories-page>` element and `static/js/pages/categories.js` (233 lines) are deleted.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] 4 new endpoint tests covering create/rename/delete + empty-name validation.
  - [x] Updated `tests/pages_test.rs::test_categories_page_*` to assert SSR content.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr7-categories-page.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-8 is `/feeds` SSR (incl. add/edit/delete/import/export form-ization).
