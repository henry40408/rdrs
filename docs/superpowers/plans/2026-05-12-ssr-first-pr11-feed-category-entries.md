# SSR-first PR-11: Feed & Category Entries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/feeds/{id}/entries` and `/categories/{id}/entries` from the legacy CSR shell (`<rdrs-entries-page>` mounted by `static/js/pages/entries.js`) to direct SSR using the PR-10 infrastructure — `build_entries_page()` helper + `_entries_layout.html` + `EntryRowView` + the existing fragment endpoints. After this PR the entries family is end-to-end SSR. PR-12 then deletes the legacy scaffolding (`static/js/pages/entries.js`, `<rdrs-entries-page>`, `<rdrs-entry-list>`, `keyboard.js`, `GET /api/entries/{id}`, the entries.js consumer of `GET /api/feeds`).

**Architecture:** Two new SSR handlers (`feed_entries_page` + `category_entries_page`) replace the existing CSR-shell ones in `src/handlers/pages.rs`. Both share the existing `build_entries_page` helper — the only differences are (1) a validating pre-query that 404s if the feed/category doesn't exist or belongs to a different user, (2) the `EntryFilter` used (`feed_id` vs. `category_id` — both fields already exist on `EntryFilter`), (3) the `EntriesLayoutContext.path` value (parameterized, e.g. `/feeds/42/entries`). All fragment endpoints (`GET /entries/{id}/fragment`, `POST /entries/{id}/{star,unstar,read,unread,summarize,save,fetch-full-content}`, `GET /sidebar/unread`) are reused without change — they're keyed on entry id only, agnostic to which list page led to opening them.

One small refactor unlocks this: `EntriesLayoutContext.path` is currently `&'static str` because PR-10's 5 paths were known compile-time constants. PR-11 paths are parameterized by `id`, so the field must become `String` (or `Cow<'static, str>` — but `String` is simpler and the per-request alloc cost is negligible vs. the surrounding Askama render). The 5 existing entries-family handlers each gain a `.to_string()` on their `path` literal.

**Tech Stack:** Rust + Axum + Askama 0.15. Reuses `entry::list_by_user`, `entry::EntryFilter`, `entry::find_by_id_for_user`, `build_entries_page`, `EntryRowView`, `EntriesLayoutContext`, `_entries_layout.html`, `_entry_row.html`, `_reading_pane.html`. No new JS, no new CSS, no new model functions. `<rdrs-sidebar>` chrome stays.

**Spec:** `docs/superpowers/specs/2026-05-08-ssr-first-redesign-design.md` (Migration Plan step 11, plus the Endpoints + Architecture sections).

**Base commit:** `ab6ba61` (current `main` after PR #197 merge).

**Branch:** `feat/ssr-feed-category-entries` (already created off `main` at `ab6ba61`).

**Reference PR:** PR-10 plan at `docs/superpowers/plans/2026-05-11-ssr-first-pr10-entries-family.md`. PR-10 (#196) + PR #197 are the up-to-date pattern; this plan re-uses every piece of that infrastructure.

---

## Pre-flight

- [ ] **Verify branch and clean state.**

  Run: `git -C /home/nixos/Develop/claude/rdrs status -sb && git -C /home/nixos/Develop/claude/rdrs branch --show-current && git -C /home/nixos/Develop/claude/rdrs log --oneline -3`
  Expected: branch `feat/ssr-feed-category-entries`, working tree clean modulo untracked `test-results/` and `rdrs.sqlite3-*`, latest commit on main is `ab6ba61 fix(ui): restore pre-SSR entries-list & reading-pane styling (#197)`.

- [ ] **Source OpenSSL env.**

  Run: `source /tmp/rdrs-env.sh`
  Expected: no output. Required on this NixOS host before any `cargo` invocation.

- [ ] **Baseline test suite green.**

  Run: `cargo nextest run`
  Expected: 727 tests, 725 PASS + 2 known dirty-tree cache-control FAIL (`test_static_css_serves_app_css`, `test_static_js_serves_known_file`). Both pass after the next commit lands.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `src/handlers/pages.rs` (`EntriesLayoutContext` struct, ~line 80-93) | Change `path: &'static str` → `path: String`. Required so feed/category pages can pass `format!("/feeds/{id}/entries", id=id)` to the Load-More form's action URL. |
| Modify | `src/handlers/pages.rs` (5 existing entries-family handlers, ~lines 439-505 unread, 938+ entries, plus read/starred/summarized) | Add `.to_string()` to the 5 hard-coded `path:` literals (`"/"`, `"/entries"`, `"/entries/read"`, `"/entries/starred"`, `"/entries/summarized"`). Pure compile-error-driven cleanup; no behavior change. |
| Modify | `src/handlers/pages.rs` (`category_entries_page`, ~line 1270) | Replace CSR shell handler with SSR handler. New `CategoryEntriesTemplate` struct gains `entries`, `reading_pane`, `next_cursor`, `entries_layout` fields. Dispatches on `EntriesQuery.fragment` for Load-More — returns `EntriesFragmentTemplate` when `?fragment=1` is present, full page otherwise. Validates category ownership via existing `category::find_by_id_and_user`. |
| Modify | `src/handlers/pages.rs` (`feed_entries_page`, ~line 1594) | Same pattern. Validates feed via the existing `feed::find_by_id` + `category::find_by_id` + `cat.user_id == user_id` chain. Page title is the feed's title. |
| Rewrite | `templates/category_entries.html` | Replace the 10-line CSR shell with `{% extends "_entries_layout.html" %}`. Inherits everything; no new fields. |
| Rewrite | `templates/feed_entries.html` | Same. |
| Modify | `tests/pages_test.rs` (~lines 704-839, 6 tests) | Rewrite assertions: drop `<rdrs-entries-page>` / `entries.js` checks; assert positive SSR markup (entry rows present, page title rendered, sidebar mounted). Keep the 404 + cross-tenant test cases intact (paths unchanged, just assertion bodies updated). |
| Add | `tests/pages_test.rs` | New tests: `test_feed_entries_page_load_more_fragment` + `test_category_entries_page_load_more_fragment` — assert `?fragment=1&after=N` returns just the rows fragment without the layout chrome. |

**Endpoints NOT touched:** every PR-10 fragment endpoint works for any entry, regardless of list page. No new routes, no removed routes.

**Endpoints kept alive but consumer-less after this PR:** `GET /api/entries/{id}`, `GET /api/feeds`, the legacy CSR JS, etc. PR-12 deletes them.

---

## Task 1: Refactor `EntriesLayoutContext.path` to `String`

**Files:**
- Modify: `src/handlers/pages.rs` (struct definition at ~line 80-93)
- Modify: `src/handlers/pages.rs` (5 callsites in the entries-family handlers)

This is a pure type refactor that unblocks Tasks 2 and 3 (which need to pass a parameterized path). No test additions — the existing 5 entries-family page tests serve as the regression net.

- [ ] **Step 1: Change the struct field.**

  Edit `src/handlers/pages.rs`. Find:

  ```rust
  pub struct EntriesLayoutContext {
      pub active: &'static str,
      pub description: Option<String>,
      pub empty_message: &'static str,
      pub path: &'static str,
      pub show_tab_bar: bool,
      pub show_mark_as_read: bool,
  }
  ```

  Change to:

  ```rust
  pub struct EntriesLayoutContext {
      pub active: &'static str,
      pub description: Option<String>,
      pub empty_message: &'static str,
      pub path: String,
      pub show_tab_bar: bool,
      pub show_mark_as_read: bool,
  }
  ```

- [ ] **Step 2: Run `cargo check` to surface the 5 compile errors.**

  Run: `cargo check 2>&1 | grep -E "expected \`String\`" | head -10`
  Expected: 5 errors, each pointing at a `path: "..."` literal in one of the 5 entries-family handlers (`unread_page`, `entries_page`, `read_entries_page`, `starred_entries_page`, `summarized_entries_page`).

- [ ] **Step 3: Add `.to_string()` to each of the 5 callsites.**

  Each error site looks like `path: "/",` — change to `path: "/".to_string(),`. The exact 5 string literals to fix:

  - `"/"` → `"/".to_string()`
  - `"/entries"` → `"/entries".to_string()`
  - `"/entries/read"` → `"/entries/read".to_string()`
  - `"/entries/starred"` → `"/entries/starred".to_string()`
  - `"/entries/summarized"` → `"/entries/summarized".to_string()`

  Each path literal appears in TWO places per handler — once in the full-page render block (where `EntriesLayoutContext` is built) and once in the fragment block (where `EntriesFragmentTemplate.path` is built). The second one is a different struct (`EntriesFragmentTemplate.path: &'static str`) and should be LEFT ALONE — only the `EntriesLayoutContext.path` instances change.

  Verify by grepping for the field assignment context:

  Run: `grep -n "path: \"" src/handlers/pages.rs | head -20`
  Expected after fix: any remaining `path: "/..."` literals are for `EntriesFragmentTemplate`, not `EntriesLayoutContext`.

- [ ] **Step 4: Run `cargo check` to verify clean compile.**

  Run: `cargo check 2>&1 | tail -3`
  Expected: `Finished \`dev\` profile ...` with no errors.

- [ ] **Step 5: Run the entries-family page tests to verify no regression.**

  Run: `cargo nextest run -E 'test(unread_page) | test(entries_page) | test(read_entries_page) | test(starred_entries_page) | test(summarized_entries_page)'`
  Expected: all pre-existing tests for these 5 handlers pass.

- [ ] **Step 6: Commit.**

  ```bash
  git add src/handlers/pages.rs
  git commit -S -m "refactor(pages): EntriesLayoutContext.path is String

Unblocks parameterized paths for the upcoming PR-11 feed/category
entries pages (e.g. \"/feeds/{id}/entries\"). The 5 existing
entries-family handlers gain a no-op \`.to_string()\` on their
literal path values.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
  ```

---

## Task 2: SSR `/feeds/{id}/entries`

**Files:**
- Modify: `src/handlers/pages.rs` (replace `feed_entries_page` + `FeedEntriesTemplate` at ~line 1594-1623)
- Rewrite: `templates/feed_entries.html`
- Modify: `tests/pages_test.rs` (rewrite `test_feed_entries_page`, `test_feed_entries_page_not_found`, `test_feed_entries_page_other_user`)
- Add: `tests/pages_test.rs` (new `test_feed_entries_page_load_more_fragment`)

- [ ] **Step 1: Write the failing test for the SSR full-page render.**

  Rewrite `test_feed_entries_page` in `tests/pages_test.rs`. The test must seed a feed with at least 2 entries (one read, one unread, both belonging to the logged-in user's category → feed). Replace any prior `assert!(html.contains("rdrs-entries-page"))` / `entries.js` checks with positive SSR assertions:

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_feed_entries_page() {
      let app = create_test_app_named(default_test_config(), "test_feed_entries_page");

      app.server
          .post("/api/register")
          .json(&json!({ "username": "alice_fe", "password": "pw123456" }))
          .await
          .assert_status(StatusCode::CREATED);
      app.server
          .post("/api/session")
          .json(&json!({ "username": "alice_fe", "password": "pw123456" }))
          .await
          .assert_status_ok();

      let (feed_id, entry_a_id, entry_b_id) = app
          .db
          .user(|conn| {
              let user_id: i64 = conn
                  .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                  .unwrap();
              let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
              let feed = rdrs::models::feed::create_feed(
                  conn,
                  &rdrs::models::feed::CreateFeedParams {
                      category_id: cat.id,
                      url: "https://x/fe-feed",
                      title: Some("FE Feed"),
                      description: None,
                      site_url: None,
                      custom_user_agent: None,
                      http2_disabled: None,
                      custom_referrer: None,
                  },
              )
              .unwrap();
              let (a, _) = rdrs::models::entry::upsert_entry(
                  conn,
                  feed.id,
                  "guid-fe-a",
                  Some("First Entry"),
                  Some("https://x/a"),
                  None,
                  None,
                  None,
                  None,
              )
              .unwrap();
              let (b, _) = rdrs::models::entry::upsert_entry(
                  conn,
                  feed.id,
                  "guid-fe-b",
                  Some("Second Entry"),
                  Some("https://x/b"),
                  None,
                  None,
                  None,
                  None,
              )
              .unwrap();
              (feed.id, a.id, b.id)
          })
          .await
          .unwrap();

      let resp = app
          .server
          .get(&format!("/feeds/{}/entries", feed_id))
          .await;
      assert_eq!(resp.status_code(), StatusCode::OK);
      let html = resp.text();

      // Page title is the feed's title.
      assert!(html.contains("FE Feed"), "page title must render feed title");

      // SSR rows present.
      assert!(
          html.contains(&format!("id=\"entry-row-{}\"", entry_a_id)),
          "row for first entry must be in the HTML"
      );
      assert!(
          html.contains(&format!("id=\"entry-row-{}\"", entry_b_id)),
          "row for second entry must be in the HTML"
      );

      // No CSR shell.
      assert!(
          !html.contains("rdrs-entries-page"),
          "SSR page must not mount the legacy <rdrs-entries-page> shell"
      );
      assert!(
          !html.contains("/static/js/pages/entries.js"),
          "SSR page must not load the legacy entries.js bundle"
      );

      // Reading-pane placeholder rendered (no entry pre-selected).
      assert!(
          html.contains("Select an entry to read."),
          "reading-pane placeholder must render when no entry is selected"
      );

      // Load-More form, if any, must target the feed-specific URL.
      // (May or may not be present depending on whether the seeded count
      // exceeds the page size — assert conditionally.)
      if html.contains("id=\"load-more\"") {
          assert!(
              html.contains(&format!("action=\"/feeds/{}/entries\"", feed_id)),
              "Load-More form must POST back to the same feed-scoped URL"
          );
      }
  }
  ```

- [ ] **Step 2: Run the test to verify it fails.**

  Run: `cargo nextest run -E 'test(test_feed_entries_page) - test(test_feed_entries_page_not_found) - test(test_feed_entries_page_other_user) - test(test_feed_entries_page_load_more_fragment)'`
  Expected: FAIL — current CSR handler renders `<rdrs-entries-page>` shell, so `assert!(html.contains("FE Feed"))` (or one of the SSR row asserts) fires.

- [ ] **Step 3: Rewrite `FeedEntriesTemplate` and `feed_entries_page` in `src/handlers/pages.rs`.**

  Replace the existing struct + handler. The struct gains the same fields the PR-10 entries-family templates use, plus `entries_layout`. The handler validates feed ownership, loads the feed's title for the page heading, builds the filter, and dispatches on `?fragment=1` for Load-More.

  ```rust
  #[derive(Template)]
  #[template(path = "feed_entries.html")]
  pub struct FeedEntriesTemplate {
      pub title: String,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub entries: Vec<EntryRowView>,
      pub reading_pane: Option<ReadingPaneView>,
      pub next_cursor: Option<i64>,
      pub entries_layout: EntriesLayoutContext,
  }

  /// `GET /feeds/{id}/entries` — SSR list of entries from a single feed.
  /// Supports the `?fragment=1&after=N` Load-More overload like the other
  /// entries-family pages.
  pub async fn feed_entries_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      Path(id): Path<i64>,
      AxumQuery(query): AxumQuery<EntriesQuery>,
      flash: Flash,
  ) -> Result<Response, AppError> {
      let user_id = auth_user.user.id;

      // Validate ownership AND fetch the feed title for the page heading
      // in one DB read.
      let feed_title = state
          .db
          .read_user(move |c| {
              let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
              let cat = category::find_by_id(c, f.category_id)?
                  .ok_or(AppError::CategoryNotFound)?;
              if cat.user_id != user_id {
                  return Err(AppError::FeedNotFound);
              }
              Ok::<_, AppError>(f.title.unwrap_or_else(|| "(untitled feed)".to_string()))
          })
          .await??;

      let filter = entry::EntryFilter {
          feed_id: Some(id),
          ..Default::default()
      };
      let sort = entry::EntrySortOrder::PublishedDesc;
      let page_size = ENTRIES_PAGE_SIZE;
      let offset = query.after.unwrap_or(0);

      let (entries, next_cursor) =
          build_entries_page(&state, user_id, filter, sort, page_size, offset).await;

      let path = format!("/feeds/{}/entries", id);

      // Fragment branch — Load-More.
      if query.fragment == Some(1) {
          let fragment = EntriesFragmentTemplate {
              entries,
              next_cursor,
              path: Box::leak(path.into_boxed_str()),
          };
          return Ok(fragment.into_response());
      }

      let layout = build_app_layout(&state, &auth_user, &flash).await;

      let template = FeedEntriesTemplate {
          title: feed_title,
          git_version: crate::GIT_VERSION,
          layout,
          entries,
          reading_pane: None,
          next_cursor,
          entries_layout: EntriesLayoutContext {
              active: "",
              description: None,
              empty_message: "No entries in this feed.",
              path,
              show_tab_bar: false,
              show_mark_as_read: false,
          },
      };

      Ok((flash, template).into_response())
  }
  ```

  Notes for the implementer:

  - Adjust imports if needed: `Path`, `AxumQuery`, `Response`, `IntoResponse`, `entry::{EntryFilter, EntrySortOrder}`, `feed`, `category`.
  - `ENTRIES_PAGE_SIZE` is the same constant the other 5 entries-family handlers use. Search for it in `pages.rs` — it's defined near the top of the file.
  - `Box::leak(path.into_boxed_str())` is the workaround for `EntriesFragmentTemplate.path: &'static str`. PR-10 used the same trick for the parameterized search path. Acceptable per-request leak — the strings are bounded by route arity.
  - If `EntriesFragmentTemplate.path` was already changed to `String` by some other PR by the time you read this, drop the `Box::leak` and pass `path` directly.

- [ ] **Step 4: Rewrite `templates/feed_entries.html`.**

  ```html
  {% extends "_entries_layout.html" %}
  ```

  That's the whole file. The shared layout pulls `title`, `entries`, `reading_pane`, `next_cursor`, `entries_layout`, `layout` from the page-level struct's fields — they all match by name.

- [ ] **Step 5: Run the test to verify it passes.**

  Run: `cargo nextest run -E 'test(test_feed_entries_page) - test(test_feed_entries_page_not_found) - test(test_feed_entries_page_other_user) - test(test_feed_entries_page_load_more_fragment)'`
  Expected: PASS.

- [ ] **Step 6: Update the existing 404 + cross-tenant tests.**

  Both `test_feed_entries_page_not_found` and `test_feed_entries_page_other_user` already exist and test the path. Their existing assertions probably reference the CSR template body — replace any positive-content asserts with `assert_eq!(resp.status_code(), StatusCode::NOT_FOUND)` only. Concretely:

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_feed_entries_page_not_found() {
      let app = create_test_app_named(default_test_config(), "test_feed_entries_page_not_found");

      app.server
          .post("/api/register")
          .json(&json!({ "username": "alice_fnf", "password": "pw123456" }))
          .await
          .assert_status(StatusCode::CREATED);
      app.server
          .post("/api/session")
          .json(&json!({ "username": "alice_fnf", "password": "pw123456" }))
          .await
          .assert_status_ok();

      let resp = app.server.get("/feeds/999999/entries").await;
      assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
  }
  ```

  For `test_feed_entries_page_other_user`: seed bob's feed directly via SQL (like `test_star_entry_form_404_for_other_user` does in `tests/handlers_test.rs`), then assert alice gets 404 when she requests bob's feed's entries page.

- [ ] **Step 7: Add the Load-More fragment test.**

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_feed_entries_page_load_more_fragment() {
      let app = create_test_app_named(default_test_config(), "test_feed_entries_page_lm");

      app.server
          .post("/api/register")
          .json(&json!({ "username": "alice_fl", "password": "pw123456" }))
          .await
          .assert_status(StatusCode::CREATED);
      app.server
          .post("/api/session")
          .json(&json!({ "username": "alice_fl", "password": "pw123456" }))
          .await
          .assert_status_ok();

      let feed_id: i64 = app
          .db
          .user(|conn| {
              let user_id: i64 = conn
                  .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                  .unwrap();
              let cat = rdrs::models::category::create_category(conn, user_id, "T").unwrap();
              let feed = rdrs::models::feed::create_feed(
                  conn,
                  &rdrs::models::feed::CreateFeedParams {
                      category_id: cat.id,
                      url: "https://x/lm-feed",
                      title: Some("LM Feed"),
                      description: None,
                      site_url: None,
                      custom_user_agent: None,
                      http2_disabled: None,
                      custom_referrer: None,
                  },
              )
              .unwrap();
              for i in 0..3 {
                  rdrs::models::entry::upsert_entry(
                      conn,
                      feed.id,
                      &format!("guid-lm-{}", i),
                      Some(&format!("Entry {}", i)),
                      Some(&format!("https://x/lm/{}", i)),
                      None,
                      None,
                      None,
                      None,
                  )
                  .unwrap();
              }
              feed.id
          })
          .await
          .unwrap();

      // `?fragment=1` returns only the rows fragment — no layout chrome,
      // no <rdrs-sidebar>, no <h1> page title.
      let resp = app
          .server
          .get(&format!(
              "/feeds/{}/entries?fragment=1&after=0",
              feed_id
          ))
          .await;
      assert_eq!(resp.status_code(), StatusCode::OK);
      let html = resp.text();
      assert!(
          html.contains("data-entry-row"),
          "fragment must include row markup"
      );
      assert!(
          !html.contains("<rdrs-sidebar"),
          "fragment must NOT include the layout chrome"
      );
      assert!(
          !html.contains("<h1>LM Feed</h1>"),
          "fragment must NOT include the page title"
      );
  }
  ```

  Run: `cargo nextest run -E 'test(test_feed_entries_page_load_more_fragment)'`
  Expected: PASS.

- [ ] **Step 8: Run the full feed-entries test set to confirm everything is green.**

  Run: `cargo nextest run -E 'test(/test_feed_entries_page/)'`
  Expected: 4 tests PASS (`test_feed_entries_page`, `_not_found`, `_other_user`, `_load_more_fragment`).

- [ ] **Step 9: Commit.**

  ```bash
  git add src/handlers/pages.rs templates/feed_entries.html tests/pages_test.rs
  git commit -S -m "feat(ssr): SSR-first PR-11 — /feeds/{id}/entries

The page is now a normal entries-family route: \`build_entries_page\`
with \`EntryFilter::feed_id\`, \`_entries_layout.html\` shell, all the
existing fragment endpoints reused. \`?fragment=1&after=N\` powers
Load-More with the same protocol as the other 5 entries-family
pages.

Tests:
- Rewrote test_feed_entries_page to assert SSR rows + absence of
  <rdrs-entries-page> shell.
- Trimmed the 404 + cross-tenant tests to status-only assertions.
- New test_feed_entries_page_load_more_fragment covers the
  fragment-only branch.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
  ```

---

## Task 3: SSR `/categories/{id}/entries`

Mirror image of Task 2 — same shape, same TDD rhythm, just with `EntryFilter::category_id` and `category::find_by_id_and_user`.

**Files:**
- Modify: `src/handlers/pages.rs` (replace `category_entries_page` + `CategoryEntriesTemplate` at ~line 1270)
- Rewrite: `templates/category_entries.html`
- Modify: `tests/pages_test.rs` (rewrite `test_category_entries_page`, `test_category_entries_page_not_found`, `test_category_entries_page_other_user`)
- Add: `tests/pages_test.rs` (new `test_category_entries_page_load_more_fragment`)

- [ ] **Step 1: Write the failing test for SSR full-page render.**

  Same structure as Task 2 step 1, but seed a category + 2 feeds + 2 entries (one entry per feed) so the category-scoped filter is non-trivial. Assertions are identical: page title = category name, both entry rows present, no `<rdrs-entries-page>`, no `entries.js`, Load-More form (if present) targets `/categories/{id}/entries`.

  ```rust
  #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
  async fn test_category_entries_page() {
      let app = create_test_app_named(default_test_config(), "test_category_entries_page");

      app.server
          .post("/api/register")
          .json(&json!({ "username": "alice_ce", "password": "pw123456" }))
          .await
          .assert_status(StatusCode::CREATED);
      app.server
          .post("/api/session")
          .json(&json!({ "username": "alice_ce", "password": "pw123456" }))
          .await
          .assert_status_ok();

      let (cat_id, entry_a_id, entry_b_id) = app
          .db
          .user(|conn| {
              let user_id: i64 = conn
                  .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                  .unwrap();
              let cat =
                  rdrs::models::category::create_category(conn, user_id, "Engineering").unwrap();
              let feed1 = rdrs::models::feed::create_feed(
                  conn,
                  &rdrs::models::feed::CreateFeedParams {
                      category_id: cat.id,
                      url: "https://x/ce-feed-1",
                      title: Some("Feed 1"),
                      description: None,
                      site_url: None,
                      custom_user_agent: None,
                      http2_disabled: None,
                      custom_referrer: None,
                  },
              )
              .unwrap();
              let feed2 = rdrs::models::feed::create_feed(
                  conn,
                  &rdrs::models::feed::CreateFeedParams {
                      category_id: cat.id,
                      url: "https://x/ce-feed-2",
                      title: Some("Feed 2"),
                      description: None,
                      site_url: None,
                      custom_user_agent: None,
                      http2_disabled: None,
                      custom_referrer: None,
                  },
              )
              .unwrap();
              let (a, _) = rdrs::models::entry::upsert_entry(
                  conn,
                  feed1.id,
                  "guid-ce-a",
                  Some("Entry A"),
                  Some("https://x/ce/a"),
                  None,
                  None,
                  None,
                  None,
              )
              .unwrap();
              let (b, _) = rdrs::models::entry::upsert_entry(
                  conn,
                  feed2.id,
                  "guid-ce-b",
                  Some("Entry B"),
                  Some("https://x/ce/b"),
                  None,
                  None,
                  None,
                  None,
              )
              .unwrap();
              (cat.id, a.id, b.id)
          })
          .await
          .unwrap();

      let resp = app
          .server
          .get(&format!("/categories/{}/entries", cat_id))
          .await;
      assert_eq!(resp.status_code(), StatusCode::OK);
      let html = resp.text();

      assert!(html.contains("Engineering"), "page title must render the category name");
      assert!(
          html.contains(&format!("id=\"entry-row-{}\"", entry_a_id)),
          "row for entry from feed 1 must be present"
      );
      assert!(
          html.contains(&format!("id=\"entry-row-{}\"", entry_b_id)),
          "row for entry from feed 2 must be present"
      );
      assert!(
          !html.contains("rdrs-entries-page"),
          "SSR page must not mount the legacy CSR shell"
      );
      assert!(
          !html.contains("/static/js/pages/entries.js"),
          "SSR page must not load the legacy entries.js bundle"
      );
      assert!(
          html.contains("Select an entry to read."),
          "reading-pane placeholder must render"
      );
      if html.contains("id=\"load-more\"") {
          assert!(
              html.contains(&format!("action=\"/categories/{}/entries\"", cat_id)),
              "Load-More form must POST back to the category-scoped URL"
          );
      }
  }
  ```

- [ ] **Step 2: Run the test, confirm it fails.**

  Run: `cargo nextest run -E 'test(test_category_entries_page) - test(test_category_entries_page_not_found) - test(test_category_entries_page_other_user) - test(test_category_entries_page_load_more_fragment)'`
  Expected: FAIL.

- [ ] **Step 3: Rewrite `CategoryEntriesTemplate` and `category_entries_page`.**

  ```rust
  #[derive(Template)]
  #[template(path = "category_entries.html")]
  pub struct CategoryEntriesTemplate {
      pub title: String,
      pub git_version: &'static str,
      pub layout: AppLayoutContext,
      pub entries: Vec<EntryRowView>,
      pub reading_pane: Option<ReadingPaneView>,
      pub next_cursor: Option<i64>,
      pub entries_layout: EntriesLayoutContext,
  }

  /// `GET /categories/{id}/entries` — SSR list of entries from every feed
  /// in a single category. Supports the `?fragment=1&after=N` Load-More
  /// overload.
  pub async fn category_entries_page(
      auth_user: PageAuthUser,
      State(state): State<AppState>,
      Path(id): Path<i64>,
      AxumQuery(query): AxumQuery<EntriesQuery>,
      flash: Flash,
  ) -> Result<Response, AppError> {
      let user_id = auth_user.user.id;

      let category_name = state
          .db
          .read_user(move |c| {
              let cat = category::find_by_id_and_user(c, id, user_id)?
                  .ok_or(AppError::CategoryNotFound)?;
              Ok::<_, AppError>(cat.name)
          })
          .await??;

      let filter = entry::EntryFilter {
          category_id: Some(id),
          ..Default::default()
      };
      let sort = entry::EntrySortOrder::PublishedDesc;
      let page_size = ENTRIES_PAGE_SIZE;
      let offset = query.after.unwrap_or(0);

      let (entries, next_cursor) =
          build_entries_page(&state, user_id, filter, sort, page_size, offset).await;

      let path = format!("/categories/{}/entries", id);

      if query.fragment == Some(1) {
          let fragment = EntriesFragmentTemplate {
              entries,
              next_cursor,
              path: Box::leak(path.into_boxed_str()),
          };
          return Ok(fragment.into_response());
      }

      let layout = build_app_layout(&state, &auth_user, &flash).await;

      let template = CategoryEntriesTemplate {
          title: category_name,
          git_version: crate::GIT_VERSION,
          layout,
          entries,
          reading_pane: None,
          next_cursor,
          entries_layout: EntriesLayoutContext {
              active: "",
              description: None,
              empty_message: "No entries in this category.",
              path,
              show_tab_bar: false,
              show_mark_as_read: false,
          },
      };

      Ok((flash, template).into_response())
  }
  ```

- [ ] **Step 4: Rewrite `templates/category_entries.html`.**

  ```html
  {% extends "_entries_layout.html" %}
  ```

- [ ] **Step 5: Run the test, verify pass.**

  Run: `cargo nextest run -E 'test(test_category_entries_page) - test(test_category_entries_page_not_found) - test(test_category_entries_page_other_user) - test(test_category_entries_page_load_more_fragment)'`
  Expected: PASS.

- [ ] **Step 6: Update the 404 + cross-tenant tests.**

  Same pattern as Task 2 step 6 — trim to status-only assertions. `test_category_entries_page_not_found` becomes a one-liner GET on `/categories/999999/entries` → 404. `test_category_entries_page_other_user` seeds bob's category directly via SQL, alice gets 404.

- [ ] **Step 7: Add the Load-More fragment test.**

  Same shape as Task 2 step 7 — seed 3+ entries in the category, GET with `?fragment=1&after=0`, assert rows present + layout chrome absent.

- [ ] **Step 8: Run the full category-entries test set.**

  Run: `cargo nextest run -E 'test(/test_category_entries_page/)'`
  Expected: 4 tests PASS.

- [ ] **Step 9: Commit.**

  ```bash
  git add src/handlers/pages.rs templates/category_entries.html tests/pages_test.rs
  git commit -S -m "feat(ssr): SSR-first PR-11 — /categories/{id}/entries

Mirrors the /feeds/{id}/entries migration: \`build_entries_page\`
with \`EntryFilter::category_id\`, \`_entries_layout.html\` shell,
Load-More via \`?fragment=1&after=N\`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
  ```

---

## Task 4: Pre-merge polish

- [ ] **Step 1: Format.**

  Run: `cargo fmt`
  Expected: no diff (or whitespace-only).

- [ ] **Step 2: Clippy with deny warnings.**

  Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
  Expected: `Finished \`dev\` profile ...` with no warnings.

- [ ] **Step 3: Full test suite.**

  Run: `cargo nextest run --no-fail-fast 2>&1 | tail -8`
  Expected: 727+ tests, ≤2 failures (the known dirty-tree cache-control pair, which clears on the next commit).

- [ ] **Step 4: Push.**

  Run: `git push -u origin feat/ssr-feed-category-entries 2>&1 | tail -3`

- [ ] **Step 5: Open the PR.**

  Run:
  ```bash
  gh pr create --title "feat(ssr): SSR-first PR-11 — /feeds/{id}/entries + /categories/{id}/entries" --body "$(cat <<'EOF'
## Summary
- Migrate the last two CSR entries-family routes (`/feeds/{id}/entries`, `/categories/{id}/entries`) to SSR.
- Reuse PR-10's `build_entries_page` helper + `_entries_layout.html` shell verbatim. No new fragment endpoints, no new model functions.
- Bump `EntriesLayoutContext.path` from `&'static str` to `String` so the parameterized routes can pass their dynamic Load-More form action.

## Notes
- After this PR, every entries-family route is SSR. PR-12 will delete the legacy CSR scaffolding (`static/js/pages/entries.js`, `<rdrs-entries-page>`, `<rdrs-entry-list>`, `keyboard.js`, and the `GET /api/feeds` + `GET /api/entries/{id}` JSON endpoints they consumed).
- Pre-existing 404 and cross-tenant tests for both routes were trimmed to status-only assertions (their old positive-content checks referenced the CSR shell).

## Test plan
- [ ] `cargo nextest run` — full suite green except the known dirty-tree cache-control pair.
- [ ] Manual: open `/feeds/{id}/entries` and `/categories/{id}/entries`, confirm rows render server-side (view-source shows entry rows in the initial HTML), title link opens the reading pane via the existing swap helper, star/read/unread/mark-unread/save/fetch-full-content all work.
- [ ] Manual: scroll to the bottom of a list with >page-size entries, confirm Load-More appends more rows and POSTs to `?fragment=1&after=N` on the same scoped path.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
  ```

  Return the PR URL to the user. Then pause for their manual review per their standing instruction.

---

## Self-Review

**Spec coverage:**
- Spec § Migration Plan step 11: "`/feeds/{id}/entries`, `/categories/{id}/entries` SSR" — covered by Tasks 2 + 3.
- Spec § Endpoints: no new endpoints required for PR-11; fragment endpoints already exist. ✓
- Spec § Architecture / Server: "one `*.html` template + one handler per route" — Tasks 2 + 3 produce exactly that. ✓
- Spec § Repo conventions: handler returns a Template type, validates user access pre-render, fragment endpoint shares a partial. ✓

**Placeholder scan:** No "TBD", no "Similar to Task N" without inline code, every code block is concrete. ✓

**Type consistency:**
- `EntriesLayoutContext.path: String` (Task 1) — used in Tasks 2 + 3 as `path: format!(...)` and `path: path` (after move). ✓
- `EntriesFragmentTemplate.path: &'static str` is left as-is; Tasks 2 + 3 use `Box::leak` workaround. Noted in Task 2 Step 3 implementer notes. ✓
- `FeedEntriesTemplate.title: String` and `CategoryEntriesTemplate.title: String` are consistent with each other and with how they're used in `_entries_layout.html` (`{{ title }}` works for both `String` and `&str`). ✓
- `EntryFilter` field names (`feed_id`, `category_id`) match between the survey-quoted struct and the handler usage. ✓
- All test seeding code reuses the exact `CreateFeedParams` field names (already verified against existing handler tests on this branch). ✓
