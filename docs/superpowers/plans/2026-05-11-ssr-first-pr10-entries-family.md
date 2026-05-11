# SSR-first PR-10: Entries Family Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the 5 entries-family pages (`/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`) from CSR shell (`<rdrs-entries-page>` mode dispatch in `static/js/pages/entries.js`) to direct SSR + a two-pane layout where the reading pane is swapped in via `app.js`'s `swap()` helper. Adds 6 fragment endpoints (`/entries/{id}/fragment`, `POST /entries/{id}/star`, `POST /entries/{id}/read`, `POST /entries/{id}/summarize`, `GET /entries?after={cursor}&fragment=1` for Load-More, `GET /sidebar/unread` for polling). Extends `app.js` with a sidebar-polling section and a minimal keyboard section (j/k/o/s/space) targeting the new row markup.

PR-11 (next) migrates `/feeds/{id}/entries` + `/categories/{id}/entries`. PR-12 deletes `entries.js`, `rdrs-entry-list.js`, the old keyboard.js, and the now-unused JSON endpoints (`/api/entries/{id}`, `/api/feeds`, the GReader edit-tag consumer paths used only by `<rdrs-entry-list>`).

**Architecture:** Five SSR handlers share one helper (`build_entries_page`) that takes an `EntryFilter` + sort order, fetches a page from `entry::list_by_user`, builds row view-models, and renders. The 5 page templates extend a new shared `_entries_layout.html`. Fragment endpoints reuse `_entry_row.html` / `_reading_pane.html` / `_sidebar_unread.html` partials so the markup has a single source of truth. Star/read actions return a multi-target response containing both the swapped row and the swapped sidebar-unread block — invalidating both views in one round trip. Load-More uses OFFSET pagination (matches PR-9 `/search`) for now; composite-cursor migration is deferred.

**Tech Stack:** Rust + Axum + Askama 0.15. `entry::list_by_user`, `entry::EntryFilter`, `entry::find_by_id_for_user`, and the existing summarize pipeline (`summary_cache` + `summary_tx`) are reused as-is. `<rdrs-sidebar>` chrome stays. `app.js` gets two new sections.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md` (Migration Plan step 10, plus the "Endpoints" + "Data flow examples" sections that fix wire format).

**Branch:** `feat/ssr-entries-family` (already created off updated `main` at commit `393b475`).

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git status && git branch --show-current && git log -3 --oneline`
  Expected: branch `feat/ssr-entries-family`, working tree clean modulo untracked `test-results/`, latest commit on main is `393b475 feat(ssr): SSR-first PR-9 — /search page (#195)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: post-PR-9 baseline pass (~700+ tests).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `templates/_entry_row.html` | One row in the entries list: title link with `data-swap="#reading-pane"`, feed icon, feed title, published time, star/read inline forms with `data-swap` multi-target. |
| Create | `templates/_reading_pane.html` | Reading pane content: title, link-out, author + feed + published, body content (sanitized), actions (summarize, save). |
| Create | `templates/_sidebar_unread.html` | Sidebar polling target: just the unread-count subtree (categories + feeds with unread numbers). |
| Create | `templates/_entries_layout.html` | Shared two-pane shell for the 5 entries-family pages. Extends `app_layout.html`. Includes the entry list + reading pane + (optional) Load-More form. |
| Modify | `templates/unread.html` | Use `_entries_layout.html` shell; pass title + filter description. |
| Modify | `templates/entries.html` | Same. |
| Modify | `templates/read_entries.html` | Same. |
| Modify | `templates/starred_entries.html` | Same. |
| Modify | `templates/summarized_entries.html` | Same. |
| Modify | `src/handlers/pages.rs` | Replace 5 CSR-shell handlers with SSR handlers sharing a `build_entries_page(...)` helper. Extend the 5 `*Template` structs with `entries: Vec<EntryRowView>` + `reading_pane: ReadingPaneView` + `mode: &'static str` + `next_cursor: Option<i64>` + `entries_layout: EntriesLayoutContext`. |
| Create | `src/handlers/entries.rs` | New per-resource module for the 6 fragment endpoints. Handlers: `entry_fragment`, `star_entry_form`, `read_entry_form`, `summarize_entry_form`, `entries_load_more_fragment`, `sidebar_unread_fragment`. |
| Modify | `src/lib.rs` | Register 6 new routes: `GET /entries/{id}/fragment`, `POST /entries/{id}/star`, `POST /entries/{id}/read`, `POST /entries/{id}/summarize`, `GET /sidebar/unread`. Note `/entries?after=...&fragment=1` overloads the existing `GET /entries` page handler — dispatched in `entries_page`. |
| Modify | `static/js/app.js` | Add two sections: sidebar polling (`setInterval(20s)` → fetch `/sidebar/unread` → swap into `#sidebar-unread`); minimal keyboard shortcuts j/k/o/s/space wired against `[data-entry-row]` rows on entries-family pages. |
| Modify | `tests/pages_test.rs` | Replace CSR-shell assertions in `test_unread_page_*`, `test_entries_page_*`, `test_read_entries_page_*`, `test_starred_entries_page_*`, `test_summarized_entries_page_*` with SSR content asserts. Add integration tests that seed feeds+entries and assert rows appear. |
| Modify | `tests/handlers_test.rs` | Add tests for the 6 fragment endpoints (positive + 404/403 paths). |
| Modify | `templates/feed_entries.html`, `templates/category_entries.html` | UNCHANGED in PR-10. They remain CSR shells (PR-11 will migrate them). |

**Endpoints kept** (still consumed by PR-11 CSR routes or external clients): `GET /api/entries/{id}` (`<rdrs-entry-list>` deep link); `GET /api/feeds` (feed-icon column on PR-11 CSR routes); all `/reader/api/0/*` GReader paths; `GET /api/feeds/{id}/icon` (referenced from `<img src="…">`).

**Endpoints NOT removed in PR-10:** Per the spec, `/api/entries/{id}/save`, `/api/entries/{id}/fetch-full-content`, `/api/entries/{id}/summary`, `/api/entries/{id}/neighbors`, `/api/entries/{id}/summarize` (POST) stay — the SSR reading-pane uses some of them via the `data-swap` action endpoints we add (which internally call the same model functions). The legacy JSON endpoints stay alive until their last consumer dies in PR-11/PR-12. Document this in `lib.rs` comments.

---

## Task 1: Shared partials + SSR `/` (unread)

This task establishes the row + reading-pane + entries-layout primitives and migrates the first page. The remaining 4 routes in Task 2 are mechanical reuses.

### Steps

- [ ] **Step 1: Confirm referenced fields and helper signatures.**

  Read these files at the start of the task. They establish what fields the row + reading-pane view-models can populate from:
  - `src/models/entry.rs` lines 19-95 — `Entry`, `EntryWithFeed`, `EntryFilter`, `ContinuationCursor`.
  - `src/models/entry.rs` lines 257-311 — `list_by_user` signature, `EntrySortOrder` enum.
  - `src/models/entry.rs` — `find_by_id_for_user` (or whatever the per-user single-entry fetch is); confirm name + signature; verify it returns `EntryWithFeed`.
  - `src/handlers/entry.rs` lines 406-474 — `get_entry_detail` and `EntryDetailResponse` (the JSON shape the new fragment endpoint replaces; reuse the same field selection).
  - `src/handlers/pages.rs` near line 220 for `build_app_layout(...)` + `PageAuthUser` + the per-template `git_version` + `layout` + `title` convention.
  - `src/utils/time.rs` for `format_relative_time` (used by PR-9 search).
  - `src/services/summary_cache.rs` lines 82-149 for the SummaryStatus enum + the cache API the reading-pane needs.

  **NOTE the verify list — do NOT proceed with the View struct names below if any field disagrees with the model.** If `find_by_id_for_user` doesn't exist, add it as a small helper in `src/models/entry.rs` near `list_by_user` (`SELECT … WHERE entries.id = ? AND <user-ownership join>` — copy the same join boilerplate from `list_by_user`).

- [ ] **Step 2: Write failing integration test for SSR `/`.**

  Append to `tests/pages_test.rs`. Drop the existing `test_unread_page_returns_shell` next-task; this test takes its place.

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_unread_page_renders_entry_rows() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "Tech").await;
      let feed = env.create_feed(user.id, cat.id, "https://blog.example/feed").await;
      env.create_entry(feed.id, "Entry One", "https://blog.example/one", true).await; // unread
      env.create_entry(feed.id, "Entry Two", "https://blog.example/two", true).await; // unread
      let read_one = env.create_entry(feed.id, "Read Already", "https://blog.example/three", true).await;
      env.mark_read(user.id, read_one.id).await;

      let response = env.get_as(&user, "/").await;
      assert_eq!(response.status(), 200);
      let html = response.text().await.unwrap();

      // SSR rows present (drop CSR-shell assertions)
      assert!(!html.contains("<rdrs-entries-page"), "shell should be gone");
      assert!(html.contains("data-entry-row"), "rows should be SSR'd");
      assert!(html.contains("Entry One"), "unread entry should appear");
      assert!(html.contains("Entry Two"), "unread entry should appear");
      assert!(!html.contains("Read Already"), "read entries should be filtered out on /");

      // Reading pane placeholder + swap target
      assert!(html.contains(r#"id="reading-pane""#));
      assert!(html.contains("Select an entry"));
  }
  ```

  Helpers `create_user / create_category / create_feed / create_entry / mark_read / get_as` should exist in `tests/common/` (or wherever the test fixtures live). Check `tests/pages_test.rs` top for the existing helper pattern and reuse / extend.

  **VERIFY:** Run `cargo nextest run --test pages_test test_unread_page_renders_entry_rows`. Expect: FAIL because `<rdrs-entries-page>` still in the shell template.

- [ ] **Step 3: Create `templates/_entry_row.html`.**

  ```html
  {# Single entry row. Receives `EntryRowView` via the surrounding `for r in entries` loop.
     The form-action endpoints accept either POST with no body (toggle) or POST `unread=1`/`unstar=1` to force-set. #}
  <article id="entry-row-{{ r.id }}" class="entry-row{% if r.is_read %} entry-row-read{% endif %}{% if r.is_starred %} entry-row-starred{% endif %}" data-entry-row data-entry-id="{{ r.id }}">
      <a class="entry-row-title" href="/entries/{{ r.id }}/fragment" data-swap="#reading-pane">
          {% if r.feed_has_icon %}<img class="entry-row-feed-icon" src="/api/feeds/{{ r.feed_id }}/icon" alt="" loading="lazy" width="16" height="16">{% endif %}
          <span class="entry-row-feed">{{ r.feed_title }}</span>
          <span class="entry-row-headline">{{ r.title }}</span>
          <time class="entry-row-time" datetime="{{ r.published_at_iso }}">{{ r.published_relative }}</time>
      </a>
      <div class="entry-row-actions">
          <form method="post" action="/entries/{{ r.id }}/star" data-swap="#entry-row-{{ r.id }}" class="entry-row-action">
              <button type="submit" aria-label="{% if r.is_starred %}Unstar{% else %}Star{% endif %}" data-testid="star-btn-{{ r.id }}">{% if r.is_starred %}★{% else %}☆{% endif %}</button>
          </form>
          <form method="post" action="/entries/{{ r.id }}/read" data-swap="#entry-row-{{ r.id }}" class="entry-row-action">
              <button type="submit" aria-label="{% if r.is_read %}Mark unread{% else %}Mark read{% endif %}" data-testid="read-btn-{{ r.id }}">{% if r.is_read %}●{% else %}○{% endif %}</button>
          </form>
      </div>
  </article>
  ```

  Note: `data-swap="#entry-row-{{ r.id }}"` is the row-level default target, but the server response will use multi-target `<template data-swap-target>` blocks to also swap `#sidebar-unread`. The `data-swap` attribute on the form is required for `app.js` to intercept; the actual target list comes from the response.

- [ ] **Step 4: Create `templates/_reading_pane.html`.**

  ```html
  {# Reading pane content. `r` is `ReadingPaneView` (Option-like via render-time absence).
     When no entry selected the pane is a placeholder rendered in `_entries_layout.html` instead. #}
  <article id="reading-pane" class="reading-pane">
      <header class="reading-pane-header">
          <h1 class="reading-pane-title">
              {% if let Some(link) = pane.link %}<a href="{{ link }}" target="_blank" rel="noopener noreferrer">{{ pane.title }}</a>{% else %}{{ pane.title }}{% endif %}
          </h1>
          <div class="reading-pane-meta">
              <span class="muted">{{ pane.feed_title }}</span>
              {% if let Some(author) = pane.author %} &middot; <span class="muted">{{ author }}</span>{% endif %}
              {% if let Some(ts) = pane.published_at_iso %} &middot; <time datetime="{{ ts }}" class="muted">{{ pane.published_relative }}</time>{% endif %}
          </div>
          <div class="reading-pane-actions">
              <form method="post" action="/entries/{{ pane.id }}/star" data-swap="#entry-row-{{ pane.id }}" class="reading-pane-action">
                  <button type="submit">{% if pane.is_starred %}Unstar{% else %}Star{% endif %}</button>
              </form>
              <form method="post" action="/entries/{{ pane.id }}/read" data-swap="#entry-row-{{ pane.id }}" class="reading-pane-action">
                  <button type="submit">{% if pane.is_read %}Mark unread{% else %}Mark read{% endif %}</button>
              </form>
              <form method="post" action="/entries/{{ pane.id }}/summarize" data-swap="#reading-pane" class="reading-pane-action">
                  <button type="submit" {% if pane.summary_in_flight %}disabled{% endif %}>Summarize</button>
              </form>
          </div>
      </header>
      {% if let Some(summary) = pane.summary_text %}
          <section class="reading-pane-summary"><h2>Summary</h2><div>{{ summary|safe }}</div></section>
      {% endif %}
      <section class="reading-pane-body">{{ pane.content_html|safe }}</section>
  </article>
  ```

  Note: when star/read is toggled from the reading pane, the form points at `#entry-row-{id}` (the list row), not the pane. The server still emits the multi-target response so the row + sidebar both update. The pane itself does not need to re-render on a simple star toggle.

- [ ] **Step 5: Create `templates/_sidebar_unread.html`.**

  Skinny: just the counts. The full sidebar tree stays in the `<rdrs-sidebar>` custom element for now (it's still loaded by `base.html` per the spec, kept until PR-12). Poll output is a small element the client swaps into `#sidebar-unread`.

  Decision: render the same sidebar JSON shape the existing `<rdrs-sidebar>` uses, but as an HTML snippet of `<span data-feed-id="N" data-count="K">K</span>` entries that the existing `<rdrs-sidebar>` already knows how to apply, OR — simpler — render a single hidden `<script type="application/json" id="rdrs-sidebar-unread">…</script>` block that the sidebar element subscribes to.

  **Simplest first.** For PR-10, the polling endpoint just returns a JSON payload identical to the `/api/sidebar` slice but as an HTML container so swap() can apply it. The existing `<rdrs-sidebar>` reads from a global event; we'll dispatch the event from `app.js` after parsing the swapped JSON. (See Task 8 for the wire-up.)

  ```html
  {# Sidebar unread polling payload. The container element holds the JSON
     in a `data-payload` attribute so the swap helper can replace by
     selector and `app.js` can re-emit a custom event for `<rdrs-sidebar>`. #}
  <div id="sidebar-unread" data-payload='{{ payload_json|safe }}' hidden></div>
  ```

  `payload_json` is a JSON-encoded `Vec<UnreadCount>` from a new helper (see Task 7 for impl). For now the template just renders; the wire-up is in Task 7+8.

- [ ] **Step 6: Create `templates/_entries_layout.html`.**

  ```html
  {% extends "app_layout.html" %}

  {% block page %}
      <div class="app-layout">
          <rdrs-sidebar active="{{ entries_layout.active }}"></rdrs-sidebar>
          <main class="main-content">
              <div class="split-view">
                  <div class="list-pane">
                      <div class="list-pane-header">
                          <rdrs-flash class="flash-container"></rdrs-flash>
                          <h1 class="page-title">{{ title }}</h1>
                          {% if let Some(desc) = entries_layout.description %}<p class="muted">{{ desc }}</p>{% endif %}
                      </div>
                      <div class="list-pane-body" data-entries-list>
                          {% if entries.is_empty() %}
                              <p class="muted">{{ entries_layout.empty_message }}</p>
                          {% else %}
                              {% for r in entries %}{% include "_entry_row.html" %}{% endfor %}
                              {% if let Some(after) = next_cursor %}
                                  <form id="load-more" method="get" action="{{ entries_layout.path }}" data-swap="#load-more" class="load-more-form">
                                      <input type="hidden" name="after" value="{{ after }}">
                                      <input type="hidden" name="fragment" value="1">
                                      <button type="submit" data-testid="load-more-btn">Load more</button>
                                  </form>
                              {% endif %}
                          {% endif %}
                      </div>
                  </div>
                  <div class="reading-pane-host">
                      {% if let Some(pane) = reading_pane %}{% include "_reading_pane.html" %}{% else %}
                          <div id="reading-pane" class="reading-pane reading-pane-empty"><p class="muted">Select an entry to read.</p></div>
                      {% endif %}
                  </div>
              </div>
          </main>
      </div>
  {% endblock %}
  ```

  `entries_layout` is an `EntriesLayoutContext` struct providing `active: &'static str`, `description: Option<String>`, `empty_message: &'static str`, `path: &'static str`. The placeholder reading pane id is `reading-pane` so swap-target alignment works.

  **Askama if-let-include caveat:** the `{% if let Some(pane) = reading_pane %}{% include "_reading_pane.html" %}{% endif %}` form depends on the included template using the name `pane`. Confirm Askama 0.15 propagates outer scope into includes; if not, fall back to `{% match reading_pane %} ... {% endmatch %}` or move the partial into a `{% block %}` definition inside `_entries_layout.html`.

- [ ] **Step 7: Update `templates/unread.html` to use the shared layout.**

  ```html
  {% extends "_entries_layout.html" %}
  ```

  Just one line — the page template provides nothing extra; all context comes from the handler.

- [ ] **Step 8: Add `EntryRowView`, `ReadingPaneView`, `EntriesLayoutContext` structs + `build_entries_page` helper to `src/handlers/pages.rs`.**

  Place them near the top of the file alongside existing view structs. Type definitions:

  ```rust
  #[derive(Debug, Clone)]
  pub struct EntryRowView {
      pub id: i64,
      pub feed_id: i64,
      pub feed_title: String,
      pub feed_has_icon: bool,
      pub title: String,
      pub published_at_iso: String,
      pub published_relative: String,
      pub is_read: bool,
      pub is_starred: bool,
  }

  #[derive(Debug, Clone)]
  pub struct ReadingPaneView {
      pub id: i64,
      pub title: String,
      pub link: Option<String>,
      pub feed_title: String,
      pub author: Option<String>,
      pub published_at_iso: Option<String>,
      pub published_relative: String,
      pub content_html: String,
      pub is_read: bool,
      pub is_starred: bool,
      pub summary_text: Option<String>,
      pub summary_in_flight: bool,
  }

  #[derive(Debug, Clone)]
  pub struct EntriesLayoutContext {
      pub active: &'static str,
      pub description: Option<String>,
      pub empty_message: &'static str,
      pub path: &'static str,
  }

  fn row_view_from(e: &entry::EntryWithFeed) -> EntryRowView {
      let title = e.entry.title.clone().unwrap_or_else(|| "(no title)".to_string());
      let published_at = e.entry.published_at.unwrap_or(e.entry.created_at);
      EntryRowView {
          id: e.entry.id,
          feed_id: e.entry.feed_id,
          feed_title: e.feed_title.clone().unwrap_or_else(|| "(no feed)".to_string()),
          feed_has_icon: e.feed_has_icon,
          title,
          published_at_iso: published_at.to_rfc3339(),
          published_relative: format_relative_time(Some(published_at)).0,
          is_read: e.entry.read_at.is_some(),
          is_starred: e.entry.starred_at.is_some(),
      }
  }

  pub(crate) async fn build_entries_page(
      state: &AppState,
      user_id: i64,
      filter: entry::EntryFilter,
      sort: entry::EntrySortOrder,
      page_size: i64,
      offset: i64,
  ) -> (Vec<EntryRowView>, Option<i64>) {
      let rows = state
          .db
          .read_user(move |conn| {
              let entries = entry::list_by_user(conn, user_id, &filter, sort, page_size + 1, offset)?;
              Ok::<_, AppError>(entries)
          })
          .await
          .ok()
          .and_then(|r| r.ok())
          .unwrap_or_default();
      let next_cursor = if rows.len() as i64 > page_size {
          Some(offset + page_size)
      } else {
          None
      };
      let views = rows.into_iter().take(page_size as usize).map(|e| row_view_from(&e)).collect();
      (views, next_cursor)
  }
  ```

  `page_size = 50` is the call-site default.

- [ ] **Step 9: Rewrite `unread_page` handler.**

  Replace lines 235-252 of `src/handlers/pages.rs`:

  ```rust
  pub async fn unread_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      flash: Flash,
  ) -> (Flash, UnreadTemplate) {
      let layout = build_app_layout(&state, &auth_user, &flash).await;
      let user_id = auth_user.user.id;
      let (entries, next_cursor) = build_entries_page(
          &state,
          user_id,
          entry::EntryFilter { unread_only: true, ..Default::default() },
          entry::EntrySortOrder::PublishedAt,
          50,
          0,
      )
      .await;

      (
          flash,
          UnreadTemplate {
              title: "Unread",
              git_version: crate::GIT_VERSION,
              layout,
              entries,
              reading_pane: None,
              next_cursor,
              entries_layout: EntriesLayoutContext {
                  active: "unread",
                  description: None,
                  empty_message: "No unread entries — nice work.",
                  path: "/",
              },
          },
      )
  }
  ```

  Extend `UnreadTemplate` to add: `entries: Vec<EntryRowView>`, `reading_pane: Option<ReadingPaneView>`, `next_cursor: Option<i64>`, `entries_layout: EntriesLayoutContext`. Keep `title`, `git_version`, `layout` (the existing leaf-struct fields the base template still references).

- [ ] **Step 10: Re-run the failing test from Step 2; expect PASS.**

  Run: `cargo nextest run --test pages_test test_unread_page_renders_entry_rows`
  Expected: PASS.

  If the test fails on `<rdrs-entries-page>` still appearing, recheck `templates/unread.html` — should be ONLY `{% extends "_entries_layout.html" %}`.

- [ ] **Step 11: Run full suite.**

  Run: `cargo nextest run`
  Expected: post-PR-9 baseline +1 new test passing. Existing CSR-shell tests for the other 4 entries pages still pass (they assert shell content; we haven't migrated those yet).

- [ ] **Step 12: Commit.**

  ```bash
  cargo fmt
  git add templates/_entry_row.html templates/_reading_pane.html templates/_sidebar_unread.html templates/_entries_layout.html templates/unread.html src/handlers/pages.rs tests/pages_test.rs
  git commit -m "feat(ssr): PR-10 T1 — SSR / (unread) + shared entries partials"
  ```

  (Models edits, if any, go in this commit too.)

---

## Task 2: SSR remaining 4 routes (`/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`)

Mechanical replication of Task 1's pattern.

### Steps

- [ ] **Step 1: Update integration tests in `tests/pages_test.rs`.**

  Replace each of `test_entries_page_*`, `test_read_entries_page_*`, `test_starred_entries_page_*`, `test_summarized_entries_page_*` to assert SSR row content. Seed two matching + one non-matching entry per page. The 5th test (unread) was done in Task 1.

  Run: `cargo nextest run --test pages_test test_entries_page test_read_entries_page test_starred_entries_page test_summarized_entries_page`
  Expected: FAIL for the 4 new ones until handlers updated.

- [ ] **Step 2: Update `templates/entries.html`, `read_entries.html`, `starred_entries.html`, `summarized_entries.html`.**

  Each becomes:
  ```html
  {% extends "_entries_layout.html" %}
  ```

- [ ] **Step 3: Rewrite the 4 page handlers in `src/handlers/pages.rs`.**

  Identical structure to Task 1 Step 9 with different filters:

  | Handler | Filter | `active` | `description` | `path` | `empty_message` |
  |---------|--------|----------|---------------|--------|-----------------|
  | `entries_page` | `EntryFilter::default()` | `"all"` | `None` | `"/entries"` | `"No entries."` |
  | `read_entries_page` | `read_only: true` | `"read"` | `None` | `"/entries/read"` | `"No read entries."` |
  | `starred_entries_page` | `starred_only: true` | `"starred"` | `None` | `"/entries/starred"` | `"No starred entries."` |
  | `summarized_entries_page` | `has_summary: Some(true)` | `"summarized"` | `None` | `"/entries/summarized"` | `"No summarized entries."` |

  Extend each `*Template` struct in lockstep with `UnreadTemplate` from Task 1.

- [ ] **Step 4: `cargo nextest run` — full green.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run`
  Expected: 4 new tests pass, no regressions.

- [ ] **Step 5: Commit.**

  ```bash
  cargo fmt
  git add templates/entries.html templates/read_entries.html templates/starred_entries.html templates/summarized_entries.html src/handlers/pages.rs tests/pages_test.rs
  git commit -m "feat(ssr): PR-10 T2 — SSR /entries, /entries/{read,starred,summarized}"
  ```

---

## Task 3: Reading-pane fragment endpoint `GET /entries/{id}/fragment`

### Steps

- [ ] **Step 1: Write failing test.**

  Append to `tests/handlers_test.rs`:

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_entry_fragment_renders_reading_pane() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "T").await;
      let feed = env.create_feed(user.id, cat.id, "https://x/feed").await;
      let entry = env.create_entry_with_content(feed.id, "Hello World", "https://x/post", "<p>Body text here</p>").await;

      let resp = env.get_as(&user, &format!("/entries/{}/fragment", entry.id)).await;
      assert_eq!(resp.status(), 200);
      assert_eq!(resp.headers().get("content-type").unwrap(), "text/html; charset=utf-8");
      let html = resp.text().await.unwrap();
      assert!(html.contains(r#"id="reading-pane""#));
      assert!(html.contains("Hello World"));
      assert!(html.contains("Body text here"));
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_entry_fragment_404_for_other_user() {
      let env = TestEnv::new().await;
      let alice = env.create_user("alice", "pw").await;
      let bob = env.create_user("bob", "pw").await;
      let cat = env.create_category(bob.id, "T").await;
      let feed = env.create_feed(bob.id, cat.id, "https://b/feed").await;
      let entry = env.create_entry(feed.id, "Bob's Entry", "https://b/post", false).await;

      let resp = env.get_as(&alice, &format!("/entries/{}/fragment", entry.id)).await;
      assert_eq!(resp.status(), 404);
  }
  ```

  Run: `cargo nextest run --test handlers_test test_entry_fragment`
  Expected: FAIL (route missing).

- [ ] **Step 2: Create `src/handlers/entries.rs`.**

  ```rust
  use axum::{
      extract::{Path as AxumPath, State},
      response::{Html, IntoResponse, Response},
      http::StatusCode,
  };
  use askama::Template;

  use crate::{
      error::{AppError, AppResult},
      handlers::pages::{ReadingPaneView, EntryRowView, row_view_from},
      middleware::auth::PageAuthUser,
      models::entry,
      services::summary_cache::SummaryStatus,
      utils::time::format_relative_time,
      AppState,
  };

  #[derive(Template)]
  #[template(path = "_reading_pane.html")]
  pub struct ReadingPaneFragment {
      pub pane: ReadingPaneView,
  }

  pub async fn entry_fragment(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      AxumPath(entry_id): AxumPath<i64>,
  ) -> AppResult<ReadingPaneFragment> {
      let user_id = auth_user.user.id;
      let pane = load_reading_pane(&state, user_id, entry_id).await?;
      Ok(ReadingPaneFragment { pane })
  }

  pub(crate) async fn load_reading_pane(
      state: &AppState,
      user_id: i64,
      entry_id: i64,
  ) -> AppResult<ReadingPaneView> {
      let entry = state
          .db
          .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
          .await??
          .ok_or(AppError::NotFound)?;

      let summary = state.summary_cache.get(user_id, entry_id);
      let (summary_text, summary_in_flight) = match summary.as_ref().map(|s| &s.status) {
          Some(SummaryStatus::Completed) => (summary.as_ref().and_then(|s| s.summary_text.clone()), false),
          Some(SummaryStatus::Pending | SummaryStatus::Processing) => (None, true),
          _ => (None, false),
      };

      let published_at = entry.entry.published_at;
      Ok(ReadingPaneView {
          id: entry.entry.id,
          title: entry.entry.title.clone().unwrap_or_else(|| "(no title)".to_string()),
          link: entry.entry.link.clone(),
          feed_title: entry.feed_title.clone().unwrap_or_default(),
          author: entry.entry.author.clone(),
          published_at_iso: published_at.map(|t| t.to_rfc3339()),
          published_relative: format_relative_time(published_at).0,
          content_html: sanitize_content(&entry.entry.content.clone().unwrap_or_default()),
          is_read: entry.entry.read_at.is_some(),
          is_starred: entry.entry.starred_at.is_some(),
          summary_text,
          summary_in_flight,
      })
  }

  fn sanitize_content(raw: &str) -> String {
      crate::services::html_sanitize::sanitize(raw).unwrap_or_else(|_| String::new())
  }
  ```

  **VERIFY:** Read `src/services/html_sanitize.rs` (or wherever the sanitizer lives — `get_entry_detail` in `src/handlers/entry.rs:406-474` uses one). Copy its exact public API surface. If sanitizer needs an image-proxy parameter, pass `&state.config.image_proxy_url` similarly. The fragment must NOT re-run image-proxy rewriting if `get_entry_detail` already does it — match that behavior.

- [ ] **Step 3: Register route in `src/lib.rs`.**

  After line 161 (where `/entries/{id}` redirect is):

  ```rust
  .route(
      "/entries/{id}/fragment",
      get(handlers::entries::entry_fragment),
  )
  ```

  Add `pub mod entries;` to `src/handlers/mod.rs`.

- [ ] **Step 4: Run the failing tests; expect PASS.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run --test handlers_test test_entry_fragment`
  Expected: both tests pass.

- [ ] **Step 5: Commit.**

  ```bash
  cargo fmt
  git add src/handlers/entries.rs src/handlers/mod.rs src/lib.rs tests/handlers_test.rs
  git commit -m "feat(ssr): PR-10 T3 — reading-pane fragment endpoint"
  ```

---

## Task 4: Star + Read action endpoints

Both follow the same pattern: toggle, return multi-target HTML with the updated row + the updated `#sidebar-unread`. Implement star first, then read by analogy.

### Steps

- [ ] **Step 1: Confirm star/read model helpers exist.**

  Read `src/models/entry.rs` for `toggle_star` / `toggle_read` (or `set_starred` / `set_read`). Find the existing GReader-API entry points (in `src/handlers/greader.rs` or similar) and identify the underlying model function they call. Reuse that function — do NOT add a new SQL path.

  Likely candidates: `entry::set_read_state(conn, user_id, entry_id, is_read)`, `entry::set_starred_state(conn, user_id, entry_id, is_starred)`. Confirm names. **Do not proceed if these aren't present** — call out the gap.

- [ ] **Step 2: Add `_sidebar_unread.html` payload builder.**

  In `src/handlers/entries.rs`:

  ```rust
  use serde::Serialize;

  #[derive(Serialize, Debug, Clone)]
  pub struct UnreadCount {
      pub feed_id: i64,
      pub unread: i64,
  }

  pub(crate) async fn build_sidebar_unread(
      state: &AppState,
      user_id: i64,
  ) -> AppResult<String> {
      let counts = state
          .db
          .read_user(move |conn| entry::unread_counts_per_feed(conn, user_id))
          .await??;
      Ok(serde_json::to_string(&counts).unwrap_or_else(|_| "[]".to_string()))
  }
  ```

  **VERIFY:** `entry::unread_counts_per_feed` may not exist. If absent, add a small helper in `src/models/entry.rs`:

  ```rust
  pub fn unread_counts_per_feed(conn: &Connection, user_id: i64) -> AppResult<Vec<UnreadCount>> {
      let mut stmt = conn.prepare(
          "SELECT entries.feed_id, COUNT(*) AS unread \
           FROM entries \
           INNER JOIN feeds ON feeds.id = entries.feed_id \
           WHERE feeds.user_id = ? AND entries.read_at IS NULL \
           GROUP BY entries.feed_id"
      )?;
      let rows = stmt.query_map([user_id], |row| Ok(UnreadCount { feed_id: row.get(0)?, unread: row.get(1)? }))?
          .collect::<Result<Vec<_>, _>>()?;
      Ok(rows)
  }
  ```

  Adjust the JOIN to whatever the existing schema uses (some queries in this file go `feeds → categories → users`).

- [ ] **Step 3: Write failing test for `POST /entries/{id}/star`.**

  Append to `tests/handlers_test.rs`:

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_star_entry_form_toggles_and_returns_multi_target() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "T").await;
      let feed = env.create_feed(user.id, cat.id, "https://x/feed").await;
      let entry = env.create_entry(feed.id, "E", "https://x/p", true).await;

      let resp = env.post_as(&user, &format!("/entries/{}/star", entry.id), "").await;
      assert_eq!(resp.status(), 200);
      let html = resp.text().await.unwrap();
      assert!(html.contains(r#"data-swap-target="#entry-row-"#), "multi-target row block present");
      assert!(html.contains(r#"data-swap-target="#sidebar-unread""#), "multi-target sidebar block present");
      assert!(html.contains("entry-row-starred"), "row reflects starred state");

      // Toggle back
      let resp2 = env.post_as(&user, &format!("/entries/{}/star", entry.id), "").await;
      let html2 = resp2.text().await.unwrap();
      assert!(!html2.contains("entry-row-starred"), "row reflects unstarred after second toggle");
  }
  ```

  Run: `cargo nextest run --test handlers_test test_star_entry_form_toggles`. Expect FAIL (route missing).

- [ ] **Step 4: Add `star_entry_form` to `src/handlers/entries.rs`.**

  ```rust
  #[derive(Template)]
  #[template(path = "_entry_actions_multi.html")]
  pub struct EntryActionMulti {
      pub r: EntryRowView,
      pub sidebar_unread_payload_json: String,
  }

  pub async fn star_entry_form(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      AxumPath(entry_id): AxumPath<i64>,
  ) -> AppResult<EntryActionMulti> {
      let user_id = auth_user.user.id;
      let entry = state
          .db
          .write_user(move |conn| entry::toggle_starred(conn, user_id, entry_id))
          .await??
          .ok_or(AppError::NotFound)?;
      let payload_json = build_sidebar_unread(&state, user_id).await?;
      Ok(EntryActionMulti {
          r: row_view_from(&entry),
          sidebar_unread_payload_json: payload_json,
      })
  }
  ```

  `toggle_starred` returns `Option<EntryWithFeed>` (the post-toggle entry). If the model only has `set_starred_state(bool)`, wrap it:

  ```rust
  pub fn toggle_starred(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<Option<EntryWithFeed>> {
      let cur = find_by_id_for_user(conn, user_id, entry_id)?;
      let Some(e) = cur else { return Ok(None); };
      let new_state = e.entry.starred_at.is_none();
      set_starred_state(conn, user_id, entry_id, new_state)?;
      find_by_id_for_user(conn, user_id, entry_id)
  }
  ```

- [ ] **Step 5: Create `templates/_entry_actions_multi.html`.**

  ```html
  <template data-swap-target="#entry-row-{{ r.id }}">{% include "_entry_row.html" %}</template>
  <template data-swap-target="#sidebar-unread"><div id="sidebar-unread" data-payload='{{ sidebar_unread_payload_json|safe }}' hidden></div></template>
  ```

  Two `<template>` blocks. The swap helper iterates both and replaces each target by selector.

- [ ] **Step 6: Register route in `src/lib.rs`.**

  ```rust
  .route(
      "/entries/{id}/star",
      post(handlers::entries::star_entry_form),
  )
  ```

- [ ] **Step 7: Run the failing star test; expect PASS.**

- [ ] **Step 8: Repeat for `POST /entries/{id}/read`.**

  Identical pattern. New test `test_read_entry_form_toggles_and_returns_multi_target`. New handler `read_entry_form`. New route registration.

  Toggle helper: `entry::toggle_read` (mirror `toggle_starred`).

- [ ] **Step 9: Run full suite.**

  Run: `cargo nextest run`. Expect green.

- [ ] **Step 10: Commit.**

  ```bash
  cargo fmt
  git add src/handlers/entries.rs templates/_entry_actions_multi.html src/lib.rs tests/handlers_test.rs src/models/entry.rs
  git commit -m "feat(ssr): PR-10 T4 — star + read fragment endpoints (multi-target)"
  ```

---

## Task 5: Summarize action endpoint

### Steps

- [ ] **Step 1: Write failing test.**

  Append to `tests/handlers_test.rs`:

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_summarize_entry_form_renders_reading_pane() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "T").await;
      let feed = env.create_feed(user.id, cat.id, "https://x/feed").await;
      let entry = env.create_entry_with_content(feed.id, "E", "https://x/p", "<p>Body</p>").await;

      let resp = env.post_as(&user, &format!("/entries/{}/summarize", entry.id), "").await;
      assert_eq!(resp.status(), 200);
      let html = resp.text().await.unwrap();
      assert!(html.contains(r#"id="reading-pane""#));
      // Pending / processing badge visible
      assert!(html.contains("disabled"));
  }
  ```

  Run; expect FAIL.

- [ ] **Step 2: Add `summarize_entry_form` to `src/handlers/entries.rs`.**

  ```rust
  pub async fn summarize_entry_form(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      AxumPath(entry_id): AxumPath<i64>,
  ) -> AppResult<ReadingPaneFragment> {
      let user_id = auth_user.user.id;
      let entry = state
          .db
          .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
          .await??
          .ok_or(AppError::NotFound)?;

      state.summary_cache.set_pending(user_id, entry_id);
      let _ = state.summary_tx.send(crate::services::SummaryJob {
          user_id,
          entry_id,
          title: entry.entry.title.clone().unwrap_or_default(),
          content: entry.entry.content.clone().unwrap_or_default(),
      }).await;

      let pane = load_reading_pane(&state, user_id, entry_id).await?;
      Ok(ReadingPaneFragment { pane })
  }
  ```

  **VERIFY:** `SummaryJob` field names — read `src/services/summary.rs` (or wherever it's defined) to confirm. Don't guess.

- [ ] **Step 3: Register route in `src/lib.rs`.**

  ```rust
  .route(
      "/entries/{id}/summarize",
      post(handlers::entries::summarize_entry_form),
  )
  ```

- [ ] **Step 4: Run test; expect PASS.**

- [ ] **Step 5: Commit.**

  ```bash
  cargo fmt
  git add src/handlers/entries.rs src/lib.rs tests/handlers_test.rs
  git commit -m "feat(ssr): PR-10 T5 — summarize fragment endpoint"
  ```

---

## Task 6: Load-More fragment endpoint

Overload `GET /entries`: when `?fragment=1&after={offset}` is present, return a sequence of `_entry_row.html` blocks + a new `<form id="load-more">` (or omit if no more).

### Steps

- [ ] **Step 1: Write failing test.**

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_entries_load_more_returns_row_fragments() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "T").await;
      let feed = env.create_feed(user.id, cat.id, "https://x/feed").await;
      for i in 0..75 { env.create_entry(feed.id, &format!("E{i}"), &format!("https://x/{i}"), true).await; }

      let resp = env.get_as(&user, "/entries?fragment=1&after=50").await;
      assert_eq!(resp.status(), 200);
      let html = resp.text().await.unwrap();
      let row_count = html.matches("data-entry-row").count();
      assert_eq!(row_count, 25, "page 2 should yield 25 remaining rows");
      assert!(!html.contains(r#"id="load-more""#), "no more pages → no Load-More form");
  }
  ```

  Run; expect FAIL.

- [ ] **Step 2: Extend `entries_page` handler to handle `?fragment=1`.**

  In `src/handlers/pages.rs`, add `Query<EntriesQuery>` extractor:

  ```rust
  #[derive(serde::Deserialize, Default)]
  pub struct EntriesQuery {
      pub fragment: Option<u8>,
      pub after: Option<i64>,
  }
  ```

  Dispatch: if `query.fragment == Some(1)`, render a new `EntriesFragmentTemplate` with `entries` + `next_cursor` only (no layout context). Otherwise render the full `EntriesTemplate` with `offset = 0`.

- [ ] **Step 3: Create `templates/_entries_fragment.html`.**

  ```html
  {% for r in entries %}{% include "_entry_row.html" %}{% endfor %}
  {% if let Some(after) = next_cursor %}
      <form id="load-more" method="get" action="/entries" data-swap="#load-more" class="load-more-form">
          <input type="hidden" name="after" value="{{ after }}">
          <input type="hidden" name="fragment" value="1">
          <button type="submit">Load more</button>
      </form>
  {% endif %}
  ```

  And `EntriesFragmentTemplate` struct:
  ```rust
  #[derive(Template)]
  #[template(path = "_entries_fragment.html")]
  pub struct EntriesFragmentTemplate {
      pub entries: Vec<EntryRowView>,
      pub next_cursor: Option<i64>,
  }
  ```

- [ ] **Step 4: Update the `data-swap` target on the page-render `#load-more` form so it matches.**

  The form's `data-swap` is `#load-more` (the form itself). When the response contains a new `#load-more`, swap replaces; when it omits one, the old form's `outerHTML` becomes nothing (use `<div hidden id="load-more"></div>` as a guard, or arrange the response to contain a `<div hidden id="load-more"></div>` when finished).

  Pragmatic decision: when `next_cursor.is_none()`, the fragment response renders `<div hidden id="load-more"></div>` so the swap helper has something to replace `#load-more` with. The `_entries_fragment.html` template should produce row blocks followed by either the new form OR the hidden div. Update the template:

  ```html
  {% for r in entries %}{% include "_entry_row.html" %}{% endfor %}
  {% if let Some(after) = next_cursor %}
      <form id="load-more" ...>...</form>
  {% else %}
      <div id="load-more" hidden></div>
  {% endif %}
  ```

  **But this doesn't append rows.** The default-target swap replaces `#load-more` with the *first child* of the response body. We need multi-target append, but `swap()` doesn't support append.

  Decision: use `<template data-swap-target>` to make the swap multi-target with explicit insertion. Two options:
  - (a) Server returns `<template data-swap-target="#load-more">…</template>` containing the row blocks + new form/sentinel; swap helper replaces `#load-more` with its content. But we lose the existing rows.
  - (b) Re-render the full `data-entries-list` container each time. Simplest. Server returns `<template data-swap-target="[data-entries-list]">`. Existing rows + new rows + new form/sentinel inside.

  **Picked: (b)** — simpler model, no append semantics needed. `_entries_fragment.html` re-renders the entire `data-entries-list` block. The handler must therefore re-query from offset 0 to offset + 50 (whole prefix), not just offset 50..100. This is acceptable for PR-10; PR-12 cleanup can swap to append semantics via a small `appendChild` extension in `app.js`.

  Update Step 3's template to wrap in the multi-target template tag:
  ```html
  <template data-swap-target="[data-entries-list]">
      <div class="list-pane-body" data-entries-list>
          {% for r in entries %}{% include "_entry_row.html" %}{% endfor %}
          {% if let Some(after) = next_cursor %}
              <form id="load-more" method="get" action="{{ path }}" data-swap="[data-entries-list]" class="load-more-form">
                  <input type="hidden" name="after" value="{{ after }}">
                  <input type="hidden" name="fragment" value="1">
                  <button type="submit" data-testid="load-more-btn">Load more</button>
              </form>
          {% endif %}
      </div>
  </template>
  ```

  And update the handler to fetch from offset 0 to `after + 50` (i.e., full prefix). Update the `_entries_layout.html` initial `<form id="load-more">` `data-swap` target to `[data-entries-list]` too.

  Update test expectations: `row_count` after `?after=50` should be 75 (full prefix), not 25.

- [ ] **Step 5: Run test; expect PASS.**

- [ ] **Step 6: Apply same fragment overload to the other 4 entries handlers.**

  Each of `unread_page`, `read_entries_page`, `starred_entries_page`, `summarized_entries_page` accepts the same `EntriesQuery` and dispatches identically. Extract the dispatch into a helper:

  ```rust
  pub(crate) async fn render_entries_or_fragment(
      state: &AppState,
      auth_user: &PageAuthUser,
      flash: Flash,
      query: EntriesQuery,
      filter: entry::EntryFilter,
      sort: entry::EntrySortOrder,
      ctx: EntriesLayoutContext,
      title: &'static str,
  ) -> Response { ... }
  ```

  Returns either `(Flash, FullTemplate).into_response()` or `EntriesFragmentTemplate.into_response()`. Each of the 5 page handlers becomes thin.

- [ ] **Step 7: Run full suite; expect green.**

- [ ] **Step 8: Commit.**

  ```bash
  cargo fmt
  git add src/handlers/pages.rs templates/_entries_fragment.html templates/_entries_layout.html tests/handlers_test.rs tests/pages_test.rs
  git commit -m "feat(ssr): PR-10 T6 — Load-More fragment for entries family"
  ```

---

## Task 7: Sidebar unread polling endpoint `GET /sidebar/unread`

### Steps

- [ ] **Step 1: Write failing test.**

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_sidebar_unread_returns_payload() {
      let env = TestEnv::new().await;
      let user = env.create_user("alice", "pw").await;
      let cat = env.create_category(user.id, "T").await;
      let feed = env.create_feed(user.id, cat.id, "https://x/feed").await;
      env.create_entry(feed.id, "U1", "https://x/1", true).await;
      env.create_entry(feed.id, "U2", "https://x/2", true).await;

      let resp = env.get_as(&user, "/sidebar/unread").await;
      assert_eq!(resp.status(), 200);
      let html = resp.text().await.unwrap();
      assert!(html.contains(r#"id="sidebar-unread""#));
      assert!(html.contains(r#""feed_id":"#) || html.contains(r#""feed_id":"#));
      assert!(html.contains(r#""unread":2"#));
  }
  ```

  Expect FAIL.

- [ ] **Step 2: Add `sidebar_unread_fragment` handler to `src/handlers/entries.rs`.**

  ```rust
  #[derive(Template)]
  #[template(path = "_sidebar_unread.html")]
  pub struct SidebarUnreadFragment {
      pub payload_json: String,
  }

  pub async fn sidebar_unread_fragment(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
  ) -> AppResult<SidebarUnreadFragment> {
      let user_id = auth_user.user.id;
      let payload_json = build_sidebar_unread(&state, user_id).await?;
      Ok(SidebarUnreadFragment { payload_json })
  }
  ```

- [ ] **Step 3: Register route.**

  ```rust
  .route("/sidebar/unread", get(handlers::entries::sidebar_unread_fragment))
  ```

- [ ] **Step 4: Run test; expect PASS.**

- [ ] **Step 5: Commit.**

  ```bash
  cargo fmt
  git add src/handlers/entries.rs src/lib.rs tests/handlers_test.rs
  git commit -m "feat(ssr): PR-10 T7 — sidebar unread polling endpoint"
  ```

---

## Task 8: `app.js` — sidebar polling + minimal keyboard

Add two sections to `static/js/app.js`. **Do not delete the existing `keyboard.js` script tag from `app_layout.html`** — PR-11 routes still need it. The new app.js keyboard handler is page-scoped: it only fires on pages that have `[data-entries-list]` (the SSR entries family).

### Steps

- [ ] **Step 1: Append polling section to `static/js/app.js`.**

  ```javascript
  // Sidebar unread polling — fires every 20s on pages that mount the
  // SSR sidebar-unread block (the 5 entries-family routes in PR-10).
  // The payload is JSON in the `data-payload` attribute; we dispatch a
  // custom event so `<rdrs-sidebar>` can apply the new counts.
  function installSidebarPolling() {
      const host = document.getElementById('sidebar-unread');
      if (!host) return;
      const tick = async () => {
          try {
              const resp = await fetch('/sidebar/unread', { credentials: 'same-origin' });
              if (!resp.ok) return;
              const html = await resp.text();
              const doc = new DOMParser().parseFromString(html, 'text/html');
              const node = doc.getElementById('sidebar-unread');
              if (!node) return;
              const payload = node.getAttribute('data-payload') || '[]';
              const target = document.getElementById('sidebar-unread');
              if (target) target.setAttribute('data-payload', payload);
              document.dispatchEvent(new CustomEvent('rdrs:sidebar-unread', {
                  detail: JSON.parse(payload),
              }));
          } catch {}
      };
      setInterval(tick, 20000);
  }
  installSidebarPolling();
  ```

  **VERIFY:** Check `<rdrs-sidebar>` (`static/js/components/rdrs-sidebar.js`) — does it already listen for an event or read from a global? Wire to whatever it expects, OR add a small listener block in the sidebar component. Don't invent an event the sidebar doesn't understand. If the sidebar reads only from `#rdrs-sidebar-bootstrap`, this PR can ship the polling endpoint but skip the live-apply wiring (defer to PR-12). For PR-10 the safer option is: poll, swap, but accept that the sidebar UI updates only on next page-nav until PR-12 wires up the listener. Document this acceptance in a code comment.

- [ ] **Step 2: Append minimal keyboard section.**

  ```javascript
  // Keyboard shortcuts for SSR entries-family pages. Only active when a
  // `[data-entries-list]` is present so we don't conflict with the
  // legacy `keyboard.js` running on PR-11 CSR routes.
  function installEntriesKeyboard() {
      if (!document.querySelector('[data-entries-list]')) return;
      let active = null; // currently focused entry row
      const rows = () => Array.from(document.querySelectorAll('[data-entry-row]'));
      const focusRow = (row) => {
          if (!row) return;
          if (active) active.classList.remove('entry-row-focused');
          active = row;
          row.classList.add('entry-row-focused');
          row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      };
      const move = (delta) => {
          const all = rows();
          if (all.length === 0) return;
          const idx = active ? all.indexOf(active) : -1;
          const next = Math.max(0, Math.min(all.length - 1, idx + delta));
          focusRow(all[next]);
      };
      document.addEventListener('keydown', async (e) => {
          if (e.target.matches('input, textarea, select')) return;
          if (e.metaKey || e.ctrlKey || e.altKey) return;
          switch (e.key) {
              case 'j': e.preventDefault(); move(1); break;
              case 'k': e.preventDefault(); move(-1); break;
              case 'o':
              case 'Enter': {
                  if (!active) return;
                  const link = active.querySelector('a[data-swap]');
                  if (link) { e.preventDefault(); link.click(); }
                  break;
              }
              case 's': {
                  if (!active) return;
                  const form = active.querySelector('form[action$="/star"]');
                  if (form) { e.preventDefault(); form.requestSubmit(); }
                  break;
              }
              case ' ': {
                  if (!active) return;
                  const form = active.querySelector('form[action$="/read"]');
                  if (form) { e.preventDefault(); form.requestSubmit(); }
                  break;
              }
          }
      });
  }
  installEntriesKeyboard();
  ```

  These five bindings (`j`/`k`/`o`/`s`/`space`) cover the minimum keyboard surface for PR-10. `?` (help) and `m` (mark all read) defer to PR-12 with the rest of the keyboard polish.

- [ ] **Step 3: Visual e2e check via Playwright (manual).**

  Run the dev server, log in as a seeded user with a feed and a few unread entries:
  - Visit `/` — verify entries list renders, click a title — reading pane swaps in.
  - Click a star button — row star icon flips, no full reload.
  - Click a read button — row read icon flips, no full reload.
  - Click Load-More — list grows.
  - Press `j`/`k` — focus moves between rows.
  - Press `o` — reading pane swaps.
  - Press `s` / space — star/read toggle.

  Note any visual regressions in `.entry-row-focused` styling. If the focused-row outline isn't visible, add `static/css/app.css` rule:

  ```css
  .entry-row-focused { outline: 2px solid var(--accent, currentColor); outline-offset: -2px; }
  ```

- [ ] **Step 4: Commit.**

  ```bash
  cargo fmt
  git add static/js/app.js static/css/app.css
  git commit -m "feat(ssr): PR-10 T8 — app.js sidebar polling + entries keyboard"
  ```

---

## Task 9: Verify, push, PR

### Steps

- [ ] **Step 1: Full test suite.**

  Run: `source /tmp/rdrs-env.sh && cargo nextest run`
  Expected: all green. Total grows by ~12-15 new tests across pages_test + handlers_test.

- [ ] **Step 2: Format check.**

  Run: `cargo fmt --check`
  Expected: no output.

- [ ] **Step 3: Smoke build.**

  Run: `cargo build --release` (or `cargo check --all-targets` if release build is slow).
  Expected: no errors.

- [ ] **Step 4: Spec self-review.**

  Re-read the spec section on entries (lines 78-114, 117-145, 164-194). Verify every fragment endpoint listed exists. Verify the row + reading-pane templates match the spec's wire format. List any gaps; if none, proceed.

- [ ] **Step 5: Run a code-review subagent (optional but recommended).**

  Spawn `superpowers:requesting-code-review` against the diff. Address any blocking feedback inline; defer non-blocking items to the carry-forward list in `project_ssr_first_migration.md`.

- [ ] **Step 6: Push branch.**

  ```bash
  git push -u origin feat/ssr-entries-family
  ```

- [ ] **Step 7: Open PR.**

  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-10 — entries family" --body "$(cat <<'EOF'
  ## Summary
  - SSR'd 5 entries-family pages (`/`, `/entries`, `/entries/{read,starred,summarized}`).
  - Added 6 fragment endpoints (`/entries/{id}/fragment`, `POST /entries/{id}/{star,read,summarize}`, `/entries?fragment=1&after=…` Load-More, `GET /sidebar/unread` polling).
  - Extended `app.js` with sidebar polling and a minimal keyboard section (j/k/o/s/space) scoped to SSR entries pages.

  ## Test plan
  - [ ] Rust nextest suite green (~12-15 new tests added).
  - [ ] Manual: visit `/`, click an entry, verify reading-pane swap.
  - [ ] Manual: star + read toggle via row buttons.
  - [ ] Manual: Load-More expands list.
  - [ ] Manual: `j`/`k` cycle rows, `o`/Enter open reading pane.

  Refs: `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md` migration step 10.
  EOF
  )"
  ```

- [ ] **Step 8: Watch CI; merge when green.**

  Wait for CI checks. Once green, squash-merge with the source branch deleted.

  ```bash
  gh pr checks <pr-number>
  gh pr merge <pr-number> --squash --delete-branch
  ```

- [ ] **Step 9: Update memory + close out.**

  Update `project_ssr_first_migration.md`: PR-10 merged, PR-11 next. Add any carry-forwards (sidebar live-apply wiring deferred, ?/m keyboard deferred, append-vs-prefix Load-More semantic deferred).

---

## Carry-forward to PR-11 / PR-12

- `entries.js` still loaded by `templates/feed_entries.html` + `templates/category_entries.html` — PR-11 migrates them.
- `<rdrs-entry-list>` still in `app_layout.html` script tags — PR-11 may drop if PR-11 SSRs the last consumer; otherwise PR-12.
- `keyboard.js` still loaded — PR-11 routes need it; PR-12 deletes.
- `?` and `m` keyboard shortcuts not yet in `app.js` — PR-12.
- Sidebar live-apply on poll event — PR-12 (requires touching `rdrs-sidebar.js`).
- Composite-cursor pagination for entries (currently OFFSET) — PR-12 or follow-up.
- `GET /api/entries/{id}`, `GET /api/feeds` deletion — PR-11/PR-12 once their last consumer dies.
- Append (not prefix-rerender) Load-More semantics — PR-12 or follow-up.
- CSS for `.entry-row-focused`, `.entry-row-starred`, `.entry-row-read` may need refinement — eyeball during manual e2e.
