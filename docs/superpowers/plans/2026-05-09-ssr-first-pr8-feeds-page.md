# SSR-first PR-8: /feeds Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/feeds` from CSR shell (`<rdrs-feeds-page>` + `static/js/pages/feeds.js`, 529 lines) to direct SSR + 6 form-action endpoints under `/feeds/*` (create, edit, delete, refresh, fetch-metadata, OPML import). The list page renders the feed table directly with filter/sort selects and per-row inline `refresh`/`delete` forms; editing a feed lives on a dedicated `/feeds/{id}/edit` SSR page; OPML import lives on a dedicated `/feeds/import` SSR page (multipart upload). The GReader endpoints (`/reader/api/0/subscription/{edit,import,export}`) stay alive — external clients depend on them. The internal JSON endpoints `GET /api/feeds`, `POST /api/feeds/fetch-metadata`, `POST /api/feeds/{id}/refresh` are deleted with their last consumer. `GET /api/feeds/{id}/icon` stays — referenced directly from `<img src="…">`.

**Architecture:** Two commits. T1 adds the 6 form-action endpoints + integration tests. T2 swaps the page to SSR (list + edit + import templates), deletes `static/js/pages/feeds.js`, and removes the now-unused JSON endpoints.

**Tech Stack:** Rust + Axum + Askama. `crate::models::feed::*`, `crate::models::category::*`, `crate::services::feed_discovery`, `crate::services::feed_sync::refresh_feed`, `crate::services::opml::parse_opml` all already exist. Multipart upload via `axum::extract::Multipart` (already a transitive dep via Axum).

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-feeds-page` (already created off updated `main` at commit `c44c98e`).

---

## Pre-flight

- [x] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-feeds-page`, working tree clean modulo untracked `test-results/`, latest commit on main is `c44c98e chore(ci): speed up CI — drop Windows + dedupe test runs + prune e2e (#193)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: 700/700 pass (post-PR-7 baseline confirmed 2026-05-10).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create (T1) | `src/handlers/feeds.rs` (extend) | Add 6 form-action handlers + their request structs alongside the existing `list_feeds` JSON handler. NOTE: T2 deletes `list_feeds`. |
| Modify (T1) | `src/lib.rs` | Register the 6 new routes. |
| Modify (T1) | `tests/handlers_test.rs` | Add 7-9 integration tests (create+errors, edit+category-change, delete, refresh, fetch-metadata validation, import). |
| Modify (T2) | `templates/feeds.html` | Full SSR list page replacement — table with filter/sort + add form + per-row refresh/delete forms. |
| Create (T2) | `templates/feed_edit.html` | New SSR edit page extending `app_layout.html`. |
| Create (T2) | `templates/feeds_import.html` | New SSR OPML import page extending `app_layout.html`. |
| Modify (T2) | `src/handlers/pages.rs` | Extend `FeedsTemplate` (rows + filter context); add `feed_edit_page` + `feeds_import_page` handlers + their templates. |
| Modify (T2) | `src/lib.rs` | Register `GET /feeds/{id}/edit` + `GET /feeds/import`. Remove `GET /api/feeds`, `POST /api/feeds/fetch-metadata`, `POST /api/feeds/{id}/refresh`. |
| Modify (T2) | `src/handlers/feeds.rs` | Delete `list_feeds`, `FeedDto`, `CategoryOptionDto`, `FeedsResponse`. (Keep the 6 form handlers from T1.) |
| Delete (T2) | `src/handlers/feed.rs::fetch_metadata` (and entire file if get_feed_icon is the only survivor) | Move `get_feed_icon` somewhere if needed (probably keep `feed.rs` with just the icon handler). |
| Modify (T2) | `src/handlers/entry.rs` | Delete `refresh_feed_handler` (its logic is reused by the new form handler). |
| Delete (T2) | `static/js/pages/feeds.js` | Page module gone (529 lines). |
| Modify (T2) | `src/handlers/static_assets.rs` | Drop `js/pages/feeds.js` allowlist entry. |
| Modify (T2) | `tests/handlers_test.rs` | Delete `test_fetch_metadata_*` (3 tests, /api/feeds/fetch-metadata gone). Update / drop tests for `GET /api/feeds`. |
| Modify (T2) | `tests/pages_test.rs` | Update `test_feeds_page_*` to assert SSR content; drop CSR-shell assertions. |

**Endpoints kept** (used by external clients or non-page consumers): all `/reader/api/0/*` GReader endpoints; `GET /api/feeds/{id}/icon` (referenced by `<img>` in the SSR template).

---

## Task 1: Add 6 form-action POST endpoints under `/feeds/*`

| Method | Path | Handler | Form fields | Success | Error |
|--------|------|---------|-------------|---------|-------|
| POST | `/feeds` | `create_feed_form` | `url`, `category_id` | Redirect `/feeds` with success | Redirect `/feeds` with error |
| POST | `/feeds/{id}/edit` | `edit_feed_form` | `url`, `title`, `description`, `site_url`, `category_id`, `custom_user_agent`, `custom_referrer`, `http2_disabled` (checkbox), `_clear_referrer` (hidden), `_clear_user_agent` (hidden) | Redirect `/feeds/{id}/edit` with success | Redirect `/feeds/{id}/edit` with error |
| POST | `/feeds/{id}/delete` | `delete_feed_form` | (no body) | Redirect `/feeds` with success | Redirect `/feeds` with error |
| POST | `/feeds/{id}/refresh` | `refresh_feed_form` | (no body) | Redirect `/feeds` with success (incl. `N new, M updated`) | Redirect `/feeds` with error |
| POST | `/feeds/{id}/fetch-metadata` | `fetch_metadata_form` | (no body, reads URL from existing feed) | Redirect `/feeds/{id}/edit` with success — title/description/site_url updated in DB | Redirect `/feeds/{id}/edit` with error |
| POST | `/feeds/import` | `import_opml_form` | multipart: `file` (optional) and/or `content` (optional textarea) | Redirect `/feeds` with success | Redirect `/feeds/import` with error |

### Steps

- [ ] **Step 1: Read referenced helper signatures.**

  Required reading before writing handlers:
  - `src/models/feed.rs` lines 93-260 — `create_feed`, `update_feed`, `delete_feed`, `find_by_id`, `find_by_url_for_user`, `find_by_url_and_category`, `CreateFeedParams`, `UpdateFeedParams`.
  - `src/models/category.rs` lines 36-160 — `create_category`, `find_by_id_and_user`, `find_by_name_and_user`, `list_by_user`.
  - `src/services/feed_discovery.rs` — `discover_feed(url, user_agent) -> AppResult<DiscoveryResult>` (fields: `feed_url`, `title`, `description`, `site_url`).
  - `src/services/feed_sync.rs` — `refresh_feed(db, feed_id, user_agent) -> AppResult<SyncResult>` (fields: `new_entries`, `updated_entries`).
  - `src/services/opml.rs` — `parse_opml(content) -> AppResult<Vec<OpmlOutline>>`.
  - `src/middleware/flash.rs` lines 190-220 — `FlashRedirect::{success,error,info,warning}`.
  - Existing handlers `src/handlers/categories.rs` and `src/handlers/admin.rs` for the form-action style precedent.

- [ ] **Step 2: Extend `src/handlers/feeds.rs` with the 6 form handlers.**

  Append after the existing `list_feeds` function. Imports to add:
  ```rust
  use axum::{
      extract::{Multipart, Path as AxumPath, State},
      response::IntoResponse,
  };
  use axum::extract::Form;
  use serde::Deserialize;
  use crate::error::AppError;
  use crate::middleware::flash::FlashRedirect;
  use crate::services::{feed_discovery, feed_sync, opml};
  ```

  All 6 handlers extract `AuthUser` (the JSON-aware extractor used by existing `list_feeds`) **except** the form-action handlers should extract `PageAuthUser` instead — see admin handlers for the pattern. PageAuthUser sends 303 to `/login` on auth failure, AuthUser returns 401 JSON. **Decision: PageAuthUser** for all 6 form handlers (they are page-driven).

  Verify by reading `src/middleware/auth.rs` to confirm `PageAuthUser` is exposed and what its public path is. If unexposed there, copy the pattern from `src/handlers/admin.rs` line ~30.

  ### `create_feed_form`

  ```rust
  #[derive(Debug, Deserialize)]
  pub struct CreateFeedForm {
      pub url: String,
      pub category_id: i64,
  }

  pub async fn create_feed_form(
      State(state): State<AppState>,
      auth_user: PageAuthUser,
      Form(req): Form<CreateFeedForm>,
  ) -> impl IntoResponse {
      let url = req.url.trim().to_string();
      if url.is_empty() {
          return FlashRedirect::error("/feeds", "Feed URL cannot be empty").into_response();
      }
      let user_id = auth_user.user.id;
      let category_id = req.category_id;
      let user_agent = state.config.user_agent.clone();

      // Verify category ownership before doing network discovery.
      let owned = state
          .db
          .read_user(move |conn| {
              Ok::<_, AppError>(
                  category::find_by_id_and_user(conn, category_id, user_id)?.is_some(),
              )
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or(false);
      if !owned {
          return FlashRedirect::error("/feeds", "Invalid category").into_response();
      }

      let discovered = match feed_discovery::discover_feed(&url, &user_agent).await {
          Ok(d) => d,
          Err(e) => {
              return FlashRedirect::error("/feeds", format!("Failed to discover feed: {e}"))
                  .into_response();
          }
      };

      let create_url = discovered.feed_url.clone();
      let create_title = discovered.title.clone();
      let create_desc = discovered.description.clone();
      let create_site = discovered.site_url.clone();
      let result = state
          .db
          .user(move |conn| {
              if feed::find_by_url_for_user(conn, &create_url, user_id)?.is_some() {
                  return Err(AppError::FeedExists);
              }
              feed::create_feed(
                  conn,
                  &feed::CreateFeedParams {
                      category_id,
                      url: &create_url,
                      title: create_title.as_deref(),
                      description: create_desc.as_deref(),
                      site_url: create_site.as_deref(),
                      custom_user_agent: None,
                      http2_disabled: None,
                      custom_referrer: None,
                  },
              )?;
              Ok::<_, AppError>(())
          })
          .await;

      match result {
          Ok(Ok(())) => FlashRedirect::success("/feeds", "Feed added.").into_response(),
          Ok(Err(AppError::FeedExists)) => {
              FlashRedirect::error("/feeds", "Feed already subscribed").into_response()
          }
          Ok(Err(AppError::Validation(msg))) => {
              FlashRedirect::error("/feeds", msg).into_response()
          }
          _ => FlashRedirect::error("/feeds", "Failed to add feed").into_response(),
      }
  }
  ```

  ### `edit_feed_form`

  ```rust
  #[derive(Debug, Deserialize)]
  pub struct EditFeedForm {
      pub url: String,
      #[serde(default)]
      pub title: String,
      #[serde(default)]
      pub description: String,
      #[serde(default)]
      pub site_url: String,
      pub category_id: i64,
      #[serde(default)]
      pub custom_user_agent: String,
      #[serde(default)]
      pub custom_referrer: String,
      #[serde(default)]
      pub http2_disabled: Option<String>, // checkbox: "on" if checked, absent otherwise
      #[serde(default)]
      pub _clear_referrer: Option<String>,
      #[serde(default)]
      pub _clear_user_agent: Option<String>,
  }
  ```

  Logic:
  - Look up feed by id, verify category ownership through `find_by_id_and_user(conn, f.category_id, user_id)`.
  - Verify the new `category_id` belongs to user too.
  - Compute effective values:
    - `title` empty → keep existing.
    - `description`/`site_url`: empty string → set to None.
    - `custom_user_agent`: if `_clear_user_agent` present → None; if non-empty string → Some; else keep existing.
    - `custom_referrer`: same pattern with `_clear_referrer`.
    - `http2_disabled`: `Some("on")` → true, else false.
  - Call `feed::update_feed` with `UpdateFeedParams` (id, category_id=current, new_category_id=req, url=&req.url, title, description, site_url, custom_user_agent, http2_disabled, custom_referrer).
  - On success: `FlashRedirect::success(format!("/feeds/{id}/edit"), "Feed updated.")`.
  - On error: `FlashRedirect::error(format!("/feeds/{id}/edit"), "Failed to update feed")`.

  ### `delete_feed_form`

  ```rust
  pub async fn delete_feed_form(
      State(state): State<AppState>,
      auth_user: PageAuthUser,
      AxumPath(id): AxumPath<i64>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| {
              let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;
              category::find_by_id_and_user(conn, f.category_id, user_id)?
                  .ok_or(AppError::FeedNotFound)?;
              feed::delete_feed(conn, f.id, f.category_id)?;
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success("/feeds", "Feed deleted.").into_response(),
          _ => FlashRedirect::error("/feeds", "Failed to delete feed").into_response(),
      }
  }
  ```

  ### `refresh_feed_form`

  Reuses `feed_sync::refresh_feed` (the same service the JSON `/api/feeds/{id}/refresh` calls).

  ```rust
  pub async fn refresh_feed_form(
      State(state): State<AppState>,
      auth_user: PageAuthUser,
      AxumPath(id): AxumPath<i64>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      // Verify ownership before doing network sync.
      let owned = state
          .db
          .read_user(move |conn| {
              let f = match feed::find_by_id(conn, id)? {
                  Some(f) => f,
                  None => return Ok::<_, AppError>(false),
              };
              Ok(category::find_by_id_and_user(conn, f.category_id, user_id)?.is_some())
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or(false);
      if !owned {
          return FlashRedirect::error("/feeds", "Feed not found").into_response();
      }
      match feed_sync::refresh_feed(state.db.clone(), id, &state.config.user_agent).await {
          Ok(r) => FlashRedirect::success(
              "/feeds",
              format!("Refreshed: {} new, {} updated.", r.new_entries, r.updated_entries),
          )
          .into_response(),
          Err(e) => {
              FlashRedirect::error("/feeds", format!("Refresh failed: {e}")).into_response()
          }
      }
  }
  ```

  ### `fetch_metadata_form`

  Pulls metadata from the feed's stored URL, persists title/description/site_url, redirects back to the edit page so user sees the updated values.

  ```rust
  pub async fn fetch_metadata_form(
      State(state): State<AppState>,
      auth_user: PageAuthUser,
      AxumPath(id): AxumPath<i64>,
  ) -> impl IntoResponse {
      let user_id = auth_user.user.id;
      let edit_path = format!("/feeds/{id}/edit");

      let feed_owned = state
          .db
          .read_user(move |conn| {
              let f = match feed::find_by_id(conn, id)? {
                  Some(f) => f,
                  None => return Ok::<_, AppError>(None),
              };
              if category::find_by_id_and_user(conn, f.category_id, user_id)?.is_none() {
                  return Ok(None);
              }
              Ok(Some(f))
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .flatten();
      let feed = match feed_owned {
          Some(f) => f,
          None => return FlashRedirect::error(edit_path, "Feed not found").into_response(),
      };

      let user_agent = state.config.user_agent.clone();
      let discovered = match feed_discovery::discover_feed(&feed.url, &user_agent).await {
          Ok(d) => d,
          Err(e) => {
              return FlashRedirect::error(
                  edit_path,
                  format!("Failed to fetch metadata: {e}"),
              )
              .into_response();
          }
      };

      let category_id = feed.category_id;
      let result = state
          .db
          .user(move |conn| {
              feed::update_feed(
                  conn,
                  &feed::UpdateFeedParams {
                      id: feed.id,
                      category_id,
                      new_category_id: category_id,
                      url: &feed.url,
                      title: discovered.title.as_deref().or(feed.title.as_deref()),
                      description: discovered
                          .description
                          .as_deref()
                          .or(feed.description.as_deref()),
                      site_url: discovered.site_url.as_deref().or(feed.site_url.as_deref()),
                      custom_user_agent: feed.custom_user_agent.as_deref(),
                      http2_disabled: feed.http2_disabled,
                      custom_referrer: feed.custom_referrer.as_deref(),
                  },
              )?;
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => {
              FlashRedirect::success(edit_path, "Metadata fetched.").into_response()
          }
          _ => FlashRedirect::error(edit_path, "Failed to update feed").into_response(),
      }
  }
  ```

  ### `import_opml_form`

  Multipart: file or textarea. Concatenates the file body and textarea content (whichever is non-empty). Reuses `opml::parse_opml` + the same loop as `greader::subscription::import` lines 393-435.

  ```rust
  pub async fn import_opml_form(
      State(state): State<AppState>,
      auth_user: PageAuthUser,
      mut multipart: Multipart,
  ) -> impl IntoResponse {
      let mut content = String::new();
      while let Ok(Some(field)) = multipart.next_field().await {
          let name = field.name().unwrap_or("").to_string();
          let bytes = match field.bytes().await {
              Ok(b) => b,
              Err(_) => continue,
          };
          if (name == "file" || name == "content") && !bytes.is_empty() {
              if let Ok(text) = std::str::from_utf8(&bytes) {
                  if !text.trim().is_empty() {
                      content = text.to_string();
                      break;
                  }
              }
          }
      }
      if content.trim().is_empty() {
          return FlashRedirect::error(
              "/feeds/import",
              "Please upload a file or paste OPML content",
          )
          .into_response();
      }
      let outlines = match opml::parse_opml(&content) {
          Ok(o) => o,
          Err(e) => {
              return FlashRedirect::error(
                  "/feeds/import",
                  format!("Failed to parse OPML: {e}"),
              )
              .into_response();
          }
      };
      let user_id = auth_user.user.id;
      let result = state
          .db
          .user(move |conn| {
              for outline in outlines {
                  let cat = match category::find_by_name_and_user(
                      conn,
                      &outline.category_name,
                      user_id,
                  )? {
                      Some(cat) => cat,
                      None => category::create_category(conn, user_id, &outline.category_name)?,
                  };
                  for opml_feed in outline.feeds {
                      if feed::find_by_url_and_category(conn, &opml_feed.xml_url, cat.id)?
                          .is_some()
                      {
                          continue;
                      }
                      let _ = feed::create_feed(
                          conn,
                          &feed::CreateFeedParams {
                              category_id: cat.id,
                              url: &opml_feed.xml_url,
                              title: opml_feed.title.as_deref(),
                              description: None,
                              site_url: opml_feed.html_url.as_deref(),
                              custom_user_agent: None,
                              http2_disabled: None,
                              custom_referrer: None,
                          },
                      );
                  }
              }
              Ok::<_, AppError>(())
          })
          .await;
      match result {
          Ok(Ok(())) => FlashRedirect::success("/feeds", "OPML imported.").into_response(),
          _ => {
              FlashRedirect::error("/feeds/import", "Failed to import OPML").into_response()
          }
      }
  }
  ```

  Note: multipart processing reads only the first non-empty `file` or `content` field. This intentionally accepts either source. Upper bound on body size is governed by Axum's default which is fine for typical OPML files.

- [ ] **Step 3: Register routes in `src/lib.rs`.**

  Add 6 routes adjacent to the existing `/categories/*` form-action block (lines 112-123). Note: `/feeds` is currently `GET only`. Change it to `GET + POST` like `/categories`.

  ```rust
          .route(
              "/feeds",
              get(handlers::pages::feeds_page).post(handlers::feeds::create_feed_form),
          )
          .route(
              "/feeds/{id}/edit",
              post(handlers::feeds::edit_feed_form), // GET added in T2
          )
          .route(
              "/feeds/{id}/delete",
              post(handlers::feeds::delete_feed_form),
          )
          .route(
              "/feeds/{id}/refresh",
              post(handlers::feeds::refresh_feed_form),
          )
          .route(
              "/feeds/{id}/fetch-metadata",
              post(handlers::feeds::fetch_metadata_form),
          )
          .route(
              "/feeds/import",
              post(handlers::feeds::import_opml_form), // GET added in T2
          )
  ```

  IMPORTANT: leave `/api/feeds`, `/api/feeds/fetch-metadata`, `/api/feeds/{id}/refresh` in place during T1 — T2 deletes them with their last consumer.

- [ ] **Step 4: Add 7-9 integration tests in `tests/handlers_test.rs`.**

  Required tests (skipping anything that requires real network):

  1. `test_create_feed_form_empty_url` — POST `/feeds` with `url=""` → 303 to `/feeds` with error flash.
  2. `test_create_feed_form_invalid_category` — POST with valid url-shaped string + `category_id=999999` → 303 with error.
  3. `test_edit_feed_form_succeeds` — Pre-create feed via model. POST `/feeds/{id}/edit` with new title + same category_id → 303 to `/feeds/{id}/edit`. Verify title in DB.
  4. `test_edit_feed_form_changes_category` — Pre-create feed + 2nd category. POST with `category_id=other` → 303. Verify feed.category_id changed.
  5. `test_delete_feed_form_succeeds` — Pre-create feed. POST `/feeds/{id}/delete` → 303 to `/feeds`. Verify feed gone.
  6. `test_delete_feed_form_not_owned` — User A creates feed, user B tries to delete → 303 with error (or treats as not found).
  7. `test_refresh_feed_form_not_owned` — User B → user A's feed → 303 to `/feeds` with error flash. (Network probe is skipped because ownership check fails first.)
  8. `test_import_opml_form_empty` — multipart with neither file nor content → 303 to `/feeds/import` with error.
  9. `test_import_opml_form_succeeds` — multipart with `content=<sample OPML>` → 303 to `/feeds`. Verify categories + feeds created.

  Skip writing tests for `create_feed_form` happy-path (`feed_discovery` does network), `refresh_feed_form` happy-path (network), `fetch_metadata_form` happy-path (network). The branch's existing tests already exercise `feed_discovery` via the GReader subscribe path — that coverage is sufficient.

  Use `app.server.post(...).form(&serde_json::json!({...}))` for url-encoded form, and `multipart` builder from `axum_test::multipart::*` for the OPML test.

  Look at existing patterns in `tests/handlers_test.rs` for how the multi-user setup helper is named (probably `setup_authenticated_user` and a sibling for second user).

- [ ] **Step 5: Compile + test.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run`
  Expected: 700 baseline + 7-9 new tests pass.

- [ ] **Step 6: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add src/handlers/feeds.rs src/lib.rs tests/handlers_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): add /feeds form-action endpoints

  Six new POST endpoints under /feeds/* accept form bodies and return
  FlashRedirect responses (303 + flash cookie + Location):

      POST /feeds                   create
      POST /feeds/{id}/edit         update
      POST /feeds/{id}/delete       delete
      POST /feeds/{id}/refresh      manual sync (incl. count flash)
      POST /feeds/{id}/fetch-metadata  re-fetch + persist title/desc/site
      POST /feeds/import            multipart OPML import

  GReader /reader/api/0/subscription/{edit,import,export} stay alive —
  used by external clients (FreshRSS, Reeder).

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: SSR /feeds list + edit + import pages, delete CSR

### Steps

- [ ] **Step 1: Rewrite `templates/feeds.html` (list page).**

  Replace the file's contents with full SSR. Use `<form method="get">` with inline `onchange="this.form.submit()"` for the filter selects.

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="feeds"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Feeds</h1>

                  <div class="feeds-toolbar">
                      <a href="/reader/api/0/subscription/export" class="btn btn-secondary">Export OPML</a>
                      <a href="/feeds/import" class="btn-secondary">Import OPML</a>
                  </div>

                  <hr>

                  <form method="post" action="/feeds">
                      <div class="form-group">
                          <label for="url">Feed URL</label>
                          <input type="text" id="url" name="url" placeholder="https://example.com/feed.xml or https://example.com" required data-testid="feed-url-input">
                      </div>
                      <div class="form-group">
                          <label for="category_id">Category</label>
                          <select id="category_id" name="category_id" required data-testid="feed-category-select">
                              {% if categories.is_empty() %}
                                  <option value="">No categories available</option>
                              {% else %}
                                  {% for c in categories %}
                                      <option value="{{ c.id }}">{{ c.name }}</option>
                                  {% endfor %}
                              {% endif %}
                          </select>
                      </div>
                      <button type="submit" data-testid="add-feed-btn">Add Feed</button>
                  </form>

                  <hr>

                  <form method="get" action="/feeds" class="filter-bar">
                      <div class="form-group form-group-inline">
                          <label for="filter-category">Category</label>
                          <select id="filter-category" name="category" class="select-auto" onchange="this.form.submit()">
                              <option value="">All Categories ({{ total_feed_count }})</option>
                              {% for c in categories %}
                                  <option value="{{ c.id }}"{% if active_category == Some(c.id) %} selected{% endif %}>{{ c.name }} ({{ c.feed_count }})</option>
                              {% endfor %}
                          </select>
                      </div>
                      <div class="form-group form-group-inline">
                          <label for="sort-by">Sort</label>
                          <select id="sort-by" name="sort" class="select-auto" onchange="this.form.submit()">
                              <option value="title"{% if active_sort == "title" %} selected{% endif %}>Title</option>
                              <option value="unread"{% if active_sort == "unread" %} selected{% endif %}>Unread Count</option>
                              <option value="category"{% if active_sort == "category" %} selected{% endif %}>Category</option>
                          </select>
                      </div>
                      <div class="form-group form-group-inline feed-filter-links">
                          {% for f in filter_links %}
                              <a href="{{ f.href }}" class="feed-filter-link{% if f.active %} active{% endif %}">{{ f.label }}</a>
                          {% endfor %}
                      </div>
                      {# preserve filter param on category/sort change #}
                      <input type="hidden" name="filter" value="{{ active_filter }}">
                  </form>

                  <table class="mobile-cards">
                      <thead>
                          <tr>
                              <th>Title</th>
                              <th>Category</th>
                              <th>Unread</th>
                              <th>Actions</th>
                          </tr>
                      </thead>
                      <tbody data-testid="feeds-table">
                          {% for feed in feeds %}
                              <tr id="row-feed-{{ feed.id }}" data-feed-id="{{ feed.id }}"{% if feed.fetch_error.is_some() %} class="feed-error-no-border"{% endif %}>
                                  <td data-label="Title"{% if feed.fetch_error.is_some() %} class="feed-error-no-border"{% endif %}>
                                      <div class="feed-title-cell">
                                          <div>
                                              {% if feed.has_icon %}
                                                  <img src="/api/feeds/{{ feed.id }}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">
                                              {% endif %}
                                              <span title="{{ feed.url }}">{{ feed.title }}</span>
                                          </div>
                                          <div class="feed-health-info">
                                              <span class="muted" title="{{ feed.fetched_at_datetime }}">Fetched: {{ feed.fetched_at_relative }}</span>
                                              &middot;
                                              <span class="{{ feed.freshness_class }}" title="{{ feed.feed_updated_at_datetime }}">Updated: {{ feed.feed_updated_at_relative }}</span>
                                          </div>
                                      </div>
                                  </td>
                                  <td data-label="Category"{% if feed.fetch_error.is_some() %} class="feed-error-no-border"{% endif %}>{{ feed.category_name }}</td>
                                  <td data-label="Unread"{% if feed.fetch_error.is_some() %} class="feed-error-no-border"{% endif %}>
                                      {% if feed.unread_count > 0 %}<strong>{{ feed.unread_count }}</strong>{% else %}0{% endif %}
                                  </td>
                                  <td class="actions{% if feed.fetch_error.is_some() %} feed-error-no-border{% endif %}">
                                      <a href="/feeds/{{ feed.id }}/entries">entries</a>
                                      <form method="post" action="/feeds/{{ feed.id }}/refresh" style="display:inline">
                                          <button type="submit" class="action-link">refresh</button>
                                      </form>
                                      <a href="/feeds/{{ feed.id }}/edit">edit</a>
                                      <form method="post" action="/feeds/{{ feed.id }}/delete" style="display:inline" onsubmit="return confirm('Delete feed &quot;{{ feed.title }}&quot;? This cannot be undone.')">
                                          <button type="submit" class="action-link-danger">delete</button>
                                      </form>
                                  </td>
                              </tr>
                              {% if let Some(err) = feed.fetch_error %}
                                  <tr class="error-row" data-feed-id="{{ feed.id }}" data-is-error-row="true">
                                      <td colspan="4" class="error-text feed-error-cell">Error: {{ err }}</td>
                                  </tr>
                              {% endif %}
                          {% endfor %}
                          {% if feeds.is_empty() %}
                              <tr><td colspan="4" class="muted">No feeds yet.</td></tr>
                          {% endif %}
                      </tbody>
                  </table>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Notes:
  - `feed.title` text in the inline `confirm()` may contain quotes — escaping is via `{{ ... }}` (Askama default html-escape) PLUS the surrounding `'...'` JS string. Use `&quot;` in the message text and trust Askama to escape `<>` safely.
  - `class="action-link"` / `action-link-danger` / `link-button` exist in CSS from PR-5/PR-7.
  - The hidden `filter` input on the GET form preserves the active filter when the user changes category/sort. The filter links carry their own URL.
  - The `{% if let Some(err) = feed.fetch_error %}` pattern requires Askama's `if let` support — verify Askama 0.15 has it. If not, expose a separate `feed.has_error` bool and `feed.fetch_error_msg: String`.

- [ ] **Step 2: Rewrite `FeedsTemplate` and `feeds_page` handler in `src/handlers/pages.rs`.**

  Move `FeedRowView` + filter context types into the template's struct. The view-construction logic ports almost line-for-line from `src/handlers/feeds.rs::list_feeds` (currently the JSON DTO builder) — the JSON-only fields (`url`, `description`, `site_url`, `custom_user_agent`, etc.) are dropped from the row view since the list page doesn't need them.

  ```rust
  pub struct FeedRowView {
      pub id: i64,
      pub url: String,
      pub title: String,
      pub category_id: i64,
      pub category_name: String,
      pub has_icon: bool,
      pub fetch_error: Option<String>,
      pub unread_count: i64,
      pub fetched_at_relative: String,
      pub fetched_at_datetime: String,
      pub feed_updated_at_relative: String,
      pub feed_updated_at_datetime: String,
      pub freshness_class: String,
      pub freshness_key: String,
  }

  pub struct FeedCategoryOption {
      pub id: i64,
      pub name: String,
      pub feed_count: i64,
  }

  pub struct FeedFilterLink {
      pub label: &'static str,
      pub href: String,
      pub active: bool,
  }

  #[derive(Template)]
  #[template(path = "feeds.html")]
  pub struct FeedsTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub feeds: Vec<FeedRowView>,
      pub categories: Vec<FeedCategoryOption>,
      pub total_feed_count: i64,
      pub active_filter: String,
      pub active_sort: String,
      pub active_category: Option<i64>,
      pub filter_links: Vec<FeedFilterLink>,
  }
  ```

  Rewrite `feeds_page`:

  ```rust
  pub async fn feeds_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
      Query(query): Query<FeedsQuery>,
  ) -> (Flash, FeedsTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let user_id = auth_user.user.id;

      let (mut rows, categories, total) = state.db.read_user(move |conn| {
          let cats = category::list_by_user(conn, user_id).unwrap_or_default();
          let feeds = feed::list_by_user(conn, user_id).unwrap_or_default();
          let unread_map = entry::count_unread_by_feed(conn, user_id).unwrap_or_default();

          let cat_map: std::collections::HashMap<i64, String> =
              cats.iter().map(|c| (c.id, c.name.clone())).collect();
          let mut count_by_cat: std::collections::HashMap<i64, i64> =
              std::collections::HashMap::new();
          for f in &feeds {
              *count_by_cat.entry(f.category_id).or_insert(0) += 1;
          }

          let total = feeds.len() as i64;
          let row_views: Vec<FeedRowView> = feeds.into_iter().map(|f| {
              // …same logic as current list_feeds: relative-time, has_icon query,
              // freshness compute. Keep compute_freshness + format_relative_time
              // helpers (still defined in pages.rs).
              // …
              FeedRowView { /* … */ }
          }).collect();

          let cat_views: Vec<FeedCategoryOption> = cats.into_iter().map(|cat| FeedCategoryOption {
              feed_count: count_by_cat.get(&cat.id).copied().unwrap_or(0),
              id: cat.id,
              name: cat.name,
          }).collect();

          Ok::<_, AppError>((row_views, cat_views, total))
      }).await.ok().and_then(|r| r.ok()).unwrap_or_default();

      let active_filter = query.filter.as_deref().unwrap_or("all").to_string();
      let active_sort = query.sort.as_deref().unwrap_or("title").to_string();
      let active_category = query.category.as_deref().and_then(|s| s.parse::<i64>().ok());

      if let Some(cid) = active_category {
          rows.retain(|r| r.category_id == cid);
      }
      match active_filter.as_str() {
          "errors" => rows.retain(|r| r.fetch_error.is_some()),
          "stale" => rows.retain(|r| r.freshness_key == "stale"),
          _ => {}
      }
      match active_sort.as_str() {
          "unread" => rows.sort_by(|a, b| b.unread_count.cmp(&a.unread_count)),
          "category" => rows.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
          _ => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
      }
      let normalized_filter = match active_filter.as_str() {
          "errors" | "stale" | "all" => active_filter.clone(),
          _ => "all".to_string(),
      };

      let cat_param = active_category.map(|c| format!("category={c}&")).unwrap_or_default();
      let filter_links = vec![
          FeedFilterLink {
              label: "All",
              href: format!("/feeds?{}sort={}&filter=all", cat_param, active_sort),
              active: normalized_filter == "all",
          },
          FeedFilterLink {
              label: "Errors",
              href: format!("/feeds?{}sort={}&filter=errors", cat_param, active_sort),
              active: normalized_filter == "errors",
          },
          FeedFilterLink {
              label: "Stale",
              href: format!("/feeds?{}sort={}&filter=stale", cat_param, active_sort),
              active: normalized_filter == "stale",
          },
      ];

      (flash, FeedsTemplate {
          title: "Feeds",
          git_version: crate::GIT_VERSION,
          layout,
          feeds: rows,
          categories,
          total_feed_count: total,
          active_filter: normalized_filter,
          active_sort,
          active_category,
          filter_links,
      })
  }
  ```

- [ ] **Step 3: Add `templates/feed_edit.html` and the GET handler.**

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="feeds"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Edit Feed</h1>

                  <form method="post" action="/feeds/{{ feed.id }}/edit">
                      <div class="form-group">
                          <label for="url">Feed URL</label>
                          <div class="feed-edit-url-row">
                              <input type="text" id="url" name="url" value="{{ feed.url }}" required>
                          </div>
                      </div>
                      <div class="form-group">
                          <label for="title">Title</label>
                          <input type="text" id="title" name="title" value="{{ feed.title|default("") }}">
                      </div>
                      <div class="form-group">
                          <label for="description">Description</label>
                          <input type="text" id="description" name="description" value="{{ feed.description|default("") }}">
                      </div>
                      <div class="form-group">
                          <label for="site_url">Site URL</label>
                          <input type="text" id="site_url" name="site_url" value="{{ feed.site_url|default("") }}">
                      </div>
                      <div class="form-group">
                          <label for="category_id">Category</label>
                          <select id="category_id" name="category_id" required>
                              {% for c in categories %}
                                  <option value="{{ c.id }}"{% if c.id == feed.category_id %} selected{% endif %}>{{ c.name }}</option>
                              {% endfor %}
                          </select>
                      </div>
                      <details class="feed-http-settings">
                          <summary>HTTP Settings</summary>
                          <div class="feed-http-settings-body">
                              <div class="form-group">
                                  <label for="custom_user_agent">Custom User Agent</label>
                                  <input type="text" id="custom_user_agent" name="custom_user_agent" placeholder="Leave empty to use global default" value="{{ feed.custom_user_agent|default("") }}">
                              </div>
                              <div class="form-group">
                                  <label for="custom_referrer">Custom Referrer</label>
                                  <input type="text" id="custom_referrer" name="custom_referrer" placeholder="Leave empty to not send Referer header" value="{{ feed.custom_referrer|default("") }}">
                                  <div class="feed-http-hint">Some image servers require a specific Referer header to serve images</div>
                              </div>
                              <div class="form-group">
                                  <label>
                                      <input type="checkbox" id="http2_disabled" name="http2_disabled"{% if feed.http2_disabled %} checked{% endif %}>
                                      Disable HTTP/2
                                  </label>
                                  <div class="feed-http-hint">Enable this if the feed server has HTTP/2 compatibility issues</div>
                              </div>
                          </div>
                      </details>
                      <div class="modal-actions">
                          <button type="submit">Save</button>
                          <a href="/feeds" class="btn-secondary">Cancel</a>
                      </div>
                  </form>

                  <hr>

                  <form method="post" action="/feeds/{{ feed.id }}/fetch-metadata" onsubmit="return confirm('Re-fetch metadata from the feed URL? This will overwrite current title/description/site URL.')">
                      <button type="submit" class="btn-secondary">Re-fetch Metadata</button>
                  </form>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  Add to `src/handlers/pages.rs`:

  ```rust
  pub struct FeedEditView {
      pub id: i64,
      pub url: String,
      pub title: String,
      pub description: Option<String>,
      pub site_url: Option<String>,
      pub category_id: i64,
      pub custom_user_agent: Option<String>,
      pub http2_disabled: bool,
      pub custom_referrer: Option<String>,
  }

  #[derive(Template)]
  #[template(path = "feed_edit.html")]
  pub struct FeedEditTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub feed: FeedEditView,
      pub categories: Vec<FeedCategoryOption>, // reuse; only id+name needed
  }

  // …IntoResponse impl…

  pub async fn feed_edit_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
      Path(id): Path<i64>,
  ) -> AppResult<(Flash, FeedEditTemplate)> {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let user_id = auth_user.user.id;

      let (feed_view, cats) = state
          .db
          .read_user(move |conn| {
              let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;
              category::find_by_id_and_user(conn, f.category_id, user_id)?
                  .ok_or(AppError::FeedNotFound)?;
              let cats = category::list_by_user(conn, user_id)?;
              Ok::<_, AppError>((
                  FeedEditView {
                      id: f.id,
                      url: f.url,
                      title: f.title.unwrap_or_default(),
                      description: f.description,
                      site_url: f.site_url,
                      category_id: f.category_id,
                      custom_user_agent: f.custom_user_agent,
                      http2_disabled: f.http2_disabled,
                      custom_referrer: f.custom_referrer,
                  },
                  cats.into_iter()
                      .map(|c| FeedCategoryOption {
                          id: c.id,
                          name: c.name,
                          feed_count: 0,
                      })
                      .collect::<Vec<_>>(),
              ))
          })
          .await??;

      Ok((
          flash,
          FeedEditTemplate {
              title: "Edit Feed",
              git_version: crate::GIT_VERSION,
              layout,
              feed: feed_view,
              categories: cats,
          },
      ))
  }
  ```

- [ ] **Step 4: Add `templates/feeds_import.html` and the GET handler.**

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="feeds"></rdrs-sidebar>
          <main class="main-content">
              <div class="page-content">
                  <rdrs-flash class="flash-container"></rdrs-flash>
                  <h1>Import OPML</h1>

                  <form method="post" action="/feeds/import" enctype="multipart/form-data">
                      <div class="form-group">
                          <label for="file">Upload .opml file</label>
                          <input type="file" id="file" name="file" accept=".opml,.xml">
                      </div>
                      <div class="form-group">
                          <label for="content">Or paste OPML content</label>
                          <textarea id="content" name="content" rows="10" class="textarea-full"></textarea>
                      </div>
                      <div class="modal-actions">
                          <button type="submit">Import</button>
                          <a href="/feeds" class="btn-secondary">Cancel</a>
                      </div>
                  </form>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  ```rust
  #[derive(Template)]
  #[template(path = "feeds_import.html")]
  pub struct FeedsImportTemplate {
      pub title: &'static str,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
  }
  // …IntoResponse impl…

  pub async fn feeds_import_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, FeedsImportTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      (flash, FeedsImportTemplate {
          title: "Import OPML",
          git_version: crate::GIT_VERSION,
          layout,
      })
  }
  ```

- [ ] **Step 5: Wire new GET routes in `src/lib.rs`.**

  ```rust
          .route(
              "/feeds/{id}/edit",
              get(handlers::pages::feed_edit_page).post(handlers::feeds::edit_feed_form),
          )
          .route(
              "/feeds/import",
              get(handlers::pages::feeds_import_page).post(handlers::feeds::import_opml_form),
          )
  ```

- [ ] **Step 6: Delete unused JSON endpoints + handlers + JS module + tests.**

  In `src/lib.rs`, delete these lines:
  ```rust
          .route("/api/feeds", get(handlers::feeds::list_feeds))
          .route(
              "/api/feeds/fetch-metadata",
              post(handlers::feed::fetch_metadata),
          )
          .route(
              "/api/feeds/{id}/refresh",
              post(handlers::entry::refresh_feed_handler),
          )
  ```

  Keep:
  ```rust
          .route("/api/feeds/{id}/icon", get(handlers::feed::get_feed_icon))
  ```

  In `src/handlers/feeds.rs`, delete `FeedDto`, `CategoryOptionDto`, `FeedsResponse`, `list_feeds`. The 6 form handlers from T1 stay.

  In `src/handlers/feed.rs`, delete `fetch_metadata`, `FetchMetadataRequest`, `FeedMetadataResponse`. Keep `get_feed_icon`. (File still alive.)

  In `src/handlers/entry.rs`, delete `refresh_feed_handler`. The `crate::services::feed_sync::refresh_feed` function it called stays — used by both the new form handler and the cron job.

  Delete `static/js/pages/feeds.js`:
  ```bash
  git rm static/js/pages/feeds.js
  ```

  Drop the allowlist entry from `src/handlers/static_assets.rs` for `js/pages/feeds.js`.

  In `tests/handlers_test.rs`, delete:
  - `test_fetch_metadata_empty_url`
  - `test_fetch_metadata_whitespace_url`
  - `test_fetch_metadata_unauthorized`

  Search for any test that calls `GET /api/feeds` JSON endpoint and either delete or rewrite. Run:
  ```bash
  grep -n '"/api/feeds"\|/api/feeds/fetch-metadata\|/api/feeds/.*refresh' tests/
  ```

- [ ] **Step 7: Update `tests/pages_test.rs::test_feeds_page_*`.**

  Find existing `test_feeds_page_with_flash` (line ~349) and `test_feeds_page_csr_shell_does_not_embed_rows` (line ~755). Update / replace to assert SSR content:
  - `<h1>Feeds</h1>`
  - `<form method="post" action="/feeds">` (add form)
  - `data-testid="feeds-table"`
  - When feeds exist (pre-create via model in test setup), feed titles + URLs appear; `/feeds/{id}/refresh`, `/feeds/{id}/edit`, `/feeds/{id}/delete` form actions appear.
  - The `*_csr_shell_does_not_embed_rows` test's premise inverts — keep it as a positive assertion that rows ARE embedded, or delete and add a new `test_feeds_page_renders_rows` test.

  Add a new test for `feed_edit_page`: GET `/feeds/{id}/edit` → 200, contains `<input type="text" id="url"` with the feed URL, contains all category options.

  Add a new test for `feeds_import_page`: GET `/feeds/import` → 200, contains `<form … enctype="multipart/form-data">`.

- [ ] **Step 8: Compile + test.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run`
  Expected: full suite green. New count ≈ baseline + (new T1 tests) + (new T2 page tests) − 3 (deleted fetch-metadata tests).

- [ ] **Step 9: Verify cleanup.**

  ```bash
  grep -rn "rdrs-feeds-page\|/static/js/pages/feeds.js\|/api/feeds/fetch-metadata\|/api/feeds/.*refresh\|fn list_feeds\|fn refresh_feed_handler\|fn fetch_metadata\b" src/ templates/ static/ tests/
  ```
  Acceptable: zero in `src/`, `templates/`, `static/`. In `tests/`, only negative assertions (if any retained).

  ```bash
  grep -rn '"/api/feeds"' src/ templates/ static/ tests/
  ```
  Should be zero.

- [ ] **Step 10: Format and commit.**

  Run: `cargo fmt`
  ```bash
  git add templates/feeds.html templates/feed_edit.html templates/feeds_import.html \
      src/handlers/pages.rs src/handlers/feeds.rs src/handlers/feed.rs src/handlers/entry.rs \
      src/handlers/static_assets.rs src/lib.rs tests/handlers_test.rs tests/pages_test.rs
  # git rm of feeds.js already staged.
  git commit -m "$(cat <<'EOF'
  feat(ssr): SSR /feeds — drop CSR element

  /feeds now renders the feed table directly from the DB with filter,
  sort, per-row refresh/delete forms, and an add-feed form at the top.
  Editing moves to a dedicated SSR page at /feeds/{id}/edit and OPML
  import to /feeds/import (multipart upload).

  Deletes static/js/pages/feeds.js (529 lines), the
  <rdrs-feeds-page> custom element, and the now-unused JSON endpoints
  GET /api/feeds, POST /api/feeds/fetch-metadata, and
  POST /api/feeds/{id}/refresh.

  GET /api/feeds/{id}/icon stays (referenced from <img>). The
  /reader/api/0/subscription/{edit,import,export} GReader endpoints
  stay alive — external clients depend on them.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Wrap-up

- [ ] **Final sweep.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run && cargo fmt --check`

- [ ] **Push branch.**

  Run: `git push -u origin feat/ssr-feeds-page`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-8 — /feeds page" --body "$(cat <<'EOF'
  ## Summary

  Migrates `/feeds` to SSR + 6 form-action endpoints under `/feeds/*`
  (create, edit, delete, refresh, fetch-metadata, OPML import). The
  list page uses `<select onchange="this.form.submit()">` for filter
  & sort, per-row inline `<form>`s for refresh & delete (with
  `onsubmit="return confirm()"`), and a top-of-page form for add.
  Editing moves to a dedicated `/feeds/{id}/edit` SSR page; OPML
  import to `/feeds/import` (multipart upload).

  Deletes `static/js/pages/feeds.js` (529 lines), the
  `<rdrs-feeds-page>` custom element, and the JSON endpoints
  `GET /api/feeds`, `POST /api/feeds/fetch-metadata`, and
  `POST /api/feeds/{id}/refresh`. `GET /api/feeds/{id}/icon` stays —
  referenced from `<img src=…>`. The
  `/reader/api/0/subscription/{edit,import,export}` GReader
  endpoints stay alive — external clients depend on them.

  ## Test plan

  - [x] `cargo nextest run` — full suite green.
  - [x] 7-9 new endpoint tests covering create/edit/delete/refresh
    /fetch-metadata/import + ownership rejections.
  - [x] Updated `tests/pages_test.rs::test_feeds_page_*` to assert
    SSR content; added tests for `/feeds/{id}/edit` and
    `/feeds/import` GET pages.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-09-ssr-first-pr8-feeds-page.md`

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-9 is `/search` SSR (Low risk).
