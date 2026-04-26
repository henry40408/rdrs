# Composite Cursor Pagination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the bare `e.id < c` pagination cursor with a `(sort_ts, id)` composite cursor so backfill / OPML imports / out-of-order timestamps no longer cause silent entry skips on Load More.

**Architecture:** New `ContinuationCursor` enum (`Composite` + `LegacyId` grace path) parsed from the existing opaque cursor string. SQL predicate uses bounded-OR form `sort_ts_expr <= ?ts AND (sort_ts_expr < ?ts OR e.id < ?id)` (PoC-confirmed planner-friendly). New expression index `idx_entry_sort_ts ON entry(COALESCE(published_at, created_at))` keeps the `PublishedAt` path on an indexed range scan. Cursor format `<iso_8601_ts>|<id>`.

**Tech Stack:** Rust, axum, rusqlite, chrono, askama (SSR), Playwright (E2E).

**Spec:** `docs/superpowers/specs/2026-04-26-composite-cursor-pagination-design.md`

---

### Task 1: Add expression index `idx_entry_sort_ts`

**Files:**
- Modify: `src/db/schema.rs` (after the existing `CREATE INDEX IF NOT EXISTS idx_entry_starred_at ...` line, ~line 82)

- [ ] **Step 1: Add the index DDL**

In `src/db/schema.rs`, after line 82 (`CREATE INDEX IF NOT EXISTS idx_entry_starred_at ON entry(starred_at);`), add:

```sql
        CREATE INDEX IF NOT EXISTS idx_entry_sort_ts ON entry(COALESCE(published_at, created_at));
```

(Mind indentation — match the surrounding lines.)

- [ ] **Step 2: Verify build**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo build`
Expected: clean build, no warnings about the new line.

- [ ] **Step 3: Verify existing tests still pass**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: all pass (no behavior change yet — just an extra index built at startup).

- [ ] **Step 4: Commit**

```bash
git add src/db/schema.rs
git commit -m "feat(db): add expression index on COALESCE(published_at, created_at)

Foundation for #164 composite cursor: lets the planner use an indexed
range scan for PublishedAt-sorted pagination predicates that reference
the COALESCE expression. Idempotent (CREATE INDEX IF NOT EXISTS), built
at startup."
```

---

### Task 2: Add `ContinuationCursor` type with parse/encode (TDD)

**Files:**
- Modify: `src/models/entry.rs` (add new type near `ContinuationParams` ~line 60; add tests in the existing `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests { ... }` block in `src/models/entry.rs`:

```rust
    #[test]
    fn cursor_parses_composite_format() {
        let c = ContinuationCursor::parse("2026-04-26 12:34:56|142")
            .expect("composite parses");
        match c {
            ContinuationCursor::Composite { sort_ts, id } => {
                assert_eq!(sort_ts, "2026-04-26 12:34:56");
                assert_eq!(id, 142);
            }
            _ => panic!("expected Composite"),
        }
    }

    #[test]
    fn cursor_parses_bare_i64_as_legacy() {
        let c = ContinuationCursor::parse("142").expect("legacy parses");
        match c {
            ContinuationCursor::LegacyId(id) => assert_eq!(id, 142),
            _ => panic!("expected LegacyId"),
        }
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(ContinuationCursor::parse("not-a-cursor").is_none());
        assert!(ContinuationCursor::parse("|").is_none());
        assert!(ContinuationCursor::parse("ts|notnum").is_none());
        assert!(ContinuationCursor::parse("").is_none());
    }

    #[test]
    fn cursor_encodes_composite() {
        assert_eq!(
            ContinuationCursor::encode_composite("2026-04-26 12:34:56", 142),
            "2026-04-26 12:34:56|142"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -p rdrs --test-threads 1 -E 'test(cursor_)'`
Expected: compilation error — `ContinuationCursor` not in scope.

- [ ] **Step 3: Implement `ContinuationCursor`**

In `src/models/entry.rs`, immediately before `pub struct ContinuationParams` (around line 60), add:

```rust
/// Pagination cursor. The wire format on the API is opaque to clients; we
/// emit the new composite form `<iso_8601_ts>|<id>` and accept the legacy
/// bare-`i64` form as a one-time grace path for in-flight cursors that may
/// still live in browser URLs/JS state at deploy time.
#[derive(Debug, Clone)]
pub enum ContinuationCursor {
    /// New `(sort_ts, id)` composite. `sort_ts` is the entry's sort-field
    /// value as TEXT (the same byte-string SQLite stores), so the predicate
    /// compares against an indexed column without conversion.
    Composite { sort_ts: String, id: i64 },
    /// Legacy `e.id < ?` cursor — accepted on input only; emitted only by
    /// pre-#164 clients.
    LegacyId(i64),
}

impl ContinuationCursor {
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        if let Some((ts, id)) = s.split_once('|') {
            if ts.is_empty() {
                return None;
            }
            id.parse::<i64>().ok().map(|id| Self::Composite {
                sort_ts: ts.to_string(),
                id,
            })
        } else {
            s.parse::<i64>().ok().map(Self::LegacyId)
        }
    }

    pub fn encode_composite(sort_ts: &str, id: i64) -> String {
        format!("{}|{}", sort_ts, id)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(cursor_)'`
Expected: all 4 cursor tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry.rs
git commit -m "feat(entry): ContinuationCursor type with composite/legacy parse

Wire format is opaque to clients; new composite form '<iso_8601_ts>|<id>'
plus a one-time grace path that accepts bare-i64 for in-flight cursors
left over from pre-#164 deployments. Pure parsing/encoding logic with
unit-test coverage; integration with the SQL builder follows."
```

---

### Task 3: Add `fetch_sort_ts` helper for cursor emission

**Files:**
- Modify: `src/models/entry.rs` (add after `find_by_id_with_feed`, ~line 165)
- Test: `src/models/entry.rs` (existing `tests` module)

**Why:** The handlers emit a cursor from the boundary entry's sort-field value. We need that value as the **exact string SQLite stored** (not via re-serializing `DateTime<Utc>`, which can produce a different format from `published_at` columns set by feed-parser inserts vs. SQLite's `datetime('now')` defaults). A tiny helper queries it AS TEXT.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/models/entry.rs`:

```rust
    #[test]
    fn fetch_sort_ts_returns_published_or_created_for_publishedat() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        // Entry with published_at set
        conn.execute(
            "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g1", "2026-04-01 10:00:00"],
        ).unwrap();
        let id1: i64 = conn.last_insert_rowid();

        // Entry with published_at NULL → COALESCE falls back to created_at
        conn.execute(
            "INSERT INTO entry (feed_id, guid, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g2", "2026-04-02 11:00:00"],
        ).unwrap();
        let id2: i64 = conn.last_insert_rowid();

        let ts1 = fetch_sort_ts(&conn, id1, EntrySortOrder::PublishedAt).unwrap();
        let ts2 = fetch_sort_ts(&conn, id2, EntrySortOrder::PublishedAt).unwrap();
        assert_eq!(ts1.as_deref(), Some("2026-04-01 10:00:00"));
        assert_eq!(ts2.as_deref(), Some("2026-04-02 11:00:00"));
    }

    #[test]
    fn fetch_sort_ts_returns_read_at_for_readat_sort() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        conn.execute(
            "INSERT INTO entry (feed_id, guid, read_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g1", "2026-04-03 12:00:00"],
        ).unwrap();
        let id: i64 = conn.last_insert_rowid();

        let ts = fetch_sort_ts(&conn, id, EntrySortOrder::ReadAt).unwrap();
        assert_eq!(ts.as_deref(), Some("2026-04-03 12:00:00"));
    }

    #[test]
    fn fetch_sort_ts_returns_none_for_missing_id() {
        let conn = setup_db();
        let ts = fetch_sort_ts(&conn, 99999, EntrySortOrder::PublishedAt).unwrap();
        assert_eq!(ts, None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(fetch_sort_ts_)'`
Expected: compile error — `fetch_sort_ts` not found.

- [ ] **Step 3: Implement the helper**

In `src/models/entry.rs`, add after `find_by_id_with_feed` (the function ending around line 165):

```rust
/// Fetch the sort-field value (as the exact TEXT string SQLite stores) for
/// emitting a composite cursor. Returns `None` if the entry doesn't exist.
pub fn fetch_sort_ts(
    conn: &Connection,
    entry_id: i64,
    sort_order: EntrySortOrder,
) -> AppResult<Option<String>> {
    let column_expr = match sort_order {
        EntrySortOrder::ReadAt => "read_at",
        EntrySortOrder::StarredAt => "starred_at",
        EntrySortOrder::PublishedAt => "COALESCE(published_at, created_at)",
    };
    let sql = format!("SELECT {} FROM entry WHERE id = ?1", column_expr);
    conn.query_row(&sql, params![entry_id], |row| row.get::<_, Option<String>>(0))
        .optional()
        .map(|opt| opt.flatten())
        .map_err(AppError::Database)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(fetch_sort_ts_)'`
Expected: all 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry.rs
git commit -m "feat(entry): fetch_sort_ts helper returns SQLite-format sort string

Returns the boundary entry's sort-field value as the exact TEXT bytes
SQLite stores, used to emit a composite cursor whose string compares
byte-for-byte against the indexed column."
```

---

### Task 4: Rewire `ContinuationParams` + `apply_continuation_condition` + caller sites

**Why one task:** Changing the field on `ContinuationParams` breaks compilation at all 3 caller sites simultaneously; splitting into smaller commits would leave the build broken between them.

**Files:**
- Modify: `src/models/entry.rs` (`ContinuationParams` struct ~line 60; `apply_continuation_condition` ~line 750; **two** call sites — `list_ids_by_user` ~line 562 and `list_by_user_with_continuation` ~line 624)
- Modify: `src/handlers/greader/item.rs` (lines 64–71 + 106–110 in `stream_contents`; lines 193–200 + 224–228 in `stream_item_ids`)
- Modify: `src/handlers/pages.rs` (`fetch_entries_for_ssr_with_sort` ~line 415; cursor emission ~line 441–445)

- [ ] **Step 1: Update `ContinuationParams` field**

In `src/models/entry.rs` ~line 60, change:

```rust
pub struct ContinuationParams {
    pub oldest_first: bool,
    pub limit: i64,
    pub continuation_id: Option<i64>,
    pub ot: Option<i64>,
    pub nt: Option<i64>,
    pub sort_order: EntrySortOrder,
}
```

to:

```rust
pub struct ContinuationParams {
    pub oldest_first: bool,
    pub limit: i64,
    pub continuation: Option<ContinuationCursor>,
    pub ot: Option<i64>,
    pub nt: Option<i64>,
    pub sort_order: EntrySortOrder,
}
```

- [ ] **Step 2: Rewrite `apply_continuation_condition` with V2 bounded-OR**

In `src/models/entry.rs` ~line 750, replace the existing `apply_continuation_condition` with:

```rust
/// Apply continuation-based pagination condition.
///
/// Composite cursor uses the V2 bounded-OR form, which the SQLite planner
/// can convert to an indexed range scan even when sort_ts is an expression
/// (`COALESCE(...)`). See PoC at `docs/superpowers/specs/2026-04-26-composite-cursor-pagination-design.md`.
fn apply_continuation_condition(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    continuation: Option<&ContinuationCursor>,
    sort_order: EntrySortOrder,
    oldest_first: bool,
) {
    let Some(cursor) = continuation else {
        return;
    };

    match cursor {
        ContinuationCursor::Composite { sort_ts, id } => {
            let sort_ts_expr = match sort_order {
                EntrySortOrder::ReadAt => "e.read_at",
                EntrySortOrder::StarredAt => "e.starred_at",
                EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at)",
            };
            let (cmp_outer, cmp_inner) = if oldest_first { (">=", ">") } else { ("<=", "<") };
            let ts1 = params_vec.len() + 1;
            let ts2 = params_vec.len() + 2;
            let id_idx = params_vec.len() + 3;
            conditions.push(format!(
                "{expr} {cmp_outer} ?{ts1} AND ({expr} {cmp_inner} ?{ts2} OR e.id {cmp_inner} ?{id_idx})",
                expr = sort_ts_expr,
                cmp_outer = cmp_outer,
                cmp_inner = cmp_inner,
                ts1 = ts1,
                ts2 = ts2,
                id_idx = id_idx,
            ));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(*id));
        }
        ContinuationCursor::LegacyId(id) => {
            let cmp = if oldest_first { ">" } else { "<" };
            let id_idx = params_vec.len() + 1;
            conditions.push(format!("e.id {} ?{}", cmp, id_idx));
            params_vec.push(Box::new(*id));
        }
    }
}
```

- [ ] **Step 3: Update both call sites of `apply_continuation_condition`**

There are TWO call sites in `src/models/entry.rs`:

(a) Inside `list_ids_by_user` (~line 562):

```rust
    apply_continuation_condition(
        &mut conditions,
        &mut params_vec,
        pagination.continuation_id,
        pagination.oldest_first,
    );
```

(b) Inside `list_by_user_with_continuation` (~line 624): identical form.

Change BOTH to:

```rust
    apply_continuation_condition(
        &mut conditions,
        &mut params_vec,
        pagination.continuation.as_ref(),
        pagination.sort_order,
        pagination.oldest_first,
    );
```

Verify with: `rg 'apply_continuation_condition\(' src/models/entry.rs` — expect 3 matches (1 definition + 2 calls).

- [ ] **Step 4: Update `stream_contents` (greader handler)**

In `src/handlers/greader/item.rs` ~line 64, change the `pagination` block:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1, // fetch one extra for continuation
        continuation_id: query.c.as_ref().and_then(|c| c.parse::<i64>().ok()),
        ot: query.ot,
        nt: query.nt,
        sort_order,
    };
```

to:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1, // fetch one extra for continuation
        continuation: query
            .c
            .as_deref()
            .and_then(entry::ContinuationCursor::parse),
        ot: query.ot,
        nt: query.nt,
        sort_order,
    };
```

Then change the cursor emission inside the `read_user` closure (~lines 106–110), from:

```rust
            let continuation = if has_more {
                entries.last().map(|e| e.entry.id.to_string())
            } else {
                None
            };
```

to:

```rust
            let continuation = if has_more {
                entries
                    .last()
                    .and_then(|e| {
                        entry::fetch_sort_ts(conn, e.entry.id, sort_order)
                            .ok()
                            .flatten()
                            .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id))
                    })
            } else {
                None
            };
```

- [ ] **Step 5: Update `stream_item_ids` (greader handler)**

In `src/handlers/greader/item.rs` ~line 193, change the `pagination` block:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1,
        continuation_id: query.c.as_ref().and_then(|c| c.parse::<i64>().ok()),
        ot: query.ot,
        nt: query.nt,
        sort_order,
    };
```

to:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1,
        continuation: query
            .c
            .as_deref()
            .and_then(entry::ContinuationCursor::parse),
        ot: query.ot,
        nt: query.nt,
        sort_order,
    };
```

Then change cursor emission (~lines 224–228), from:

```rust
            let continuation = if has_more {
                entries.last().map(|(id, _)| id.to_string())
            } else {
                None
            };
```

to:

```rust
            let continuation = if has_more {
                entries.last().and_then(|(id, _)| {
                    entry::fetch_sort_ts(conn, *id, sort_order)
                        .ok()
                        .flatten()
                        .map(|ts| entry::ContinuationCursor::encode_composite(&ts, *id))
                })
            } else {
                None
            };
```

Note: `list_ids_by_user` returns `(i64, i64)` tuples (id + microsecond timestamp). The microsecond timestamp is **not** the SQLite TEXT we need — `fetch_sort_ts` queries the column AS TEXT, which is what the predicate compares against.

- [ ] **Step 6: Update `fetch_entries_for_ssr_with_sort`**

In `src/handlers/pages.rs` ~line 415, change the `pagination` block:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: false,
        limit: limit + 1, // fetch one extra to check for continuation
        continuation_id: None,
        ot: None,
        nt: None,
        sort_order,
    };
```

to:

```rust
    let pagination = entry::ContinuationParams {
        oldest_first: false,
        limit: limit + 1, // fetch one extra to check for continuation
        continuation: None,
        ot: None,
        nt: None,
        sort_order,
    };
```

Then update cursor emission (~lines 441–445), from:

```rust
    let continuation = if has_more {
        entries.last().map(|e| e.entry.id.to_string())
    } else {
        None
    };
```

to:

```rust
    let continuation = if has_more {
        entries.last().and_then(|e| {
            entry::fetch_sort_ts(conn, e.entry.id, sort_order)
                .ok()
                .flatten()
                .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id))
        })
    } else {
        None
    };
```

Also update the SSR handler comment block above the emission (the one that says "match that convention or 'Load More' will refetch the boundary entry") to reference the composite cursor convention. Find lines 434–436 and replace with:

```rust
    // Emit a composite `<sort_ts>|<id>` cursor matching the GReader API. The
    // next-page predicate is bounded-OR `(sort_ts < ?ts) OR (sort_ts = ?ts AND id < ?id)`,
    // which keeps Load More correct under non-monotonic id↔sort_ts data.
```

- [ ] **Step 7: Verify build**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo build`
Expected: clean build. If you hit "field `continuation_id` does not exist" anywhere, that's a missed call site — find with `rg 'continuation_id' src tests`.

- [ ] **Step 8: Verify existing tests still pass**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: all pass. Existing tests don't pass cursors with non-monotonic data, so they're unaffected.

In particular `test_ssr_continuation_matches_api_convention_no_duplicates_on_load_more` (in `tests/pages_test.rs`) currently asserts `continuation == last_visible_id.to_string()`. **This will fail** because the cursor is now `<ts>|<id>`. **That's the intended new contract.** Update that assertion as part of this step:

In `tests/pages_test.rs` ~line 1192, change:

```rust
    assert_eq!(
        continuation,
        last_visible_id.to_string(),
        "SSR continuation must equal last visible entry id (API convention); off-by-one would \
         re-render the boundary entry on Load More"
    );
```

to:

```rust
    assert!(
        continuation.ends_with(&format!("|{}", last_visible_id)),
        "SSR continuation must encode the last visible entry id in the new \
         composite '<sort_ts>|<id>' format; got {:?}",
        continuation
    );
```

Re-run `cargo nextest run` and confirm green.

- [ ] **Step 9: Commit**

```bash
git add src/models/entry.rs src/handlers/greader/item.rs src/handlers/pages.rs tests/pages_test.rs
git commit -m "feat(pagination): wire ContinuationCursor through model + handlers (#164)

Rewires ContinuationParams.continuation from Option<i64> to
Option<ContinuationCursor>. apply_continuation_condition now emits the
V2 bounded-OR predicate against COALESCE(published_at, created_at) /
read_at / starred_at depending on sort_order; legacy bare-i64 cursors
still emit e.id < ? for grace.

stream_contents, stream_item_ids, and fetch_entries_for_ssr_with_sort
now parse the c query param via ContinuationCursor::parse and emit the
'<sort_ts>|<id>' composite via fetch_sort_ts + encode_composite.

Updated existing SSR-cursor regression test to assert the new format."
```

---

### Task 5: Unit test — composite cursor walks non-monotonic data without skip

**Files:**
- Test: `src/models/entry.rs` (`tests` module)

- [ ] **Step 1: Write the test**

Add to the `tests` module in `src/models/entry.rs`:

```rust
    #[test]
    fn composite_cursor_walks_non_monotonic_data_without_skip() {
        // Repro for #164: when id↔published_at order diverges (OPML re-import,
        // back-dated feed items), the legacy `e.id < ?` cursor silently skips.
        // The composite cursor must visit every entry.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        // 6 monotonic entries (newer ts ⇒ later id), then 4 "back-dated" entries
        // with NEW ids but OLD timestamps (mimics OPML re-import).
        let monotonic = [
            ("g1", "2026-04-01 10:00:00"),
            ("g2", "2026-04-02 10:00:00"),
            ("g3", "2026-04-03 10:00:00"),
            ("g4", "2026-04-04 10:00:00"),
            ("g5", "2026-04-05 10:00:00"),
            ("g6", "2026-04-06 10:00:00"),
        ];
        let backdated = [
            ("g7-bd", "2026-03-01 10:00:00"),
            ("g8-bd", "2026-03-02 10:00:00"),
            ("g9-bd", "2026-03-03 10:00:00"),
            ("g10-bd", "2026-03-04 10:00:00"),
        ];
        for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![feed_id, guid, ts],
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut seen: Vec<i64> = Vec::new();
        let mut cursor: Option<ContinuationCursor> = None;
        let page_limit: i64 = 3;

        // Walk pages (DESC sort by COALESCE(published_at, created_at)) until empty
        loop {
            let pagination = ContinuationParams {
                oldest_first: false,
                limit: page_limit,
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let page = list_by_user_with_continuation(&conn, user_id, &filter, &pagination)
                .unwrap();
            if page.is_empty() {
                break;
            }
            for ewf in &page {
                assert!(!seen.contains(&ewf.entry.id), "duplicate id {}", ewf.entry.id);
                seen.push(ewf.entry.id);
            }
            let last = page.last().unwrap();
            let sort_ts =
                fetch_sort_ts(&conn, last.entry.id, EntrySortOrder::PublishedAt)
                    .unwrap()
                    .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            });
            // safety: don't loop forever
            if seen.len() > 100 {
                panic!("runaway loop");
            }
        }

        assert_eq!(seen.len(), 10, "must visit all 10 entries; saw {}", seen.len());
    }
```

- [ ] **Step 2: Run the test**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(composite_cursor_walks_non_monotonic_data_without_skip)'`
Expected: PASS (the implementation from Task 4 already supports this; this test is the regression guard).

- [ ] **Step 3: Commit**

```bash
git add src/models/entry.rs
git commit -m "test(entry): composite cursor walks non-monotonic id/ts data fully

Regression guard for #164's silent-skip bug. Seeds 6 monotonic + 4
back-dated entries (high ids, old timestamps), walks pages with the
composite cursor, and asserts all 10 are visited exactly once."
```

---

### Task 6: Unit test — legacy bare-i64 cursor still works (grace path)

**Files:**
- Test: `src/models/entry.rs` (`tests` module)

- [ ] **Step 1: Write the test**

Add to the `tests` module:

```rust
    #[test]
    fn legacy_bare_i64_cursor_still_paginates() {
        // In-flight cursors from pre-#164 deployments must still work for one
        // grace period. Under monotonic data (the common case), the legacy
        // `e.id < ?` predicate is correct.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        for i in 1..=5 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    feed_id,
                    format!("g{}", i),
                    format!("2026-04-0{} 10:00:00", i)
                ],
            )
            .unwrap();
        }

        // Get id of "newest" entry (highest id, latest ts)
        let max_id: i64 = conn
            .query_row("SELECT MAX(id) FROM entry", [], |r| r.get(0))
            .unwrap();

        let pagination = ContinuationParams {
            oldest_first: false,
            limit: 10,
            continuation: Some(ContinuationCursor::LegacyId(max_id)),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let page =
            list_by_user_with_continuation(&conn, user_id, &EntryFilter::default(), &pagination)
                .unwrap();

        // 4 entries below the boundary id
        assert_eq!(page.len(), 4);
        for ewf in &page {
            assert!(ewf.entry.id < max_id);
        }
    }
```

- [ ] **Step 2: Run the test**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(legacy_bare_i64_cursor_still_paginates)'`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/models/entry.rs
git commit -m "test(entry): legacy bare-i64 cursor paginates correctly (grace path)

Pre-#164 cursors still in client URLs/JS state must keep working for a
release. Under monotonic data the legacy e.id < ? predicate is correct."
```

---

### Task 7: Integration test — SSR Load More skip regression

**Files:**
- Test: `tests/pages_test.rs` (new test alongside `test_ssr_continuation_matches_api_convention_no_duplicates_on_load_more`)

- [ ] **Step 1: Write the test**

Append to `tests/pages_test.rs` (after the existing SSR continuation test, around line 1200):

```rust
#[tokio::test]
async fn test_ssr_load_more_does_not_skip_backdated_entries() {
    // Regression for #164: when an entry has a HIGH id but an OLD
    // published_at (e.g. OPML re-import), the legacy `e.id < c` cursor
    // silently skipped it on Load More. With the composite cursor, every
    // entry must be visible across pages 1+2.
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Skip Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/skip.xml", "Skip Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)",
                rusqlite::params![1, 10],
            )
            .unwrap();

            // 10 monotonic newest-first (ids 1..=10, descending hours-ago)
            for i in 1..=10 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        1,
                        format!("mono-{}", i),
                        format!("M{}", i),
                        format!("-{} hours", 10 - i)
                    ],
                )
                .unwrap();
            }
            // 3 back-dated: NEW ids (11, 12, 13) but OLD timestamps (10+ days ago)
            for i in 1..=3 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        1,
                        format!("bd-{}", i),
                        format!("BD{}", i),
                        format!("-{} days", 10 + i)
                    ],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Page 1: SSR
    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    let marker = r#"<script type="application/json" class="ssr-entries">"#;
    let json_start = body.find(marker).expect("ssr-entries script") + marker.len();
    let json_end = body[json_start..].find("</script>").unwrap();
    let json = &body[json_start..json_start + json_end];
    let value: serde_json::Value = serde_json::from_str(json).expect("valid SSR JSON");

    let page1: Vec<i64> = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect();
    assert_eq!(page1.len(), 10, "page 1 should have 10 entries");

    let continuation = value["continuation"].as_str().expect("continuation").to_string();
    assert!(continuation.contains('|'), "continuation must be composite format");

    // Page 2: stream/contents API with the SSR-emitted cursor
    let url = format!(
        "/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=10&c={}",
        urlencoding::encode(&continuation)
    );
    let response = app.server.get(&url).await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let page2: Vec<i64> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            // GReader item ids look like "tag:google.com,2005:reader/item/<hex>"
            let s = e["id"].as_str().unwrap();
            let hex = s.rsplit('/').next().unwrap();
            i64::from_str_radix(hex, 16).unwrap()
        })
        .collect();

    let mut all: Vec<i64> = page1.iter().chain(page2.iter()).copied().collect();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        13,
        "pages 1+2 must include all 13 entries (10 monotonic + 3 back-dated); got {} unique ids",
        all.len()
    );
}
```

If `urlencoding` isn't already a dev-dep, check `Cargo.toml` `[dev-dependencies]`. If absent, replace `urlencoding::encode(&continuation)` with a manual escape: since cursors are `<ts>|<id>`, `|` URL-encodes to `%7C` and the space in `<ts>` to `%20`:

```rust
let encoded = continuation.replace('|', "%7C").replace(' ', "%20");
let url = format!(
    "/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=10&c={}",
    encoded
);
```

- [ ] **Step 2: Run the test**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(test_ssr_load_more_does_not_skip_backdated_entries)'`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/pages_test.rs
git commit -m "test(pages): SSR + API Load More covers back-dated entries (#164)

Repros the silent-skip bug on the SSR→stream/contents handoff. Seeds
10 monotonic + 3 back-dated entries (high ids, old timestamps), walks
pages 1+2, and asserts all 13 are visible across the boundary."
```

---

### Task 8: Integration test — `stream/contents` API skip regression

**Files:**
- Test: `tests/greader_test.rs` (alongside the existing stream-contents tests around line 347)

- [ ] **Step 1: Write the test**

Append to `tests/greader_test.rs` after `test_stream_contents_with_limit` (~line 393):

```rust
#[tokio::test]
async fn test_stream_contents_composite_cursor_no_skip_on_backdated() {
    // Regression for #164: legacy `e.id < c` cursor skipped entries with
    // high ids and old timestamps. Composite cursor must visit them.
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;
    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;

    app.db
        .user(move |conn| {
            // 5 monotonic entries
            for i in 1..=5 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        feed_id,
                        format!("mono-{}", i),
                        format!("M{}", i),
                        format!("-{} hours", 5 - i)
                    ],
                )
                .unwrap();
            }
            // 2 back-dated (new ids, old timestamps)
            for i in 1..=2 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        feed_id,
                        format!("bd-{}", i),
                        format!("BD{}", i),
                        format!("-{} days", 30 + i)
                    ],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    // Page 1: n=5
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=5")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let items1 = body["items"].as_array().unwrap();
    assert_eq!(items1.len(), 5);
    let cursor = body["continuation"].as_str().expect("continuation present").to_string();
    assert!(cursor.contains('|'), "cursor must be composite format, got {:?}", cursor);

    // Page 2: pass cursor
    let encoded = cursor.replace('|', "%7C").replace(' ', "%20");
    let url = format!(
        "/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=5&c={}",
        encoded
    );
    let response = app.server.get(&url).await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let items2 = body["items"].as_array().unwrap();

    // Page 1 holds 5 newest, page 2 holds 2 back-dated → 7 total
    assert_eq!(items1.len() + items2.len(), 7);
}
```

- [ ] **Step 2: Run the test**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -E 'test(test_stream_contents_composite_cursor_no_skip_on_backdated)'`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/greader_test.rs
git commit -m "test(greader): stream/contents composite cursor no-skip on back-dated

Direct API repro for #164. Seeds 5 monotonic + 2 back-dated entries,
walks two pages of n=5, asserts all 7 are visible."
```

---

### Task 9: E2E test — extend `ssr-no-double-render.spec.ts` with skip fixture

**Files:**
- Modify: `e2e/tests/ssr-no-double-render.spec.ts` (append a new `test.describe` block at end of file)

**Notes from existing helpers (already read):**
- `seed.insertEntries(SeedEntry[])` takes `{ feedId, guid, title, link, content, summary?, publishedOffset? }` and inserts via `datetime('now', publishedOffset)`. Returns inserted ids.
- `seed.createCategory(userId, name)` and `seed.createFeed(categoryId, url, title)` create category/feed.
- Existing tests use `page.getByTestId("entry-item")` for entry rows and `page.getByTestId("load-more-btn")` for the Load More button.
- Existing fixture exports: `{ api, seed, page, serverUrl }` from `../fixtures/rdrs.js`.
- The existing Load More tests **intentionally correlate** id↔timestamps (see comment at line 154: "Timestamps for read_at / starred_at also correlate with id so each sort criterion ... yields the same row order — otherwise the API's `e.id < c` continuation could legitimately skip rows."). Our new test must do the OPPOSITE — break that correlation to prove the new cursor doesn't skip.

- [ ] **Step 1: Append the new test block**

At the end of `e2e/tests/ssr-no-double-render.spec.ts`:

```ts
/**
 * Regression: composite cursor (#164) must surface entries with high ids
 * but old timestamps on Load More. The legacy `e.id < c` cursor silently
 * skipped these, hiding back-dated / re-imported entries from the user.
 */
test.describe("Load More surfaces back-dated entries (composite cursor #164)", () => {
  const PER_PAGE = 30; // matches default entries_per_page

  test.beforeAll(async ({ api, seed }) => {
    await api.register("backdateduser", "password123");
    const userId = seed.getUserId("backdateduser");
    const catId = seed.createCategory(userId, "Backdated Cat");
    const feedId = seed.createFeed(
      catId,
      "https://example.com/backdated.xml",
      "Backdated Feed"
    );

    // Page 1 fill: PER_PAGE recent entries (older ids = older timestamps,
    // newest-first ordering means they sort to the top of page 1).
    const recent = Array.from({ length: PER_PAGE }, (_, idx) => {
      const i = idx + 1;
      return {
        feedId,
        guid: `recent-${i}`,
        title: `Recent ${i}`,
        link: `https://example.com/recent/${i}`,
        content: `<p>Recent ${i}</p>`,
        publishedOffset: `-${PER_PAGE - i + 1} hours`,
      };
    });

    // Back-dated: 3 entries with NEW high ids but OLD timestamps. These
    // would be silently skipped by the legacy `e.id < c` cursor on page 2.
    const backdated = [1, 2, 3].map((i) => ({
      feedId,
      guid: `bd-${i}`,
      title: `Backdated ${i}`,
      link: `https://example.com/bd/${i}`,
      content: `<p>Backdated ${i}</p>`,
      publishedOffset: `-${30 + i} days`,
    }));

    // Insert recent first so back-dated rows get higher ids.
    seed.insertEntries(recent);
    seed.insertEntries(backdated);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("backdateduser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  test("Load More on / shows back-dated entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.goto(`${serverUrl}/`);

    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    const beforeCount = await page.getByTestId("entry-item").count();
    expect(beforeCount).toBe(PER_PAGE);

    // None of the back-dated entries should be on page 1 (they're older).
    for (const i of [1, 2, 3]) {
      await expect(page.getByText(`Backdated ${i}`, { exact: true })).not.toBeVisible();
    }

    await page.getByTestId("load-more-btn").click();
    await expect
      .poll(() => page.getByTestId("entry-item").count())
      .toBeGreaterThan(beforeCount);

    // All 3 back-dated entries must appear after Load More.
    for (const i of [1, 2, 3]) {
      await expect(page.getByText(`Backdated ${i}`, { exact: true })).toBeVisible();
    }
  });
});
```

- [ ] **Step 2: Run E2E for this spec only**

```bash
cd /home/nixos/Develop/claude/rdrs/e2e && npx playwright test ssr-no-double-render
```

Expected: all tests pass (existing + new).

- [ ] **Step 3: Commit**

```bash
git add e2e/tests/ssr-no-double-render.spec.ts
git commit -m "test(e2e): Load More surfaces back-dated entries (#164)

End-to-end coverage for the composite cursor: seeds 30 recent + 3
back-dated entries (high ids, 30+ days old), navigates to /, clicks
Load More, asserts all 3 back-dated entries become visible. Without
#164 they would be silently skipped on page 2."
```

---

### Task 10: Final verification + open PR

- [ ] **Step 1: Format**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
```

If `cargo fmt` produced changes (check with `git status`), stage them by explicit filename (per CLAUDE.md: never `git add -A` / `-u` / `.`):

```bash
git status --porcelain | awk '{print $2}'
# stage each modified .rs path explicitly, e.g.:
git add src/models/entry.rs src/handlers/greader/item.rs src/handlers/pages.rs
git commit -m "style: cargo fmt"
```

- [ ] **Step 2: Lint**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo clippy --all-targets -- -D warnings
```

Expected: clean. Fix any lints inline if they appear.

- [ ] **Step 3: Full test suite**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo nextest run
```

Expected: all pass.

- [ ] **Step 4: Full E2E**

```bash
cd /home/nixos/Develop/claude/rdrs/e2e && npx playwright test
```

Expected: all pass (the pre-existing flake on `entry-actions › keyboard s toggles star` mentioned in PR #163's notes is acceptable if it passes solo on retry).

- [ ] **Step 5: Push branch**

```bash
cd /home/nixos/Develop/claude/rdrs && git push -u origin fix/164-composite-cursor-pagination
```

- [ ] **Step 6: Open PR**

```bash
gh pr create --title "fix(pagination): composite (sort_ts, id) cursor — closes #164" --body "$(cat <<'EOF'
Closes #164.

## Summary

- Replaces the bare `e.id < c` continuation predicate with a composite `(sort_ts, id)` cursor so backfill / OPML re-import / out-of-order timestamps no longer silently skip entries on Load More.
- Adds expression index on `COALESCE(published_at, created_at)` so the new bounded-OR predicate keeps an indexed range scan for the `PublishedAt` path.
- Cursor is opaque to clients — wire format `<sort_ts>|<id>`. Pre-#164 bare-`i64` cursors are accepted as a one-time grace path for in-flight cursors in client state.

## Why bounded-OR + expression index

PoC results (200K rows, 50/page, mid-table cursor):

| Path | Today | After #164 |
|---|---|---|
| `ReadAt` (plain column) | 0.47 ms | **0.013 ms** |
| `PublishedAt` (`COALESCE`, no expr index) | 5.3 ms | 12.7 ms ⚠️ |
| `PublishedAt` (`COALESCE`, with expr index) | 2.4 ms | **0.017 ms** |

`SCAN entry USING COVERING INDEX idx_entry_sort_ts (<expr><?)` confirmed via `EXPLAIN QUERY PLAN`. Composite `(ts,id)` index intentionally NOT added — single-col + bounded-OR is sufficient.

## Tests

- `src/models/entry.rs`: cursor parse/encode unit tests, fetch_sort_ts helper tests, composite-cursor non-monotonic walk regression, legacy bare-i64 grace-path test.
- `tests/pages_test.rs`: SSR Load More handoff to API with back-dated entries — asserts no skip across page boundary. Existing SSR-cursor convention test updated for new composite format.
- `tests/greader_test.rs`: stream/contents direct API repro on back-dated fixture.
- `e2e/tests/ssr-no-double-render.spec.ts`: adds Load More surfacing of back-dated entries.

## Test plan

- [x] `cargo nextest run` — all pass
- [x] `cargo clippy --all-targets -- -D warnings` — clean
- [x] `cargo fmt --check` — clean
- [x] `cd e2e && npx playwright test` — pass

## Out of scope

- Removing the `LegacyId` grace path (follow-up release after we're confident no in-flight cursors remain).
- Backfilling `published_at` to drop `COALESCE` (orthogonal cleanup).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 7: Verify CI**

```bash
gh pr checks
```

Expected: all checks pass.

---

## Self-review notes (for the implementer)

If a step's command fails:

- **Compile error after Task 4**: search `rg 'continuation_id' src tests` for missed call sites.
- **Cursor format mismatch in Task 7/8**: print the cursor string in a `dbg!()` and verify it has `|` plus a parseable id.
- **`fetch_sort_ts` returns `None` unexpectedly**: the boundary entry was deleted between fetch and emission (test data race) — re-seed.
- **E2E selector mismatch in Task 9**: read `e2e/tests/ssr-no-double-render.spec.ts` and `e2e/helpers/seed.ts` carefully and match the project's existing seed signature instead of inventing a new one.

If a test fails *correctly* after Task 4 because behavior changed (e.g. SSR cursor format), update the assertion as documented in Task 4 Step 8.
