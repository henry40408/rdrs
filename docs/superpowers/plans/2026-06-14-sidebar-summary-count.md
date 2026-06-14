# Sidebar Completed-Summary Count Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "Summarized" nav item to the sidebar with a badge showing the count of the user's completed summaries.

**Architecture:** Thread one global integer `total_summarized` through the existing unread-count plumbing (`entry_summary::count_completed` → `read_chrome_data` cached closure → `ChromeData`/`SidebarResponse` → `/api/sidebar` + `#rdrs-sidebar-bootstrap`). The CSR `<rdrs-sidebar>` renders the new item + badge and patches it on revalidate. Bust the chrome cache when a summary completes (worker) or is dismissed.

**Tech Stack:** Rust (Axum, rusqlite, Askama), vanilla-JS custom element (`static/js/components/rdrs-sidebar.js`), Playwright-BDD e2e.

---

## Notes for the implementer

- **NixOS box:** `source /tmp/rdrs-env.sh` before EVERY cargo/e2e command.
- **`pwd` first**; expect `/home/nixos/Develop/claude/rdrs`.
- **Tests:** `cargo nextest run` (NOT `cargo test`), `RDRS_FAST_HASH=1`. `cargo fmt` before commit. CI runs `cargo clippy -- -D warnings` — keep it clean.
- **e2e:** run from `e2e/`; **`cargo build` first** (CSS/JS are served from the binary's static dir — actually JS is served from `static/` on disk, but the e2e harness builds the binary; rebuild after Rust changes). `npx bddgen` runs automatically via `npx playwright test`.
- **Commits GPG-signed** (`git commit -S`); trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage files explicitly; never `git add -A`/`.`. No version/CHANGELOG edits.
- Current branch is `feat/sidebar-summary-count` (already created).

## Verified facts

- `entry_summary` table: columns include `user_id`, `entry_id`, `status` (`'pending'|'processing'|'completed'|'failed'`). Index `idx_entry_summary_user_status (user_id, status)`.
- `CachedChrome` (`src/services/sidebar_cache.rs:11`): `{ theme: Option<String>, categories: Vec<SidebarCategoryDto>, total_unread: i64 }`. `SidebarCache::bust(&self, user_id: i64)`. `AppState.sidebar_cache: Arc<SidebarCache>`.
- `ChromeData` + `SidebarResponse` in `src/handlers/user.rs`. `read_chrome_data` builds `CachedChrome` + `ChromeData` (cache-hit branch ~line 190 and fresh branch). `build_sidebar_response` (line 245) maps `ChromeData` → `SidebarResponse`. `get_sidebar` (line 279) calls `build_sidebar_response`.
- `serialize_sidebar_for_script` (`src/handlers/pages/script_json.rs:14`) does `serde_json::to_string(&SidebarResponse)` → bootstrap field flows automatically. **No change needed there.**
- Summary worker `src/services/summary_worker.rs`: `start_summary_worker(rx, cache: Arc<SummaryCache>, db: DbPool, cancel_token)`; `process_summary_job(job, cache, db)` calls `entry_summary::set_completed(conn, user_id, entry_id, &summary_text)` (~line 131) inside a `db.user(...)` closure. Started in `src/main.rs`.
- Dismiss: `delete_entry_summary` (`src/handlers/entry.rs:285`) has `State(state)`, calls `entry_summary::delete`, then `state.summary_cache.remove`.
- CSR sidebar `static/js/components/rdrs-sidebar.js`: `_updateBadges(data)` (~line 103) patches `#unread-count`; `render(data)` (~line 130) builds the nav. First section has Unread / Starred / All Entries; All Entries active set is `['all','read','summarized','entries']`. e2e/seed helper `insertSummary(entryId, userId, text)` inserts a `status='completed'` row.

---

## Task 1: `entry_summary::count_completed`

**Files:**
- Modify: `src/models/entry_summary.rs` (add fn + unit test in its `mod tests`)

- [ ] **Step 1: Write the failing test**

Append inside the `mod tests` block of `src/models/entry_summary.rs` (mirror the
existing tests' setup — they create an in-memory DB via the crate's schema and
insert summaries with `set_completed` / `upsert_pending`):

```rust
    #[test]
    fn count_completed_counts_only_completed_for_user() {
        let conn = test_conn(); // same helper the existing tests use to build schema
        // user 1: two completed, one pending, one failed
        super::set_completed(&conn, 1, 101, "s").unwrap();
        super::set_completed(&conn, 1, 102, "s").unwrap();
        super::upsert_pending(&conn, 1, 103).unwrap();
        super::set_failed(&conn, 1, 104, "e").unwrap();
        // user 2: one completed (must not leak into user 1's count)
        super::set_completed(&conn, 2, 201, "s").unwrap();

        assert_eq!(super::count_completed(&conn, 1).unwrap(), 2);
        assert_eq!(super::count_completed(&conn, 2).unwrap(), 1);
        assert_eq!(super::count_completed(&conn, 999).unwrap(), 0);
    }
```

If the existing tests use a different conn-builder than `test_conn()`, use
whatever they use (read the top of the `mod tests` block first and match it).

- [ ] **Step 2: Run to verify it FAILS**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs count_completed 2>&1 | tail -15
```
Expected: compile error — `count_completed` not found.

- [ ] **Step 3: Implement `count_completed`**

Add to `src/models/entry_summary.rs` (near the other query fns):

```rust
/// Count the user's entries that have a COMPLETED summary. Index-covered by
/// `idx_entry_summary_user_status`. Used for the sidebar "Summarized" badge.
pub fn count_completed(conn: &Connection, user_id: i64) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entry_summary WHERE user_id = ?1 AND status = 'completed'",
        [user_id],
        |row| row.get(0),
    )?;
    Ok(count)
}
```

(Confirm `Connection` and `AppResult` are already imported in the file — they are,
used by the other fns.)

- [ ] **Step 4: Run to verify PASS**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs count_completed 2>&1 | tail -15
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/models/entry_summary.rs
git commit -S -m "feat(entry_summary): add count_completed query

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Thread `total_summarized` through the chrome/sidebar payload

**Files:**
- Modify: `src/services/sidebar_cache.rs` (`CachedChrome`)
- Modify: `src/handlers/user.rs` (`ChromeData`, `SidebarResponse`, `read_chrome_data`, `build_sidebar_response`)
- Modify: `tests/handlers_test.rs` (extend the `/api/sidebar` test)

- [ ] **Step 1: Add the failing handler assertion**

Find the existing `/api/sidebar` test (`test_api_sidebar_returns_categories_with_unread`
in `tests/handlers_test.rs`). After it seeds data, seed a completed summary and
assert the response JSON has `total_summarized`. Add near its existing assertions:

```rust
    // Seed a completed summary for one of the user's entries, then assert the
    // sidebar payload reports it.
    app.db
        .user(move |conn| {
            rdrs::models::entry_summary::set_completed(conn, user_id, entry_id, "sum")
        })
        .await
        .unwrap()
        .unwrap();
    // bust so the cached chrome is recomputed (the seed bypassed handler busts)
    app.state.sidebar_cache.bust(user_id);
    let body: serde_json::Value = app.server.get("/api/sidebar").await.json();
    assert_eq!(body["total_summarized"], 1);
```

Adapt `user_id` / `entry_id` / how the test accesses `app.state` / `app.db` to the
test's existing helpers (read the test first). If the test has no handle to a
seeded entry id, seed a feed+entry like the other handler tests do.

- [ ] **Step 2: Run to verify it FAILS**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_api_sidebar 2>&1 | tail -15
```
Expected: FAIL — `total_summarized` is null / field missing.

- [ ] **Step 3: Add the field to `CachedChrome`**

In `src/services/sidebar_cache.rs`, add to `CachedChrome` (after `total_unread`):

```rust
    pub total_unread: i64,
    pub total_summarized: i64,
```

- [ ] **Step 4: Add the field to `ChromeData` and `SidebarResponse`**

In `src/handlers/user.rs`:
- `SidebarResponse` (after `total_unread`): `pub total_summarized: i64,`
- `ChromeData` (after `total_unread`): `pub total_summarized: i64,`

- [ ] **Step 5: Populate it in `read_chrome_data`**

In `read_chrome_data`:
1. In the **cache-hit** branch (the `if let Some(cached) = ... { return ChromeData { ... } }`), add `total_summarized: cached.total_summarized,`.
2. In the **fresh** `read_user` closure, add the query and include it in the constructed `CachedChrome`:
   ```rust
   let total_summarized =
       crate::models::entry_summary::count_completed(conn, user_id).unwrap_or(0);
   ```
   and in the `CachedChrome { theme, categories, total_unread }` literal add `total_summarized,`.
3. In the **final** `ChromeData { ... }` return (the fresh branch), add `total_summarized: fresh.total_summarized,`.
4. The `unwrap_or_default()` on the fresh tuple relies on `CachedChrome: Default` — `total_summarized: i64` defaults to 0, fine.

- [ ] **Step 6: Map it in `build_sidebar_response`**

In `build_sidebar_response`, add to the `SidebarResponse { ... }` literal:

```rust
        total_unread: chrome.total_unread,
        total_summarized: chrome.total_summarized,
```

- [ ] **Step 7: Run to verify PASS + clippy**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_api_sidebar 2>&1 | tail -15
cargo clippy -- -D warnings 2>&1 | tail -8
```
Expected: PASS; clippy clean.

- [ ] **Step 8: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/services/sidebar_cache.rs src/handlers/user.rs tests/handlers_test.rs
git commit -S -m "feat(sidebar): thread total_summarized through chrome payload

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Bust the chrome cache when summaries change

**Files:**
- Modify: `src/handlers/entry.rs` (`delete_entry_summary`)
- Modify: `src/services/summary_worker.rs` (`start_summary_worker`, `process_summary_job`)
- Modify: `src/main.rs` (pass the sidebar cache into the worker)

- [ ] **Step 1: Bust on dismiss**

In `src/handlers/entry.rs`, in `delete_entry_summary`, after
`state.summary_cache.remove(user_id, id);` add:

```rust
    // The completed-summary count dropped — refresh the sidebar badge.
    state.sidebar_cache.bust(user_id);
```

- [ ] **Step 2: Plumb the sidebar cache into the summary worker**

In `src/services/summary_worker.rs`:
1. Add import: `use crate::services::sidebar_cache::SidebarCache;` (confirm the module path; `SidebarCache` lives in `crate::services::sidebar_cache`).
2. Change `start_summary_worker` signature to accept the cache:
   ```rust
   pub fn start_summary_worker(
       mut rx: mpsc::Receiver<SummaryJob>,
       cache: Arc<SummaryCache>,
       sidebar_cache: Arc<SidebarCache>,
       db: DbPool,
       cancel_token: CancellationToken,
   ) -> JoinHandle<()> {
   ```
3. Thread `sidebar_cache` into both `process_summary_job(&job, &cache, &db)` call sites — change to `process_summary_job(&job, &cache, &sidebar_cache, &db)` (the normal path and the cancel-drain path). Clone as needed for the `async move` (the closure already moves `cache`, `db`; add `let sidebar_cache = sidebar_cache;` move).
4. Change `process_summary_job` signature:
   ```rust
   async fn process_summary_job(
       job: &SummaryJob,
       cache: &Arc<SummaryCache>,
       sidebar_cache: &Arc<SidebarCache>,
       db: &DbPool,
   ) {
   ```
5. After the DB `set_completed` succeeds (the branch around line 125-135 that does `cache.set_completed(...)` + the `db.user(... entry_summary::set_completed ...)`), bust the sidebar cache for that user:
   ```rust
   // A summary just completed — the sidebar "Summarized" badge must tick up.
   sidebar_cache.bust(job.user_id);
   ```
   Place it in the success path only (not on `set_failed`).

- [ ] **Step 3: Update the worker start site in `main.rs`**

In `src/main.rs`, find the `start_summary_worker(...)` call and pass the sidebar
cache (the `AppState`'s `sidebar_cache`, an `Arc<SidebarCache>`), e.g.:

```rust
    let summary_worker_handle = services::start_summary_worker(
        summary_rx,
        state.summary_cache.clone(),
        state.sidebar_cache.clone(),
        state.db.clone(),
        cancel_token.clone(),
    );
```
Match the actual variable names at the call site (read it first).

- [ ] **Step 4: Fix the worker's own tests**

`src/services/summary_worker.rs` has tests that call `start_summary_worker` /
`process_summary_job`. Update them to pass an `Arc<SidebarCache>`
(`Arc::new(SidebarCache::new(...))` — match `SidebarCache`'s constructor; read
`sidebar_cache.rs` for it). The bust is a no-op cache invalidation, so the tests'
assertions on summary status are unaffected.

- [ ] **Step 5: Build + test + clippy**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
cargo build 2>&1 | tail -6
RDRS_FAST_HASH=1 cargo nextest run -p rdrs summary 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -8
```
Expected: clean build/clippy; summary worker + summary tests PASS.

- [ ] **Step 6: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/handlers/entry.rs src/services/summary_worker.rs src/main.rs
git commit -S -m "feat(sidebar): bust chrome cache on summary complete/dismiss

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Render the "Summarized" nav item + badge (CSR)

**Files:**
- Modify: `static/js/components/rdrs-sidebar.js`

- [ ] **Step 1: Add the item to `render()`**

In `render(data)`, after `const totalUnread = data ? data.total_unread : 0;` add:

```javascript
        const totalSummarized = data ? data.total_summarized : 0;
```

In the first `.sidebar-section`, insert a new item **between** the "Starred" item
and the "All Entries" item:

```html
            <a href="/entries/summarized" class="sidebar-item${isActive('summarized')}" data-testid="nav-summarized">
                <span class="sidebar-item-icon">&#x2728;</span>
                <span>Summarized</span>
                <span class="sidebar-badge" id="summarized-count">${totalSummarized > 0 ? totalSummarized : ''}</span>
            </a>
```

- [ ] **Step 2: Fix the All-Entries active set**

The "All Entries" item currently uses
`${['all', 'read', 'summarized', 'entries'].includes(active) ? ' active' : ''}`.
Remove `'summarized'` so the new dedicated item owns that active state:

```javascript
            <a href="/entries" class="sidebar-item${['all', 'read', 'entries'].includes(active) ? ' active' : ''}" data-testid="nav-entries">
```

- [ ] **Step 3: Patch the badge on revalidate**

In `_updateBadges(data)`, after the `#unread-count` block (before the
`#sidebar-categories` lookup), add:

```javascript
        const sumEl = this.querySelector('#summarized-count');
        if (sumEl) {
            const sum = data.total_summarized || 0;
            sumEl.textContent = sum > 0 ? String(sum) : '';
        }
```

- [ ] **Step 4: Build + manual sanity (no JS unit harness)**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -3
```
JS has no unit-test harness in this repo; behavior is covered by the e2e in Task 5.
Quick syntax check (optional): `node --check static/js/components/rdrs-sidebar.js`.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add static/js/components/rdrs-sidebar.js
git commit -S -m "feat(sidebar): render Summarized nav item with count badge

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: e2e — sidebar shows the Summarized item + badge

**Files:**
- Modify: `e2e/features/*.feature` (add a scenario — pick the feature file that already exercises the sidebar; likely `reading.feature` or a `sidebar`/`responsive` feature) and a step if needed.

- [ ] **Step 1: Inspect existing sidebar e2e + seed helper**

```bash
cd /home/nixos/Develop/claude/rdrs
grep -rn "nav-unread\|nav-starred\|nav-entries\|sidebar-badge\|insertSummary\|main-nav" e2e/features e2e/steps e2e/support | head
```
Identify the feature/steps that already assert sidebar items and the
`insertSummary` seed helper (`e2e/support/seed.js`).

- [ ] **Step 2: Add a scenario**

Append to the most appropriate existing feature file (one that has a Background
seeding a feed + entries and a logged-in session). Use existing step phrasings;
only the assertions below are new — reuse a generic "I see" / locator step if one
exists, else add a minimal step in the matching steps file:

```gherkin
  Scenario: Sidebar shows a Summarized count badge
    Given I have a feed with 5 test entries
    And entry "Test Entry 1" has a completed summary
    When I open the inbox
    Then the sidebar "Summarized" item shows a count of "1"
    And clicking the sidebar "Summarized" item lands on the summarized view
```

If those exact steps don't exist, implement them in the matching `*.steps.js`:
- `Given("entry {string} has a completed summary", …)` → use the `SeedHelper.insertSummary(entryId, userId, text)` (resolve the entry id by title + the logged-in user id, mirroring how other steps resolve them).
- `Then("the sidebar {string} item shows a count of {string}", …)` → locate
  `[data-testid="nav-summarized"] #summarized-count` (or `.sidebar-badge` within
  the item) and assert its text equals the count.
- `Then("clicking the sidebar {string} item lands on the summarized view", …)` →
  click `[data-testid="nav-summarized"]`, assert URL `/entries/summarized`.

Prefer reusing existing step infrastructure; only add steps that are genuinely missing.

- [ ] **Step 3: Build + run**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -3
cd e2e && npx playwright test <feature-file-you-edited> 2>&1 | tail -25
```
Expected: the new scenario PASSES (badge "1", click → `/entries/summarized`).
If the badge is empty: confirm the bootstrap payload carries `total_summarized`
(Task 2) and the seed inserted a `completed` row before the page loaded; the
sidebar reads the bootstrap synchronously on mount.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/features/<file>.feature e2e/steps/<file>.steps.js
git commit -S -m "test(e2e): assert sidebar Summarized count badge

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Full regression sweep

**Files:** none (verification)

- [ ] **Step 1: Rust + clippy + fmt**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt --check
RDRS_FAST_HASH=1 cargo nextest run -p rdrs 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -6
```
Expected: all pass, clean.

- [ ] **Step 2: e2e — sidebar-touching + triage (summarize) suites**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -3
cd e2e && npx playwright test reading triage responsive 2>&1 | tail -25
```
Expected: all pass (sidebar still renders; existing nav items + summarize flow
unaffected). The new Summarized item must not break sidebar layout/active-state
assertions; if a test asserted the sidebar item set, update it to include the new
item.

- [ ] **Step 3: Fix any regressions and re-run.**

---

## Self-Review

**Spec coverage:**
- New "Summarized" nav item (after Starred) + badge → Task 4. ✅
- `count_completed` (status='completed', per-user, index-covered) → Task 1. ✅
- `total_summarized` through CachedChrome/ChromeData/SidebarResponse/read_chrome_data/build_sidebar_response → Task 2. ✅
- Bootstrap carries it automatically (serialize whole struct) → no task needed (verified). ✅
- `/api/sidebar` includes it → Task 2 (via build_sidebar_response) + test. ✅
- CSR surgical badge patch (`#summarized-count`) → Task 4 Step 3. ✅
- Active-state: move `'summarized'` off All Entries onto the new item → Task 4 Step 2. ✅
- Cache bust on complete (worker) + dismiss → Task 3. ✅
- View filter unchanged → no task touches `filters.rs`. ✅
- Tests: model count, /api/sidebar payload, e2e badge+nav → Tasks 1, 2, 5. ✅

**Placeholder scan:** No TBD/TODO. The two "match the test/seed helper to what
exists" notes are concrete instructions (read the existing helper, mirror it), not
deferrals.

**Type/name consistency:** `count_completed(conn, user_id) -> AppResult<i64>` (Task 1)
called in Task 2 read_chrome_data. `total_summarized: i64` added consistently to
CachedChrome (Task 2.3), ChromeData (2.4), SidebarResponse (2.4), and read in
build_sidebar_response (2.6) and JS `data.total_summarized` (Task 4). `SidebarCache`
plumbed as `Arc<SidebarCache>` into the worker (Task 3) matching `AppState.sidebar_cache`.
JS ids `#summarized-count` / testid `nav-summarized` consistent between Task 4 and Task 5.
