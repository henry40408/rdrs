# SSR Keyset Pagination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the 7 server-rendered list pages from OFFSET pagination to the existing keyset (composite-cursor) machinery so deep-page loads stop costing O(k·page_size).

**Architecture:** Route every SSR list page through the shared `build_entries_page` builder, which switches from `entry::list_by_user(..OFFSET)` to `entry::list_by_user_with_continuation(..cursor)` (the same path the GReader API uses). The `after` request parameter changes from an integer offset to the existing opaque `"<sort_ts>|<id>"` cursor token; client JS and templates are otherwise unchanged. A prerequisite query-planner hint keeps page 0 fast.

**Tech Stack:** Rust, Axum, Askama templates, rusqlite (SQLite), axum-test for integration tests.

**Reference spec:** `docs/superpowers/specs/2026-06-11-ssr-keyset-pagination-design.md`

---

## File Structure

- `src/models/entry/mod.rs` — add the page-0 index hint to `list_by_user_with_continuation` (Task 1). Add a continuation-walk correctness test.
- `src/handlers/pages/mod.rs` — convert `build_entries_page` to a cursor builder; update its 13 call sites, the shared `EntriesQuery.after` field, and the 8 template `next_cursor` field types (Task 2).
- `tests/pages_test.rs` — fix the load-more fragment tests (drop `after=0`) and add a two-page cursor-walk test (Task 3).
- No template (`.html`) or `static/js/app.js` changes — `name="after" value="{{ after }}"` already renders a string.

---

## Task 1: Page-0 index hint on `list_by_user_with_continuation`

Without an `INDEXED BY` hint, the **cursorless** (page-0) continuation query drops to a `category→feed→entry` scan + temp B-tree sort (~60 ms at 50k entries). `list_by_user` already applies `published_sort_entry_hint`; mirror it here, gated to the page-0 case (`continuation.is_none()`) so the proven-fast deep-page path is untouched.

**Files:**
- Modify: `src/models/entry/mod.rs` (inside `list_by_user_with_continuation`, the `let sql = format!(...)` that starts `SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN} FROM entry e`)
- Test: `src/models/entry/mod.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — a continuation walk that pages through all entries with no gap/overlap, exercising the page-0 (cursorless, hinted) query first. Add inside `mod tests`:

```rust
    #[test]
    fn test_continuation_walk_is_gapless_unfiltered() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "walker");
        let category_id = create_test_category(&conn, user_id, "C");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/walk.xml");
        // 5 entries, distinct published_at so order is deterministic.
        for i in 0..5 {
            upsert_entry(
                &conn,
                feed_id,
                &format!("g{i}"),
                Some(&format!("T{i}")),
                None,
                None,
                None,
                None,
                Some(chrono::Utc.with_ymd_and_hms(2024, 1, 1 + i as u32, 0, 0, 0).unwrap()),
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut cursor: Option<ContinuationCursor> = None;
        let mut seen: Vec<i64> = Vec::new();
        loop {
            let params = ContinuationParams {
                oldest_first: false,
                limit: 3, // page size 2 + 1 sentinel
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let rows = list_by_user_with_continuation(&conn, user_id, &filter, &params).unwrap();
            let has_more = rows.len() > 2;
            let page = &rows[..rows.len().min(2)];
            if page.is_empty() {
                break;
            }
            for e in page {
                seen.push(e.entry.id);
            }
            if !has_more {
                break;
            }
            let last = page.last().unwrap();
            let ts = fetch_sort_ts(&conn, last.entry.id, EntrySortOrder::PublishedAt)
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite { sort_ts: ts, id: last.entry.id });
        }

        // All 5 seen exactly once, newest-first.
        assert_eq!(seen.len(), 5, "walk must visit every entry once: {seen:?}");
        let mut uniq = seen.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "no duplicates across pages: {seen:?}");
    }
```

Note: `chrono::TimeZone` must be in scope for `with_ymd_and_hms`. If the test module doesn't already `use chrono::TimeZone;`, add it at the top of `mod tests`.

- [ ] **Step 2: Run the test to verify it passes on current code** (it documents correctness that must survive the hint change)

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs continuation_walk_is_gapless_unfiltered`
Expected: PASS (the walk is already correct; the hint must not change results).

- [ ] **Step 3: Add the hint.** In `list_by_user_with_continuation`, immediately before the `let sql = format!(` that builds the main SELECT, insert:

```rust
    // Page-0 (cursorless) index hint. Without it the planner walks
    // category->feed->entry and temp-B-tree-sorts the whole corpus before
    // LIMIT. Mirrors `list_by_user`'s hint, but only when there is no
    // continuation predicate — at depth the predicate already drives the sort
    // index, so we leave that proven-fast plan untouched. Only the
    // published-order sorts have dedicated indexes.
    let entry_hint = if pagination.sort_order == EntrySortOrder::PublishedAt
        && pagination.continuation.is_none()
    {
        published_sort_entry_hint(filter)
    } else {
        ""
    };
```

- [ ] **Step 4: Inject the hint into the SQL.** Change the SELECT's `FROM entry e` line to `FROM entry e{}` and add `entry_hint` as the FIRST `format!` argument. The edit:

```rust
    let sql = format!(
        r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN}
        FROM entry e{}
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE {}
        ORDER BY {}
        LIMIT ?{}
        "#,
        entry_hint,
        where_clause,
        order,
        params_vec.len() + 1
    );
```

- [ ] **Step 5: Run the walk test + the GReader suite (shared function) to verify nothing broke**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs continuation_walk_is_gapless_unfiltered && cargo nextest run rdrs::greader_test`
Expected: PASS (results unchanged; the hint only changes the plan).

- [ ] **Step 6: Commit**

```bash
git add src/models/entry/mod.rs
git commit -S -m "perf(entries): page-0 index hint for continuation query

list_by_user_with_continuation lacked the INDEXED BY hint list_by_user
applies, so its cursorless (page-0) call temp-B-tree-sorted the whole corpus.
Apply published_sort_entry_hint gated on continuation.is_none(); the depth
path (cursor predicate already drives the index) is unchanged. Also fixes the
GReader stream/contents first-call cost."
```

---

## Task 2: Convert `build_entries_page` and its callers to cursors

One coherent type-cascade: the builder's return type changes from `Option<i64>` (next offset) to `Option<String>` (cursor token), which forces the shared `EntriesQuery.after` field and the 8 template `next_cursor` fields to change together so the crate compiles.

**Files:**
- Modify: `src/handlers/pages/mod.rs`:
  - `build_entries_page` (around line 203)
  - `EntriesQuery.after` (line 244)
  - 13 `build_entries_page` call sites (lines 494, 516, 1008, 1030, 1169, 1191, 1253, 1275, 1337, 1359, 1452, 1728)
  - 8 template `next_cursor` fields (lines 298, 2226, 2248, 2270, 2292, 2314, 2336, 2358)

- [ ] **Step 1: Rewrite `build_entries_page`.** Replace the whole function body. New version:

```rust
pub(crate) async fn build_entries_page(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> (Vec<EntryRowView>, Option<String>) {
    let result = state
        .db
        .read_user(move |conn| {
            let params = entry::ContinuationParams {
                oldest_first: false,
                limit: page_size + 1,
                continuation: cursor,
                ot: None,
                nt: None,
                sort_order: sort,
            };
            let rows = entry::list_by_user_with_continuation(conn, user_id, &filter, &params)?;
            let kept_len = rows.len().min(page_size as usize);
            // Derive the next cursor from the last *kept* row when an extra
            // (sentinel) row was returned. Mirrors greader/item.rs.
            let next = if rows.len() as i64 > page_size {
                match rows.get(kept_len - 1) {
                    Some(e) => entry::fetch_sort_ts(conn, e.entry.id, sort)?
                        .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id)),
                    None => None,
                }
            } else {
                None
            };
            let ids: Vec<i64> = rows.iter().take(kept_len).map(|e| e.entry.id).collect();
            let statuses = entry_summary::get_statuses_for_entries(conn, user_id, &ids)?;
            Ok::<_, AppError>((rows, kept_len, next, statuses))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_else(|| (Vec::new(), 0, None, HashMap::new()));
    let (rows, kept_len, next_cursor, statuses) = result;
    let views = rows
        .iter()
        .take(kept_len)
        .map(|e| row_view_from(e, statuses.get(&e.entry.id).copied()))
        .collect();
    (views, next_cursor)
}
```

- [ ] **Step 2: Change the shared query field.** `EntriesQuery.after` (line 244): `pub after: Option<i64>,` → `pub after: Option<String>,`.

- [ ] **Step 3: Update the two call-site shapes.** Every call site is one of two forms. The **full-page** render passes `0` today → pass `None`. The **fragment** path parses an offset today → parse a cursor.

Full-page form — change the final argument from `0` to `None`:

```rust
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        None,
    )
    .await;
```

Fragment form — replace the `let after = query.after.unwrap_or(0).max(0);` line with a cursor parse, and pass it:

```rust
    if query.fragment == Some(1) {
        let cursor = query.after.as_deref().and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        // ... existing EntriesFragmentTemplate { entries, next_cursor, .. } unchanged
```

Apply to all 13 sites. Full-page sites (pass `None`): 516, 1030, 1191, 1275, 1359. Fragment sites (parse cursor): 494, 1008, 1169, 1253, 1337. Feed/category sites: 1452 and 1728 currently read `let offset = query.after.unwrap_or(0);` then call `build_entries_page(.., offset)` for **both** full and fragment in one body — replace that line with `let cursor = query.after.as_deref().and_then(entry::ContinuationCursor::parse);` and pass `cursor`.

- [ ] **Step 4: Change the 8 template `next_cursor` field types** from `Option<i64>` to `Option<String>`:

```rust
    pub next_cursor: Option<String>,
```

Lines: 298 (`EntriesFragmentTemplate`), 2226 (`UnreadTemplate`), 2248 (`EntriesTemplate`), 2270 (`ReadEntriesTemplate`), 2292 (`StarredEntriesTemplate`), 2314 (`SummarizedEntriesTemplate`), 2336 (`FeedEntriesTemplate`), 2358 (`CategoryEntriesTemplate`). The handler assignments (`next_cursor,`) are unchanged — the type now flows from `build_entries_page`.

- [ ] **Step 5: Compile**

Run: `source /tmp/rdrs-env.sh && cargo build`
Expected: clean build, no errors. (If a call site was missed, the compiler flags the `i64` vs `Option<ContinuationCursor>` / `Option<i64>` vs `Option<String>` mismatch — fix it.)

- [ ] **Step 6: Commit**

```bash
git add src/handlers/pages/mod.rs
git commit -S -m "perf(pages): keyset pagination for SSR list pages

build_entries_page now drives list_by_user_with_continuation with a composite
cursor instead of OFFSET; the 'after' param carries the opaque <sort_ts>|<id>
token. Deep pages go from O(k*page_size) to a flat index range scan. Templates
and app.js unchanged (after is still an opaque echoed string)."
```

---

## Task 3: Fix and extend the pagination tests

The existing load-more fragment tests pass `?after=0`, which now parses as the legacy `e.id < 0` cursor (empty). Switch them to fetch the fragment with no cursor (first page), and add a real two-page keyset walk.

**Files:**
- Modify: `tests/pages_test.rs` (`test_category_entries_page_load_more_fragment` ~line 1226, `test_feed_entries_page_load_more_fragment` ~line 1733)
- Test (new): `tests/pages_test.rs`

- [ ] **Step 1: Drop `&after=0` from the two fragment tests.** In `test_category_entries_page_load_more_fragment`, change:

```rust
        .get(&format!("/categories/{}/entries?fragment=1&after=0", cat_id))
```
to
```rust
        .get(&format!("/categories/{}/entries?fragment=1", cat_id))
```

In `test_feed_entries_page_load_more_fragment`, change:

```rust
        .get(&format!("/feeds/{}/entries?fragment=1&after=0", feed_id))
```
to
```rust
        .get(&format!("/feeds/{}/entries?fragment=1", feed_id))
```

- [ ] **Step 2: Run them to confirm they pass** (cursor `None` → first page renders rows)

Run: `source /tmp/rdrs-env.sh && cargo nextest run rdrs::pages_test load_more_fragment`
Expected: PASS.

- [ ] **Step 3: Add a two-page keyset walk test.** Append to `tests/pages_test.rs`. Seeds 60 unread entries (page size 50), loads `/`, extracts the Load-More cursor token from the form, fetches the fragment with it, and asserts the second page is disjoint from the first and carries a composite (`|`) token.

```rust
#[tokio::test]
async fn test_unread_load_more_uses_keyset_cursor() {
    let app = create_test_app_named(default_test_config(), "test_unread_keyset");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "kuser", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "kuser", "password": "pw123456" }))
        .await
        .assert_status_ok();

    app.db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |r| r.get(0))
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, user_id, "K").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/keyset-feed",
                    title: Some("K Feed"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            for i in 0..60 {
                rdrs::models::entry::upsert_entry(
                    conn,
                    feed.id,
                    &format!("kg-{i}"),
                    Some(&format!("K {i}")),
                    None,
                    None,
                    None,
                    None,
                    Some(chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, i / 60, i % 60).unwrap()),
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    // Page 1 (full render): first 50 rows + a Load-More form with a cursor.
    let html = app.server.get("/").await.text();
    let entry_ids_page1: std::collections::HashSet<String> = extract_entry_ids(&html);
    assert_eq!(entry_ids_page1.len(), 50, "page 1 shows the first 50");

    // Extract the cursor token from the Load-More form's hidden `after` input.
    let cursor = extract_after_value(&html).expect("Load-More form must carry an after cursor");
    assert!(cursor.contains('|'), "cursor is a composite token, got {cursor:?}");

    // Page 2 (fragment) via the cursor.
    let frag = app
        .server
        .get(&format!("/?fragment=1&after={}", urlencoding::encode(&cursor)))
        .await
        .text();
    let entry_ids_page2 = extract_entry_ids(&frag);
    assert_eq!(entry_ids_page2.len(), 10, "page 2 shows the remaining 10");
    assert!(
        entry_ids_page1.is_disjoint(&entry_ids_page2),
        "keyset pages must not overlap"
    );
}

// Pull `data-entry-id="N"` values out of rendered HTML.
fn extract_entry_ids(html: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let needle = "data-entry-id=\"";
    let mut rest = html;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        if let Some(j) = rest.find('"') {
            out.insert(rest[..j].to_string());
            rest = &rest[j..];
        }
    }
    out
}

// Pull the value of the Load-More form's hidden `after` input.
fn extract_after_value(html: &str) -> Option<String> {
    let needle = "name=\"after\" value=\"";
    let i = html.find(needle)? + needle.len();
    let rest = &html[i..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}
```

Note: confirm `urlencoding` is a dev-dependency; if not, replace `urlencoding::encode(&cursor)` with a manual `cursor.replace('|', "%7C").replace(':', "%3A")` (the token is `YYYY-MM-DD HH:MM:SS|id` — also encode the space as `%20`). Simplest robust form: `cursor.replace(' ', "%20").replace('|', "%7C")`.

- [ ] **Step 4: Run the new test**

Run: `source /tmp/rdrs-env.sh && cargo nextest run rdrs::pages_test test_unread_load_more_uses_keyset_cursor`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/pages_test.rs
git commit -S -m "test(pages): keyset cursor pagination for SSR list pages

Drop the legacy after=0 inputs (now an empty id-cursor) and add a two-page
walk asserting disjoint pages and a composite cursor token."
```

---

## Task 4: Full verification (perf rule + suite)

Before/after benchmark on the implemented code (per the user's perf rule: no conversion lands if any page shape regresses at page 0), then the full gate.

**Files:** none committed (throwaway bench is deleted).

- [ ] **Step 1: Throwaway benchmark.** Create `examples/_verify_keyset.rs` that seeds 50k entries (10 categories / 100 feeds, 50% read, 8% starred) and times the **implemented** `build_entries_page` path indirectly via the real `entry::list_by_user` (OFFSET, baseline) vs `entry::list_by_user_with_continuation` (the new path, now hinted) for filters unread/all/read/starred at page 0 and page 200, printing OFFSET vs keyset ms and a regression flag (`key > base*1.15`). (Reuse the structure from the design-phase bench; it lived only as a throwaway.)

- [ ] **Step 2: Run it and confirm no page-0 regression**

Run: `source /tmp/rdrs-env.sh && cargo run --release --example _verify_keyset`
Expected: every page-0 row `ok` (keyset ≤ ~1.15× OFFSET); deep pages show keyset far faster. Capture the output for the PR body.

- [ ] **Step 3: Delete the throwaway bench**

```bash
rm -f examples/_verify_keyset.rs && rmdir examples 2>/dev/null || true
```

- [ ] **Step 4: Full suite + lints + format**

Run:
```bash
source /tmp/rdrs-env.sh && cargo fmt && cargo nextest run && cargo clippy --all-targets
```
Expected: all tests pass, clippy clean. If `cargo fmt` changed files, `git add` them by name and amend or add a `style:` commit.

- [ ] **Step 5: e2e smoke (Load-More flow unchanged)**

Run: `cd e2e && npm test -- reading` (or the project's e2e command; see `e2e/`)
Expected: the Load-More / reading-pane scenarios pass (cursor value is opaque to them).

- [ ] **Step 6: Open the PR** with the before/after numbers from Step 2 in the body. Base `main`, head `perf/ssr-keyset-pagination`. (Do not merge — await explicit confirmation.)

---

## Self-Review notes (author)

- **Spec coverage:** §"Mandatory hint" → Task 1; §"build_entries_page" + §"Query/template types" + §"Client JS (none)" → Task 2; §Testing → Task 3; §Verification (perf rule) → Task 4; §Scope (Search excluded) → no task touches Search. ✓
- **Type consistency:** `build_entries_page(.., cursor: Option<ContinuationCursor>) -> (Vec<EntryRowView>, Option<String>)`; `EntriesQuery.after: Option<String>`; all 8 `next_cursor: Option<String>`. Cursor parse `ContinuationCursor::parse(&str) -> Option<ContinuationCursor>`; emit `encode_composite(&str, i64) -> String`; `fetch_sort_ts(conn, i64, EntrySortOrder) -> AppResult<Option<String>>`. Consistent across tasks. ✓
- **Back-compat:** legacy `after=<int>` → `LegacyId` grace path (spec-accepted); `after=0`/garbage → first page. Tests updated accordingly. ✓
