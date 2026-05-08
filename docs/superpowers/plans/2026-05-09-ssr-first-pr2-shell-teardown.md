# SSR-first PR-2: Shell Teardown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `app_shell.html` rendering pipeline with `base.html` + 14 thin per-route templates extending a new `app_layout.html`; remove the SPA router; ship `static/js/app.js` containing the `swap()` helper plus a full-reload `rdrsNavigate` stub. All 14 logged-in routes continue to mount their existing CSR `<rdrs-*-page>` element. No real per-page SSR content lands in this PR — that begins in PR-3.

**Architecture:** This PR is a structural refactor. The render pipeline shape changes; the per-page behaviour does not. Three preparatory additions (Task 1) then a mechanical 14-route migration in two waves (Tasks 2 and 3) then teardown of the old shell + SPA router (Task 4). Each task is independently committed; mid-PR the repo runs.

**Tech Stack:** Rust + Axum 0.8 + Askama 0.15 + vanilla JS (single `app.js`). All deps already present.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`

**Branch:** `feat/ssr-shell-teardown` (already created off updated `main`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-shell-teardown`, working tree clean, latest commit is `b5f8ecb feat(ssr): SSR-first PR-1 — brotli, page_cache, ETag (#185)` (PR-1 already merged into main).

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: all 701 tests pass (this is the baseline after PR-1).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `static/js/app.js` | Single shared module for the logged-in surface. PR-2 ships only the `swap()` helper plus a full-reload `rdrsNavigate` stub (so existing CSR call sites keep working after `router.js` is deleted). Per-page PRs grow this file. |
| Modify | `src/handlers/static_assets.rs` | Add `js/app.js` to the `FILES` allowlist; remove `js/router.js` (in Task 4). |
| Create | `templates/app_layout.html` | New logged-in layout. Extends `base.html`. Defines blocks `page_script` (per-page module `<script>` tag) and `page` (per-page element body). Inlines the sidebar + flash bootstrap JSON, mounts `<rdrs-kb-help>` / `<rdrs-kb-pending>`, imports the global custom-element / keyboard / app.js modules. |
| Modify | `src/handlers/pages.rs` | Add `pub struct AppLayoutContext { theme, git_version, sidebar_bootstrap_json, flash_bootstrap_json }` plus `pub async fn build_app_layout(state, auth_user, flash) -> AppLayoutContext`. Each per-route template struct embeds a single `pub layout: AppLayoutContext` field. Existing `AppShellTemplate` stays alive through Tasks 1-3 and is deleted in Task 4. |
| Create | `templates/{settings,user_settings,admin,statistics,categories,feeds}.html` | Six thin per-route templates (Task 2). Each `extends "app_layout.html"`, sets `{% block title %}` and fills `{% block page %}` + `{% block page_script %}`. ~10 lines each. |
| Create | `templates/{unread,entries,read_entries,starred_entries,summarized_entries,feed_entries,category_entries,search}.html` | Seven entries-family per-route templates (Task 3). Same shape as Task 2's templates. |
| Modify | `src/handlers/pages.rs` | Each of 13 handlers swaps its return type from `(Flash, AppShellTemplate)` to `(Flash, <RouteName>Template)`. New per-route template structs added. |
| Modify | `templates/base.html` | (Task 4) Slim down: remove `kb-pending`, `kb-help`, `sidebar`, `keyboard`, `entry-list` script imports — those move into `app_layout.html`. `rdrs-flash.js` stays in base because login/register depend on `window.flash.redirect`. |
| Delete | `templates/app_shell.html` | (Task 4) |
| Delete | `static/js/router.js` | (Task 4) Removed from `FILES` allowlist. |
| Delete (struct) | `AppShellTemplate` in `src/handlers/pages.rs` | (Task 4) |
| Modify | `tests/pages_test.rs`, `tests/handlers_test.rs`, `tests/statistics_test.rs`, etc. | (Task 4) Update assertions that reference `app_shell` shape — usually means changing the literal string asserted in HTML to the new shape (e.g. asserting on `<rdrs-entries-page>` directly instead of via `AppShellTemplate`'s element_tag field). |

Login + register are NOT touched. They continue to extend the slimmed `base.html` and use only `rdrs-flash.js` (which stays in base).

---

## Task 1: Foundation — app.js + app_layout.html + AppLayoutContext

Adds the new layout template and its supporting Rust struct, plus `app.js` with the `swap()` helper and a full-reload `rdrsNavigate` stub. **Touches no existing route** — `app_shell.html` and the existing 13 handlers stay alive.

**Files:**
- Create: `static/js/app.js`
- Modify: `src/handlers/static_assets.rs`
- Create: `templates/app_layout.html`
- Modify: `src/handlers/pages.rs` (add struct + helper, do NOT change existing handlers)
- Test: extend `tests/pages_test.rs` with a unit test for `build_app_layout`

- [ ] **Step 1: Create `static/js/app.js`.**

  Create `static/js/app.js`:

  ```js
  // static/js/app.js — shared module for the logged-in surface.
  //
  // Currently ships:
  //   - swap(): partial-swap helper used by per-page SSR PRs to replace
  //     a target element via fetch + outerHTML. Not yet used by any
  //     consumer in PR-2.
  //   - window.rdrsNavigate: full-reload stub. Replaces the SPA router's
  //     export of the same name so existing CSR call sites in
  //     keyboard.js / page modules continue to work after router.js is
  //     removed. Each call falls through to a full document load.
  //
  // Per-page SSR PRs (PR-3+) extend this module with keyboard shortcuts,
  // sidebar polling, flash dismiss, and theme controller code. Those
  // sections are intentionally absent here.

  /**
   * Intercept form / link interactions tagged with `data-swap="<selector>"`
   * and replace the matching element with HTML returned by the request.
   *
   * Response format:
   *   - HTML fragment: replaces the target element via outerHTML.
   *   - Multi-target: response containing one or more
   *     `<template data-swap-target="<selector>">…</template>` blocks.
   *     Each template's content replaces its target via outerHTML.
   *
   * On a non-2xx response the helper falls back to native form submit /
   * link navigation so the user always sees a real page.
   */
  function installSwap() {
      document.addEventListener('click', async (event) => {
          const anchor = event.target.closest('a[data-swap]');
          if (!anchor) return;
          if (event.button !== 0 || event.metaKey || event.ctrlKey ||
              event.shiftKey || event.altKey) return;
          const target = anchor.getAttribute('data-swap');
          event.preventDefault();
          await performSwap(anchor.href, { method: 'GET' }, target);
      });

      document.addEventListener('submit', async (event) => {
          const form = event.target.closest('form[data-swap]');
          if (!form) return;
          const target = form.getAttribute('data-swap');
          event.preventDefault();
          const method = (form.method || 'GET').toUpperCase();
          const action = form.action;
          const init = { method };
          if (method !== 'GET') {
              init.body = new FormData(form);
          }
          await performSwap(action, init, target);
      });
  }

  async function performSwap(url, init, defaultTarget) {
      let response;
      try {
          response = await fetch(url, init);
      } catch {
          window.location.href = url;
          return;
      }
      if (!response.ok) {
          window.location.href = url;
          return;
      }
      const text = await response.text();
      const parsed = new DOMParser().parseFromString(text, 'text/html');

      const templates = parsed.querySelectorAll('template[data-swap-target]');
      if (templates.length > 0) {
          for (const tpl of templates) {
              const sel = tpl.getAttribute('data-swap-target');
              const dst = document.querySelector(sel);
              if (!dst) continue;
              const incoming = tpl.content.firstElementChild;
              if (!incoming) continue;
              dst.outerHTML = incoming.outerHTML;
          }
          return;
      }

      const dst = document.querySelector(defaultTarget);
      if (!dst) return;
      const incoming = parsed.body.firstElementChild;
      if (!incoming) return;
      dst.outerHTML = incoming.outerHTML;
  }

  // Full-reload stub. The SPA router's `window.rdrsNavigate(path)` API
  // is preserved here as a thin wrapper around `location.href = path`,
  // letting existing CSR keyboard / dropdown / page-module code keep
  // working after router.js is removed. Per-page PRs delete each call
  // site as they migrate to SSR.
  window.rdrsNavigate = function(path) {
      window.location.href = path;
  };

  installSwap();
  ```

- [ ] **Step 2: Register `app.js` in the static-assets allowlist.**

  Edit `src/handlers/static_assets.rs`. Append a new tuple to `FILES` (keeping `js/router.js` in place — it gets removed in Task 4). Add this entry just before the `("js/router.js", …)` line:

  ```rust
      ("js/app.js", include_str!("../../static/js/app.js")),
  ```

- [ ] **Step 3: Create `templates/app_layout.html`.**

  Create `templates/app_layout.html`:

  ```html
  {% extends "base.html" %}

  {% block html_attrs %}{% if let Some(t) = layout.theme %} data-theme="{{ t }}"{% endif %}{% endblock %}

  {% block title %}{{ title }} - RDRS{% endblock %}

  {% block head %}
      <script type="module" src="/static/js/components/rdrs-kb-pending.js?v={{ layout.git_version }}"></script>
      <script type="module" src="/static/js/components/rdrs-kb-help.js?v={{ layout.git_version }}"></script>
      <script type="module" src="/static/js/components/rdrs-sidebar.js?v={{ layout.git_version }}"></script>
      <script type="module" src="/static/js/keyboard.js?v={{ layout.git_version }}"></script>
      <script type="module" src="/static/js/components/rdrs-entry-list.js?v={{ layout.git_version }}"></script>
      <script type="module" src="/static/js/app.js?v={{ layout.git_version }}"></script>
      {% block page_script %}{% endblock %}
  {% endblock %}

  {% block body %}
      <script type="application/json" id="rdrs-sidebar-bootstrap">{{ layout.sidebar_bootstrap_json|safe }}</script>
      <script type="application/json" id="rdrs-flash-bootstrap">{{ layout.flash_bootstrap_json|safe }}</script>
      {% block page %}{% endblock %}
      <rdrs-kb-help></rdrs-kb-help>
      <rdrs-kb-pending></rdrs-kb-pending>
  {% endblock %}
  ```

  Notes:
  - `title` lives in the per-route template's own struct field (stays as `&'static str` for compile-time guarantee).
  - `layout.theme`, `layout.git_version`, `layout.sidebar_bootstrap_json`, `layout.flash_bootstrap_json` come from the embedded `AppLayoutContext`.
  - The `pageshow` listener in `base.html` is shared with login/register; no need to duplicate here.

- [ ] **Step 4: Add `AppLayoutContext` struct + builder to `src/handlers/pages.rs`.**

  Add this block to `src/handlers/pages.rs` (suggested location: just above the existing `AppShellTemplate` definition near the bottom of the file):

  ```rust
  /// Shared layout fields embedded in every per-route logged-in
  /// template. Templates reference these as `{{ layout.<field> }}`.
  pub struct AppLayoutContext {
      pub theme: Option<String>,
      pub git_version: &'static str,
      pub sidebar_bootstrap_json: String,
      pub flash_bootstrap_json: String,
  }

  /// Build the shared layout context for a logged-in page response.
  /// Loads the user's theme, the sidebar tree (escaped for inline
  /// embedding), and the flash messages (also escaped).
  pub async fn build_app_layout(
      state: &AppState,
      auth_user: &PageAuthUser,
      flash: &Flash,
  ) -> AppLayoutContext {
      let user_id = auth_user.user.id;
      let theme = state
          .db
          .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
          .await
          .unwrap_or(None);

      let sidebar_bootstrap_json = sidebar_bootstrap_json(state, auth_user).await;
      let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

      AppLayoutContext {
          theme,
          git_version: crate::GIT_VERSION,
          sidebar_bootstrap_json,
          flash_bootstrap_json,
      }
  }
  ```

  No existing handler is modified in this task — they continue to construct `AppShellTemplate` as before.

- [ ] **Step 5: Add a unit test for `build_app_layout`.**

  Append to `tests/pages_test.rs` (locate the file and append at the end). Goal: confirm a freshly-built `AppLayoutContext` for a logged-in user produces a non-null sidebar JSON, an array (possibly empty) for flash, the configured theme, and `GIT_VERSION` set.

  First, read `tests/pages_test.rs` to see the existing test-server / authenticated-user helper pattern. Match it. The new test should follow the same setup style as existing logged-in handler tests and assert:

  ```rust
  // Pseudocode; adapt to existing helpers in pages_test.rs.
  let (server, user) = make_logged_in_test_server().await;
  let layout = rdrs::handlers::pages::build_app_layout(&state, &auth_user, &flash).await;
  assert_eq!(layout.git_version, rdrs::GIT_VERSION);
  // sidebar JSON is non-empty (at least "null" or an object literal)
  assert!(!layout.sidebar_bootstrap_json.is_empty());
  // flash JSON is "[]" for a fresh request
  assert_eq!(layout.flash_bootstrap_json, "[]");
  ```

  If the existing tests file uses `axum_test::TestServer` and only exercises HTTP paths (not direct calls to `pages::*` helpers), it may be simpler to render an existing handler under the new endpoint flow once Task 2 lands. In that case skip the unit test and rely on the integration-test rebuild that Tasks 2-4 force. Document the choice in the commit message.

- [ ] **Step 6: Compile + test.**

  Run: `cargo nextest run`
  Expected: all tests pass (we did not modify any existing route, so the existing 701 tests remain green; the new test, if added, brings the total to 702).

- [ ] **Step 7: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add static/js/app.js src/handlers/static_assets.rs templates/app_layout.html src/handlers/pages.rs tests/pages_test.rs
  git commit -m "$(cat <<'EOF'
  feat(ssr): add app.js + app_layout.html foundation

  Adds the shared client module (`swap()` helper + full-reload
  rdrsNavigate stub) and the new `app_layout.html` Askama template
  with its `AppLayoutContext` struct + `build_app_layout` helper.
  No existing route is migrated yet; `AppShellTemplate` stays alive
  through the route-migration tasks and is deleted in the teardown.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 2: Migrate 6 simple routes

Migrates `/settings`, `/user-settings`, `/admin`, `/statistics`, `/categories`, `/feeds` from `AppShellTemplate` to per-route templates extending `app_layout.html`. These are the simpler routes (no path parameters, single mode each).

**Per-route template shape** (use `settings.html` as the canonical example):

```html
{% extends "app_layout.html" %}

{% block page_script %}
    <script type="module" src="/static/js/pages/settings.js?v={{ layout.git_version }}"></script>
{% endblock %}

{% block page %}
    <rdrs-settings-page></rdrs-settings-page>
{% endblock %}
```

`title` is set via the existing `app_layout.html` template's `{% block title %}{{ title }} - RDRS{% endblock %}` — so each per-route struct needs a `pub title: &'static str` field.

**Per-route handler shape** (use `settings_page` as canonical):

```rust
#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub title: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        SettingsTemplate {
            title: "Settings",
            layout,
        },
    )
}
```

**Files:**
- Create: `templates/settings.html`, `templates/user_settings.html`, `templates/admin.html`, `templates/statistics.html`, `templates/categories.html`, `templates/feeds.html`
- Modify: `src/handlers/pages.rs` (add 6 per-route template structs, swap 6 handlers' return types and bodies)
- Modify: existing tests in `tests/handlers_test.rs`, `tests/pages_test.rs`, `tests/statistics_test.rs` if they assert on `AppShellTemplate` element_tag / script_path strings — switch assertions to literal element-tag substring (e.g. `<rdrs-settings-page>`).

- [ ] **Step 1: Search for existing test assertions that need updates.**

  Run from `/home/nixos/Develop/claude/rdrs`:
  ```bash
  grep -rn "rdrs-settings-page\|rdrs-user-settings-page\|rdrs-admin-page\|rdrs-statistics-page\|rdrs-categories-page\|rdrs-feeds-page" tests/
  grep -rn "/static/js/pages/settings.js\|/static/js/pages/user-settings.js\|/static/js/pages/admin.js\|/static/js/pages/statistics.js\|/static/js/pages/categories.js\|/static/js/pages/feeds.js" tests/
  ```
  These greps surface every existing test that asserts on the rendered HTML for the 6 routes. Make a list of the file:line locations — you'll touch them in Step 6.

- [ ] **Step 2: Write per-route templates.**

  Create the six files. Use the canonical shape above. Substitute the right `<rdrs-<name>-page>` tag and JS path for each:

  | File | element tag | script path |
  |------|-------------|-------------|
  | `templates/settings.html` | `rdrs-settings-page` | `/static/js/pages/settings.js` |
  | `templates/user_settings.html` | `rdrs-user-settings-page` | `/static/js/pages/user-settings.js` |
  | `templates/admin.html` | `rdrs-admin-page` | `/static/js/pages/admin.js` |
  | `templates/statistics.html` | `rdrs-statistics-page` | `/static/js/pages/statistics.js` |
  | `templates/categories.html` | `rdrs-categories-page` | `/static/js/pages/categories.js` |
  | `templates/feeds.html` | `rdrs-feeds-page` | `/static/js/pages/feeds.js` |

- [ ] **Step 3: Add 6 per-route template structs to `src/handlers/pages.rs`.**

  Just above the existing `settings_page` etc. handlers, add six structs. Title strings (used in browser tab) must match the exact strings the existing `AppShellTemplate.title` was using:

  | Struct | title literal |
  |--------|---------------|
  | `SettingsTemplate` | `"Settings"` |
  | `UserSettingsTemplate` | `"User Settings"` |
  | `AdminTemplate` | `"Admin Panel"` |
  | `StatisticsTemplate` | `"Statistics"` |
  | `CategoriesTemplate` | `"Categories"` |
  | `FeedsTemplate` | `"Feeds"` |

  Note: existing `AppShellTemplate.title` includes `" - RDRS"`. The new title block in `app_layout.html` adds `" - RDRS"` itself. So the per-route title is just the bare page name.

  Each struct gets the canonical `IntoResponse` impl shown above.

- [ ] **Step 4: Migrate each of the 6 handlers.**

  Each handler currently does the following work and returns `AppShellTemplate`:

  ```rust
  let user_id = auth_user.user.id;
  let theme = state.db.read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None)).await.unwrap_or(None);
  let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
  let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

  (flash, AppShellTemplate {
      title: "<Title> - RDRS",
      element_tag: "<rdrs-tag>",
      script_path: "/static/js/pages/<path>",
      theme,
      git_version: crate::GIT_VERSION,
      sidebar_bootstrap_json,
      flash_bootstrap_json,
  })
  ```

  Replace with:

  ```rust
  let layout = build_app_layout(&state, &auth_user, &flash).await;

  (flash, <Route>Template {
      title: "<Title>",
      layout,
  })
  ```

  Replace `auth_user: PageAuthUser` with `auth_user: PageAdminUser` ONLY for `admin_page` (it already uses `PageAdminUser`; keep that). The admin handler converts `admin: PageAdminUser` → `auth_user: PageAuthUser` before calling `sidebar_bootstrap_json`. Preserve that conversion when calling `build_app_layout` — wrap in the same `let auth_user = PageAuthUser { user: admin.user, session: admin.session };` and pass `&auth_user`.

  Apply this transformation to each of: `settings_page`, `user_settings_page`, `admin_page`, `statistics_page`, `categories_page`, `feeds_page`.

- [ ] **Step 5: Compile.**

  Run: `cargo build`
  Expected: success. If Askama complains about the new templates, common causes are missing `{% block %}` defaults in `app_layout.html` (Step 3 of Task 1) or wrong field names — fix and retry.

- [ ] **Step 6: Update existing test assertions.**

  Walk through the file:line list from Step 1. For each assertion that referenced `AppShellTemplate` shape (e.g. asserting on `element_tag="rdrs-settings-page"` or `title="Settings - RDRS"`), update to assert against the rendered HTML directly. Common transforms:

  - Old: `assert!(html.contains("element_tag=\"rdrs-settings-page\""))` (won't match because element_tag was a struct field, not in HTML).
  - New: `assert!(html.contains("<rdrs-settings-page>"))` — the literal element appears in the page block.
  - Old: `assert!(html.contains("/static/js/pages/settings.js"))` — keep this; the script path still appears in HTML, just inside `{% block page_script %}` rather than the shell template.
  - Old: `assert!(html.contains("Settings - RDRS"))` — keep; title still includes `" - RDRS"` from `app_layout.html`'s title block.

  Most assertions transfer over with no edit because they were already asserting on rendered HTML.

- [ ] **Step 7: Run the test suite.**

  Run: `cargo nextest run`
  Expected: all tests pass.

  If specific tests fail, read the failure carefully. The most likely classes:
  - `Title text mismatch` — verify per-route title literal exactly matches the original `AppShellTemplate.title` minus `" - RDRS"`.
  - `Element tag missing in rendered HTML` — verify the per-route template puts `<rdrs-X-page>` inside `{% block page %}`.
  - `Script path missing` — verify `{% block page_script %}` was filled.

- [ ] **Step 8: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add templates/settings.html templates/user_settings.html templates/admin.html templates/statistics.html templates/categories.html templates/feeds.html src/handlers/pages.rs tests/
  git commit -m "$(cat <<'EOF'
  feat(ssr): migrate 6 simple routes to per-route templates

  Routes migrated: /settings, /user-settings, /admin, /statistics,
  /categories, /feeds. Each gets a thin Askama template extending
  the new app_layout.html, plus a per-route template struct
  embedding AppLayoutContext. AppShellTemplate stays alive — used
  by the entries-family routes until Task 3 lands and removed in
  Task 4.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

  Note: `git add tests/` is fine here since the only test changes are the ones you made. If `git status` shows unrelated files, stage explicitly.

---

## Task 3: Migrate 8 entries-family routes

Same pattern as Task 2, applied to 8 routes: `/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`, `/feeds/{id}/entries`, `/categories/{id}/entries`, `/search`. (`feed_entries` and `category_entries` have a Path extractor; the rest don't.)

**All 8 entries-family routes share the SAME element tag (`<rdrs-entries-page>`) and SAME script (`/static/js/pages/entries.js`).** Each per-route template can technically embed the same body, but we still create 8 separate template files so per-route SSR content can land independently in PR-3+.

**Files:**
- Create: `templates/{unread,entries,read_entries,starred_entries,summarized_entries,feed_entries,category_entries,search}.html`
- Modify: `src/handlers/pages.rs` (8 per-route structs + 8 handler bodies)
- Modify: relevant tests

- [ ] **Step 1: Search for existing test assertions.**

  Run:
  ```bash
  grep -rn "rdrs-entries-page" tests/
  grep -rn "/static/js/pages/entries.js" tests/
  ```
  Make a list of file:line locations.

- [ ] **Step 2: Write per-route templates.**

  Create 8 files. Each is structurally identical:

  ```html
  {% extends "app_layout.html" %}

  {% block page_script %}
      <script type="module" src="/static/js/pages/entries.js?v={{ layout.git_version }}"></script>
  {% endblock %}

  {% block page %}
      <rdrs-entries-page></rdrs-entries-page>
  {% endblock %}
  ```

  Save as: `templates/unread.html`, `templates/entries.html`, `templates/read_entries.html`, `templates/starred_entries.html`, `templates/summarized_entries.html`, `templates/feed_entries.html`, `templates/category_entries.html`, `templates/search.html`.

- [ ] **Step 3: Add 8 per-route template structs.**

  Add to `src/handlers/pages.rs`. Title literals (matching the originals minus `" - RDRS"`):

  | Struct | template path | title |
  |--------|---------------|-------|
  | `UnreadTemplate` | `"unread.html"` | `"Unread"` |
  | `EntriesTemplate` | `"entries.html"` | `"Entries"` |
  | `ReadEntriesTemplate` | `"read_entries.html"` | `"Read Entries"` |
  | `StarredEntriesTemplate` | `"starred_entries.html"` | `"Starred Entries"` |
  | `SummarizedEntriesTemplate` | `"summarized_entries.html"` | `"Summarized Entries"` |
  | `FeedEntriesTemplate` | `"feed_entries.html"` | `"Feed Entries"` |
  | `CategoryEntriesTemplate` | `"category_entries.html"` | `"Category Entries"` |
  | `SearchTemplate` | `"search.html"` | `"Search"` |

  Each struct has the same shape as Task 2's:

  ```rust
  #[derive(Template)]
  #[template(path = "unread.html")]
  pub struct UnreadTemplate {
      pub title: &'static str,
      pub layout: AppLayoutContext,
  }
  // … and IntoResponse impl
  ```

- [ ] **Step 4: Migrate the 8 handlers.**

  - `unread_page`, `entries_page`, `read_entries_page`, `starred_entries_page`, `summarized_entries_page`, `search_page` — straightforward swap (no Path extractor):

    ```rust
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    (flash, UnreadTemplate { title: "Unread", layout })
    ```

  - `feed_entries_page` and `category_entries_page` — preserve the existing ownership-check `Result<(Flash, ...), AppError>` and `Path(id)` extractor. Replace just the `AppShellTemplate { … }` literal with the per-route template:

    ```rust
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    Ok((flash, FeedEntriesTemplate { title: "Feed Entries", layout }))
    ```

    The DB ownership check in these two handlers stays exactly as it is — only the response type changes.

- [ ] **Step 5: Compile.**

  Run: `cargo build`
  Expected: success.

- [ ] **Step 6: Update existing test assertions.**

  Walk through the file:line list from Step 1. Same transforms as Task 2 Step 6.

- [ ] **Step 7: Run tests.**

  Run: `cargo nextest run`
  Expected: all tests pass.

- [ ] **Step 8: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add templates/unread.html templates/entries.html templates/read_entries.html templates/starred_entries.html templates/summarized_entries.html templates/feed_entries.html templates/category_entries.html templates/search.html src/handlers/pages.rs tests/
  git commit -m "$(cat <<'EOF'
  feat(ssr): migrate entries-family routes to per-route templates

  Routes migrated: /, /entries, /entries/{read,starred,summarized},
  /feeds/{id}/entries, /categories/{id}/entries, /search. Each
  gets its own Askama template extending app_layout.html — bodies
  are structurally identical for now (all mount
  <rdrs-entries-page>), but separate files let PR-3+ land per-route
  SSR content independently.

  Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 4: Teardown — slim base.html, delete app_shell.html, delete router.js

All 13 routes now use per-route templates. `AppShellTemplate` and `app_shell.html` no longer have any consumer. Remove them. Slim `base.html` so login/register stop loading logged-in JS. Delete the SPA router.

**Files:**
- Modify: `templates/base.html` (slim — remove logged-in JS imports)
- Delete: `templates/app_shell.html`
- Modify: `src/handlers/pages.rs` (delete `AppShellTemplate` struct + its `IntoResponse` impl)
- Delete: `static/js/router.js`
- Modify: `src/handlers/static_assets.rs` (remove `js/router.js` allowlist entry)

- [ ] **Step 1: Slim `templates/base.html`.**

  Read the file. Currently it contains script imports for the entire CSR custom-element fleet. Remove every `<script type="module" src="...">` line EXCEPT `rdrs-flash.js` (login/register depend on `window.flash.redirect`). The kept imports + inline scripts are: theme controller, sidebar mobile toggle helpers, `rdrs-flash.js`, the `pageshow` listener.

  After this step, `base.html` is the minimal pre-login shell. Logged-in chrome lives entirely in `app_layout.html`.

  Concretely, `base.html`'s `<head>` should look like (preserving the existing inline `<script>` content):

  ```html
  <head>
      <meta charset="UTF-8">
      <meta name="viewport" content="width=device-width, initial-scale=1.0">
      <title>{% block title %}RDRS{% endblock %}</title>
      <link rel="icon" href="/favicon.ico" sizes="32x32">
      <link rel="icon" href="/favicon.svg" type="image/svg+xml">
      <link rel="apple-touch-icon" href="/apple-touch-icon.png">
      <link rel="preconnect" href="https://fonts.bunny.net">
      <link href="https://fonts.bunny.net/css2?…&display=swap" rel="stylesheet">
      <link rel="stylesheet" href="/static/css/app.css?v={{ git_version }}">
      <script>
          // (existing inline theme + sidebar toggle scripts — keep verbatim)
      </script>
      <script type="module" src="/static/js/components/rdrs-flash.js?v={{ git_version }}"></script>
      {% block head %}{% endblock %}
  </head>
  ```

  Removed lines:
  - `<script type="module" src="/static/js/components/rdrs-kb-pending.js?v={{ git_version }}"></script>`
  - `<script type="module" src="/static/js/components/rdrs-kb-help.js?v={{ git_version }}"></script>`
  - `<script type="module" src="/static/js/keyboard.js?v={{ git_version }}"></script>`
  - `<script type="module" src="/static/js/components/rdrs-entry-list.js?v={{ git_version }}"></script>`

  These imports now live in `app_layout.html` (added in Task 1 Step 3).

- [ ] **Step 2: Delete `AppShellTemplate` struct from `src/handlers/pages.rs`.**

  This MUST run BEFORE deleting `app_shell.html` — the struct's `#[template(path = "app_shell.html")]` resolves the template at compile time, so leaving the struct after deleting the file would break the build.

  Locate the `AppShellTemplate` struct definition + its `IntoResponse` impl + the doc comment immediately above. Delete the entire block. Verify no handler still references `AppShellTemplate` (Tasks 2 and 3 should have rewired them all):

  ```bash
  grep -n "AppShellTemplate" src/
  ```

  Expected after this step: zero matches.

- [ ] **Step 3: Delete `templates/app_shell.html`.**

  Now safe to remove the template file (Step 2 deleted its only consumer).

  Run:
  ```bash
  git rm templates/app_shell.html
  ```

- [ ] **Step 4: Remove `js/router.js` from the static-assets allowlist.**

  This MUST run BEFORE deleting `router.js` — the allowlist entry uses `include_str!("../../static/js/router.js")` which resolves at compile time, so leaving the entry after deleting the file would break the build.

  Edit `src/handlers/static_assets.rs`. Remove this line from `FILES`:
  ```rust
      ("js/router.js", include_str!("../../static/js/router.js")),
  ```

- [ ] **Step 5: Delete `static/js/router.js`.**

  Now safe to remove (Step 4 dropped the only `include_str!` reference).

  Run:
  ```bash
  git rm static/js/router.js
  ```

- [ ] **Step 6: Verify no template still imports `router.js`.**

  Run:
  ```bash
  grep -rn "router.js" templates/ static/ src/
  ```

  Expected: zero matches under `templates/` and `static/` (one or two README references in `src/` are fine if they're explanatory comments, but should ideally be cleaned up for clarity — leave them only if removing would be confusing without context).

  If any per-route template you created in Tasks 2-3 accidentally references `router.js`, remove it now.

- [ ] **Step 7: Compile + test.**

  Run: `cargo nextest run`
  Expected: all tests pass.

  If existing tests fail because they asserted on `router.js` being served at `/static/js/router.js`, remove those assertions — the route should now return 404 for `/static/js/router.js` and that's correct.

- [ ] **Step 8: Hand-verify login/register still render.**

  Run a brief sanity check via `axum-test`-driven snippet OR add an integration test if not already present. The minimum you need to confirm:
  - `GET /login` returns 200 with HTML containing the login form.
  - `GET /register` returns 200 with HTML containing the register form.
  - Neither HTML response contains `kb-pending.js`, `kb-help.js`, `keyboard.js`, `entry-list.js` references (a slim base means smaller pre-login JS).

  If existing tests already cover these, just run them. If not, add a small `tests/auth_test.rs` or `tests/pages_test.rs` snippet.

- [ ] **Step 9: Format and commit.**

  Run: `cargo fmt`
  Then:
  ```bash
  git add templates/base.html src/handlers/pages.rs src/handlers/static_assets.rs tests/
  git commit -m "$(cat <<'EOF'
  refactor(ssr): teardown CSR shell — drop app_shell.html + SPA router

  - templates/base.html slimmed to the pre-login shell (only
    rdrs-flash.js retained for login/register's flash.redirect).
  - templates/app_shell.html, AppShellTemplate struct removed —
    all 13 routes now use per-route templates extending
    app_layout.html.
  - static/js/router.js + its static-assets entry removed.
    Existing CSR call sites of window.rdrsNavigate fall back to
    the full-reload stub in app.js.

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

  Run: `git push -u origin feat/ssr-shell-teardown`

- [ ] **Open the PR.**

  ```bash
  gh pr create --title "refactor(ssr): SSR-first PR-2 — shell teardown" --body "$(cat <<'EOF'
  ## Summary
  - Add `static/js/app.js` shipping the partial-swap helper plus a full-reload `window.rdrsNavigate` stub.
  - Add `templates/app_layout.html` + `AppLayoutContext` Rust struct as the new logged-in render shell.
  - Migrate all 13 logged-in routes to per-route Askama templates (`unread.html`, `entries.html`, …) extending `app_layout.html`. Each handler now returns its own per-route template struct embedding `AppLayoutContext`.
  - Slim `templates/base.html` to the pre-login shell. Delete `templates/app_shell.html`, `AppShellTemplate`, and `static/js/router.js`.
  - SPA navigation is gone — every link now full-reloads. `window.rdrsNavigate` calls in existing CSR code fall through to `location.href` via the new stub; per-page PRs delete those call sites as they migrate to SSR.

  Spec: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md`
  Plan: `docs/superpowers/plans/2026-05-08-ssr-first-pr2-shell-teardown.md`

  ## Test plan
  - [x] `cargo nextest run` — full suite green.
  - [x] Per-route handler tests pass; assertions on rendered HTML (element tag + script path) preserved.
  - [x] Login + register still render with slim pre-login JS (no kb-pending / kb-help / keyboard / entry-list imports).

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Subsequent PRs

Per the spec migration table, PR-3 is `/settings` SSR — the first per-route content migration. From PR-3 onward each plan replaces a single per-route template body with real SSR content + adds fragment endpoints + grows `app.js` with the consumer-specific JS (keyboard, sidebar polling, etc.). Each plan lives at `docs/superpowers/plans/2026-MM-DD-ssr-first-prN-<topic>.md`.
