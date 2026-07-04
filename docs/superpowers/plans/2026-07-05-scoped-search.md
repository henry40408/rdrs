# Scoped Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user search entries within a single category/feed by keyword, then mark the matching entries as read or read through them — and fix global `/search` to match stored plain text instead of raw HTML.

**Architecture:** Add a stored `entry.content_text` column (HTML stripped to plain text) and search `LIKE` against it. Reuse the existing `EntryFilter.search` field + the category/feed entries pages, adding a server-side search box (debounced auto-submit) and a server-side "mark matching as read" action.

**Tech Stack:** Rust, Axum, Askama templates, rusqlite/SQLite, vanilla ES modules, Playwright BDD (E2E).

## Global Constraints

- Format gate: `cargo fmt --all -- --check` — run `cargo fmt` before committing.
- Lint gate: `cargo clippy --all-targets -- -D warnings` — warnings fail the build.
- Tests run with `cargo nextest run` (never `cargo test`). Use `RDRS_FAST_HASH=1` for local runs.
- Commits MUST be GPG-signed (`git commit -S`). Never `git add -A`/`.`; stage files by name.
- After editing anything under `static/`, `templates/`, or Rust source, run `cargo build` before E2E/screenshots (E2E skips the build if a binary exists).
- The four README screenshots are unaffected by this work (search box only on category/feed pages) — do NOT regenerate them.
- SSR-first: no bundlers/transpilers; vanilla ES modules only.
- All GitHub-facing content (PR title/body) in English.

---

### Task 1: Move `strip_to_plain_text` into a shared util

The plain-text stripper is currently a private fn in a handler module; the DB/migration and model layers need it too, and importing from `handlers/` there is a layering smell. Move it to `src/utils/text.rs`.

**Files:**
- Create: `src/utils/text.rs`
- Modify: `src/utils/mod.rs` (add `pub mod text;`)
- Modify: `src/handlers/pages/search_text.rs` (delete the local fn, import the shared one)

**Interfaces:**
- Produces: `crate::utils::text::strip_to_plain_text(raw: &str) -> String`

- [ ] **Step 1: Create `src/utils/text.rs` with the moved fn + a test**

Copy the body verbatim from `src/handlers/pages/search_text.rs` (lines 4–82), making it `pub`:

```rust
//! Plain-text extraction from HTML. Pure string function — no request state.
//! Strips tags (including `<script>`/`<style>` bodies and comments) and
//! collapses whitespace to single spaces.

/// Strip HTML tags and collapse whitespace into a single line of plain text.
pub fn strip_to_plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut last_space = true;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end_tag) = skip_until {
            if let Some(pos) = raw[i..].to_ascii_lowercase().find(end_tag) {
                i += pos + end_tag.len();
                skip_until = None;
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                continue;
            } else {
                break;
            }
        }
        let ch = bytes[i] as char;
        match ch {
            '<' => {
                let lower = raw[i..].to_ascii_lowercase();
                if lower.starts_with("<script") {
                    skip_until = Some("</script>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<style") {
                    skip_until = Some("</style>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<!--") {
                    if let Some(pos) = raw[i + 4..].find("-->") {
                        i += 4 + pos + 3;
                        if !last_space {
                            out.push(' ');
                            last_space = true;
                        }
                        continue;
                    } else {
                        break;
                    }
                }
                in_tag = true;
                i += 1;
            }
            '>' if in_tag => {
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ if in_tag => {
                i += 1;
            }
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ => {
                let ch_len = raw[i..].chars().next().map_or(1, |c| c.len_utf8());
                out.push_str(&raw[i..i + ch_len]);
                last_space = false;
                i += ch_len;
            }
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_keeps_text_across_tags() {
        // A term split across inline tags must be contiguous in plain text.
        assert_eq!(strip_to_plain_text("超<b>少女</b>與機器人"), "超 少女 與機器人");
    }

    #[test]
    fn drops_script_and_attribute_text() {
        assert_eq!(
            strip_to_plain_text(r#"<a href="https://x/超少女">hi</a><script>var superheroine=1</script>"#),
            "hi",
        );
    }
}
```

Note: the stripper inserts a space where a tag was, so `超<b>少女</b>` becomes `超 少女` (not `超少女`). This is acceptable for `LIKE %超少女%`? **No** — a space would break the substring match. See Step 4 for the fix decision.

- [ ] **Step 2: Register the module and switch the handler to the shared fn**

In `src/utils/mod.rs` add:

```rust
pub mod text;
```

In `src/handlers/pages/search_text.rs`, delete the local `strip_to_plain_text` fn (lines 4–82) and add at the top:

```rust
use crate::utils::text::strip_to_plain_text;
```

- [ ] **Step 3: Run tests — expect the tag-space test to FAIL**

Run: `cargo nextest run -p rdrs strips_tags_and_keeps_text_across_tags drops_script`
Expected: `strips_tags_and_keeps_text_across_tags` FAILS (asserts `"超 少女 與機器人"` — confirming the current behavior inserts a space; the second test PASSES).

- [ ] **Step 4: Decide tag-boundary joining for search correctness**

For search we need `超<b>少女</b>` to match `超少女`. Add a second, no-space variant used by search/indexing only, so display snippets keep readable spacing:

Add to `src/utils/text.rs`:

```rust
/// Like [`strip_to_plain_text`] but inserts **no** separator at tag
/// boundaries, so a term split across inline tags (`超<b>少女</b>`) stays
/// contiguous. Used to build the searchable `entry.content_text`.
pub fn strip_to_search_text(raw: &str) -> String {
    strip_impl(raw, false)
}
```

Refactor the existing fn to delegate: rename the body to
`fn strip_impl(raw: &str, tag_gap: bool) -> String` and replace each
`out.push(' '); last_space = true;` **that fires on a tag close/open boundary**
(the `'>' if in_tag`, the `skip_until` close, and the comment-close arms — NOT
the whitespace arm) with:

```rust
if tag_gap && !last_space {
    out.push(' ');
    last_space = true;
}
```

Keep `pub fn strip_to_plain_text(raw) -> String { strip_impl(raw, true) }`.
Add a test:

```rust
#[test]
fn search_text_joins_across_tags() {
    assert_eq!(strip_to_search_text("超<b>少女</b>與機器人"), "超少女與機器人");
}
```

- [ ] **Step 5: Run tests — expect PASS**

Run: `cargo nextest run -p rdrs search_text_joins_across_tags drops_script`
Expected: PASS. Then `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add src/utils/text.rs src/utils/mod.rs src/handlers/pages/search_text.rs
git commit -S -m "refactor: extract strip_to_plain_text to utils::text, add search variant"
```

---

### Task 2: Migration v10 — add and backfill `entry.content_text`

**Files:**
- Modify: `src/db/schema.rs` (new `if version < 10` block, bump `LATEST_VERSION`, update version test assertions)

**Interfaces:**
- Consumes: `crate::utils::text::strip_to_search_text` (Task 1)
- Produces: `entry.content_text TEXT` column, populated for all existing rows with non-null `content`.

**Important:** Do NOT add `content_text` to the `CREATE TABLE entry` batch (lines 62–77). Migration-added columns (like `feed.bucket`, `feed.custom_referrer`) live only in their `ALTER`; a fresh DB (user_version 0) runs every `if version < N` block, so a column present in both CREATE and an un-swallowed ALTER would error.

- [ ] **Step 1: Write the failing test**

Add to `src/db/schema.rs` tests module:

```rust
#[test]
fn test_entry_has_content_text_column() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('entry')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(cols.contains(&"content_text".to_string()));
}

#[test]
fn test_v10_backfills_content_text() {
    // Simulate a pre-v10 DB: create schema, force user_version back to 9,
    // drop content_text, insert a row with HTML content, re-run init_db.
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO user (id, username, password_hash) VALUES (1, 'u', 'x');
         INSERT INTO category (id, user_id, name) VALUES (1, 1, 'c');
         INSERT INTO feed (id, category_id, url) VALUES (1, 1, 'http://x');
         INSERT INTO entry (id, feed_id, guid, content) VALUES (1, 1, 'g', '超<b>少女</b>');
         UPDATE entry SET content_text = NULL WHERE id = 1;",
    ).unwrap();
    conn.pragma_update(None, "user_version", 9i64).unwrap();
    init_db(&conn).unwrap();
    let text: Option<String> = conn
        .query_row("SELECT content_text FROM entry WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(text.as_deref(), Some("超少女"));
}
```

(If the `user` table columns differ, adjust the INSERT to satisfy NOT NULL constraints — inspect the CREATE at the top of `schema.rs`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p rdrs test_entry_has_content_text_column test_v10_backfills_content_text`
Expected: FAIL — column `content_text` does not exist.

- [ ] **Step 3: Add the migration block**

At the top of `src/db/schema.rs`, add the import:

```rust
use crate::utils::text::strip_to_search_text;
```

Insert **before** the `const LATEST_VERSION` line (currently line 283):

```rust
    if version < 10 {
        conn.execute("ALTER TABLE entry ADD COLUMN content_text TEXT", [])?;
        // Backfill plain-text search content in batches so a large entry
        // table doesn't build one giant transaction. Rows with NULL content
        // stay NULL (nothing to search). strip_to_search_text joins across
        // tags so terms split by inline markup remain matchable.
        loop {
            let batch: Vec<(i64, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT id, content FROM entry \
                     WHERE content_text IS NULL AND content IS NOT NULL LIMIT 500",
                )?;
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .filter_map(Result::ok)
                    .collect()
            };
            if batch.is_empty() {
                break;
            }
            let tx = conn.unchecked_transaction()?;
            {
                let mut upd =
                    tx.prepare_cached("UPDATE entry SET content_text = ?1 WHERE id = ?2")?;
                for (id, content) in &batch {
                    upd.execute(rusqlite::params![strip_to_search_text(content), id])?;
                }
            }
            tx.commit()?;
        }
    }
```

Then bump the constant:

```rust
    const LATEST_VERSION: i64 = 10;
```

- [ ] **Step 4: Update the two version assertions**

In the same file's tests, change `assert_eq!(version, 9);` → `assert_eq!(version, 10);` in both `test_init_db_idempotent` and `test_init_db_sets_user_version`.

- [ ] **Step 5: Run tests — expect PASS**

Run: `cargo nextest run -p rdrs schema`
Expected: PASS. Then `cargo fmt`.

- [ ] **Step 6: Commit**

```bash
git add src/db/schema.rs
git commit -S -m "feat(db): add entry.content_text column (migration v10) with backfill"
```

---

### Task 3: Populate `content_text` on upsert

Compute `content_text` inside `upsert_entry_id` (the single sync/upsert chokepoint) so every insert/update keeps it in sync — no new parameter, no caller churn.

**Files:**
- Modify: `src/models/entry/mod.rs` (`upsert_entry_id` UPDATE + INSERT SQL)

**Interfaces:**
- Consumes: `crate::utils::text::strip_to_search_text` (Task 1)

- [ ] **Step 1: Write the failing test**

Add to the tests module in `src/models/entry/mod.rs` (reuse existing test helpers for a conn + feed; mirror a nearby upsert test):

```rust
#[test]
fn upsert_populates_content_text_stripped() {
    let conn = test_conn();               // existing helper in this module's tests
    let feed_id = seed_feed(&conn);       // existing helper
    let out = upsert_entry_id(
        &conn, feed_id, "g1", Some("t"), None,
        Some("超<b>少女</b>與機器人"), None, None, None,
    ).unwrap();
    let id = match out { UpsertOutcome::Inserted(id) => id, _ => panic!() };
    let ct: Option<String> = conn
        .query_row("SELECT content_text FROM entry WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert_eq!(ct.as_deref(), Some("超少女與機器人"));
}
```

(If `test_conn`/`seed_feed` helpers are named differently, use the existing ones — grep the tests module for how other `upsert_entry_id` tests obtain a conn + feed_id.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p rdrs upsert_populates_content_text_stripped`
Expected: FAIL — `content_text` is NULL (not yet written).

- [ ] **Step 3: Implement — compute and store in both SQL paths**

At the top of `src/models/entry/mod.rs` add (if not already imported):

```rust
use crate::utils::text::strip_to_search_text;
```

In `upsert_entry_id`, after `let published_at_str = ...;` (line 504) add:

```rust
    let content_text = content.map(strip_to_search_text);
```

Change the UPDATE statement (lines 517–525) to:

```rust
        conn.prepare_cached(
            r#"
            UPDATE entry
            SET title = ?1, link = ?2, content = ?3, summary = ?4, author = ?5,
                content_text = ?7, updated_at = datetime('now')
            WHERE id = ?6
            "#,
        )?
        .execute(params![title, link, content, summary, author, id, content_text])?;
```

Change the INSERT statement (lines 534–553) to:

```rust
    let inserted = conn
        .prepare_cached(
            r#"
            INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at, content_text)
            SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
            WHERE NOT EXISTS (
                SELECT 1 FROM entry_tombstone WHERE feed_id = ?1 AND guid = ?2
            )
            "#,
        )?
        .execute(params![
            feed_id, guid, title, link, content, summary, author, published_at_str, content_text
        ])?;
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run -p rdrs upsert_populates_content_text_stripped`
Expected: PASS. Then `cargo nextest run -p rdrs entry` to confirm no regressions; `cargo fmt`; `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/mod.rs
git commit -S -m "feat(entry): store content_text on upsert"
```

---

### Task 4: Point the search predicate at `content_text`

**Files:**
- Modify: `src/models/entry/filters.rs:81-89`

- [ ] **Step 1: Write the failing test**

Add to the tests module in `src/models/entry/filters.rs` (or `mod.rs` tests, wherever filter behavior is tested — grep for existing `search:` filter tests to match the harness):

```rust
#[test]
fn search_matches_plain_text_across_tags_not_attributes() {
    let conn = test_conn();
    let feed_id = seed_feed(&conn);
    // (a) term split across inline tags — must match via content_text.
    upsert_entry_id(&conn, feed_id, "a", Some("x"), None, Some("超<b>少女</b>登場"), None, None, None).unwrap();
    // (b) term only inside an href attribute — must NOT match.
    upsert_entry_id(&conn, feed_id, "b", Some("y"), None, Some(r#"<a href="/超少女">z</a>"#), None, None, None).unwrap();
    let filter = EntryFilter { search: Some("超少女".to_string()), ..Default::default() };
    let rows = list_by_user(&conn, /* user_id */ 1, &filter, EntrySortOrder::PublishedAt, 50, 0).unwrap();
    let guids: Vec<_> = rows.iter().map(|r| r.entry.guid.clone()).collect();
    assert!(guids.contains(&"a".to_string()), "tag-split term should match");
    assert!(!guids.contains(&"b".to_string()), "attribute-only term should not match");
}
```

(Note: for `strip_to_search_text("<a href=\"/超少女\">z</a>")` the attribute text is dropped, so `content_text = "z"` — hence (b) does not match. `test_conn`/`seed_feed` must produce user_id 1; reuse the module's existing seeding helpers.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p rdrs search_matches_plain_text_across_tags_not_attributes`
Expected: FAIL — current predicate matches raw `content`, so (a) matches only if not tag-split and (b) wrongly matches the href.

- [ ] **Step 3: Change the predicate**

In `src/models/entry/filters.rs`, change the search clause (lines 84–87) from:

```rust
        conditions.push(format!(
            "(e.title LIKE ?{} COLLATE NOCASE OR e.content LIKE ?{} COLLATE NOCASE)",
            param_idx, param_idx
        ));
```

to:

```rust
        conditions.push(format!(
            "(e.title LIKE ?{} COLLATE NOCASE OR e.content_text LIKE ?{} COLLATE NOCASE)",
            param_idx, param_idx
        ));
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run -p rdrs search_matches_plain_text_across_tags_not_attributes`
Expected: PASS. Then `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/filters.rs
git commit -S -m "feat(search): match content_text plain text instead of raw HTML"
```

---

### Task 5: Simplify `search_page` (drop the phantom-filter scan)

Now that SQL returns only genuine plain-text matches, the OFFSET-paged re-filter loop is dead weight. Replace it with one query.

**Files:**
- Modify: `src/handlers/pages/mod.rs:1676-1760` (`search_page`)

**Interfaces:**
- Consumes: `entry::list_by_user`, `SearchResultView`, `build_snippet`, `highlight_html`, `format_relative_time` (all already in scope).

- [ ] **Step 1: Replace the loop body**

Replace the whole `let results = if q.is_empty() { Vec::new() } else { … }` block (lines 1676–1760) with:

```rust
    let results = if q.is_empty() {
        Vec::new()
    } else {
        let q_for_filter = q.clone();
        state
            .db
            .read_user(move |conn| {
                let filter = entry::EntryFilter {
                    search: Some(q_for_filter.clone()),
                    ..Default::default()
                };
                // SQL now matches stored plain text (content_text), so every
                // returned row is a real visible match — no phantom re-filter.
                const LIMIT: i64 = 50;
                let rows = entry::list_by_user(
                    conn,
                    user_id,
                    &filter,
                    entry::EntrySortOrder::PublishedAt,
                    LIMIT,
                    0,
                )?;
                let out: Vec<SearchResultView> = rows
                    .into_iter()
                    .map(|e| {
                        let title = e
                            .entry
                            .title
                            .clone()
                            .unwrap_or_else(|| "(no title)".to_string());
                        let snippet = build_snippet(
                            e.entry.content.as_deref().or(e.entry.summary.as_deref()),
                            &q_for_filter,
                            200,
                        );
                        let (published_relative, published_at_iso) =
                            format_relative_time(e.entry.published_at);
                        SearchResultView {
                            entry_id: e.entry.id,
                            title_html: highlight_html(&title, &q_for_filter),
                            feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                            published_relative,
                            published_at_iso,
                            snippet_html: highlight_html(&snippet, &q_for_filter),
                        }
                    })
                    .collect();
                Ok::<_, AppError>(out)
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    };
```

- [ ] **Step 2: Build and run existing search tests**

Run: `cargo build && cargo nextest run -p rdrs search`
Expected: PASS (existing search_page/handler tests still green; the result set is now exact). Then `cargo fmt`; `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 3: Manual smoke via the verify skill (optional but recommended)**

Drive `/search?q=<term>` against a seeded DB and confirm results render. If any test asserted the old 1000-scan behavior, update it to the exact-match expectation.

- [ ] **Step 4: Commit**

```bash
git add src/handlers/pages/mod.rs
git commit -S -m "refactor(search): drop phantom-match scan, query content_text directly"
```

---

### Task 6: Thread `q` into the category/feed entries handlers

**Files:**
- Modify: `src/handlers/pages/mod.rs` — `EntriesQuery` (305), `EntriesFragmentTemplate` (360), `category_entries_page` (1513), `feed_entries_page` (1779)

**Interfaces:**
- Produces: `EntriesQuery.q: Option<String>`; `EntriesFragmentTemplate.q: Option<String>`; both handlers set `filter.search` and compute a `matching_count` when `q` is present.

- [ ] **Step 1: Add the query field**

In `EntriesQuery` (lines 305–321) add:

```rust
    /// Scoped-search keyword (category/feed pages only). Empty/whitespace ⇒ no filter.
    pub q: Option<String>,
```

- [ ] **Step 2: Add `q` to the fragment template struct + template**

In `EntriesFragmentTemplate` (lines 360–368) add:

```rust
    /// Forwarded into the Load-More form so paged fetches keep the search filter.
    pub q: Option<String>,
```

In `templates/_entries_fragment.html`, inside the `#load-more` form (after the `status` hidden input line) add:

```html
    {% if let Some(qq) = q.as_ref() %}<input type="hidden" name="q" value="{{ qq }}">{% endif %}
```

- [ ] **Step 3: Set the filter + fragment field in `category_entries_page`**

In `category_entries_page`, after building `filter` (line 1554) add:

```rust
    let search = query.q.clone().filter(|s| !s.trim().is_empty());
    filter.search = search.clone();
```

In the fragment branch (lines 1584–1591) add `q: search.clone(),` to the `EntriesFragmentTemplate { … }` literal.

Compute the matching count for the button (only when searching), just before the `template` is built:

```rust
    let matching_count = if search.is_some() {
        let cf = filter.clone();
        state
            .db
            .read_user(move |conn| entry::count_by_user(conn, user_id, &cf))
            .await
            .ok()
            .and_then(|r| r.ok())
    } else {
        None
    };
```

(`EntryFilter` derives `Clone` — confirm; it's a plain struct of `Option`/`bool`. If not, add `#[derive(Clone)]`.)

- [ ] **Step 4: Same wiring in `feed_entries_page`**

Apply the identical three edits in `feed_entries_page` (filter build ~1829, fragment branch, and `matching_count`).

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compile error — `EntriesLayoutContext` has no `search`/`search_action`/`matching_count` fields yet, and the two template literals for `EntriesLayoutContext` are incomplete. That's fixed in Task 7; if you prefer a green checkpoint, do Task 7 before building. Otherwise proceed to Task 7 and build once at its end.

- [ ] **Step 6: Commit (fold with Task 7 if not independently green)**

```bash
git add src/handlers/pages/mod.rs templates/_entries_fragment.html
git commit -S -m "feat(entries): accept ?q= scoped-search filter on category/feed pages"
```

---

### Task 7: Render the scoped-search box + mark-matching button

**Files:**
- Modify: `src/handlers/pages/mod.rs` — `EntriesLayoutContext` (162–215) + the two handlers' `EntriesLayoutContext { … }` literals
- Modify: `templates/_entries_layout.html` (filter-bar)

**Interfaces:**
- Consumes: handler-provided `search: Option<String>`, `search_action: Option<String>`, `matching_count: Option<i64>`.

- [ ] **Step 1: Add fields to `EntriesLayoutContext`**

In the struct (lines 162–215) add:

```rust
    /// Current scoped-search keyword (prefills the box + hidden inputs). `None`
    /// on pages without scoped search.
    pub search: Option<String>,
    /// Form action for the scoped-search box. `Some` ⇒ render the box (category/
    /// feed pages only). `None` ⇒ no search box.
    pub search_action: Option<String>,
    /// Count of entries matching the active search, for the "Mark N matching as
    /// Read" button label. `None` when not searching.
    pub matching_count: Option<i64>,
```

- [ ] **Step 2: Populate them in both handlers**

In `category_entries_page`'s `EntriesLayoutContext { … }` (around 1636–1650) add:

```rust
            search: search.clone(),
            search_action: Some(format!("/categories/{}/entries", id)),
            matching_count,
```

In `feed_entries_page`'s literal add the same, with `search_action: Some(format!("/feeds/{}/entries", id)),`.

Every OTHER `EntriesLayoutContext { … }` literal in the file (unread/all/read/starred/summarized pages) must add the three fields set to `None`:

```rust
            search: None,
            search_action: None,
            matching_count: None,
```

(Grep `EntriesLayoutContext {` to find all construction sites — there are ~7.)

- [ ] **Step 3: Render the search box + mark-matching button in the template**

In `templates/_entries_layout.html`, inside the `filter-bar` div (after the `mark_as_read_scope` block, before the closing `</div>` at line 46) add:

```html
                            {% if let Some(action) = entries_layout.search_action.as_ref() %}
                            <form class="form-group form-group-inline entries-search" method="get" action="{{ action }}" data-swap="[data-entries-list]" data-entries-search>
                                {% if let Some(s) = entries_layout.status_filter.as_ref() %}<input type="hidden" name="status" value="{{ s }}">{% endif %}
                                <input type="search" name="q" class="search-input" placeholder="Search in this view…" autocomplete="off" data-testid="scoped-search-input"{% if let Some(qq) = entries_layout.search.as_ref() %} value="{{ qq }}"{% endif %}>
                            </form>
                            {% if let Some(n) = entries_layout.matching_count %}
                            <form class="form-group form-group-inline" method="post" action="{{ action }}/mark-read" data-testid="mark-matching-form">
                                {% if let Some(qq) = entries_layout.search.as_ref() %}<input type="hidden" name="q" value="{{ qq }}">{% endif %}
                                <button type="submit" class="btn-secondary btn-sm" data-testid="mark-matching-btn">Mark {{ n }} matching as Read</button>
                            </form>
                            {% endif %}
                            {% endif %}
```

- [ ] **Step 4: Add the hidden `q` to the main Load-More form**

In `templates/_entries_layout.html`, inside the `#load-more` form (lines 74–79), after the `status` hidden-input line add:

```html
                                    {% if let Some(qq) = entries_layout.search.as_ref() %}<input type="hidden" name="q" value="{{ qq }}">{% endif %}
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: PASS. Then `cargo fmt`; `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```bash
git add src/handlers/pages/mod.rs templates/_entries_layout.html
git commit -S -m "feat(entries): render scoped-search box and mark-matching button"
```

---

### Task 8: Debounced auto-submit for the search box

**Files:**
- Modify: `static/js/utils.js` (add `debounce`)
- Modify: `static/js/app.js` (add `installEntriesSearch`, call it + re-init on swap)

**Interfaces:**
- Consumes: the existing `installSwap` GET-form → query-string swap path (submits `[data-entries-list]`).

- [ ] **Step 1: Add a `debounce` helper**

In `static/js/utils.js` append and export:

```js
export function debounce(fn, ms) {
    let t;
    return function (...args) {
        clearTimeout(t);
        t = setTimeout(() => fn.apply(this, args), ms);
    };
}
```

- [ ] **Step 2: Add the search installer in `app.js`**

Near the other `install*` helpers (e.g. after `installStatusFilterSelect`), add:

```js
function installEntriesSearch() {
    const form = document.querySelector('form[data-entries-search]');
    if (!form || form.dataset.searchBound) return;
    form.dataset.searchBound = '1';
    const input = form.querySelector('input[name="q"]');
    if (!input) return;
    const submit = debounce(() => form.requestSubmit(), 250);
    input.addEventListener('input', submit);
}
installEntriesSearch();
document.addEventListener('rdrs:swap-complete', installEntriesSearch);
```

Ensure `debounce` is imported at the top of `app.js`:

```js
import { debounce } from './utils.js';
```

(If `app.js` already imports from `./utils.js`, add `debounce` to the existing import list. Match the existing import style — grep the file head.)

The search `<form>` lives in `.list-pane-header`, OUTSIDE the swapped `[data-entries-list]` container, so it is not replaced by the swap and keeps focus/caret while typing. `form[data-swap]` in `installSwap` handles the actual fetch + list replacement; this installer only triggers the debounced submit.

- [ ] **Step 3: Build and eyeball**

Run: `cargo build`
Expected: PASS. Rebuild is required so the embedded JS is fresh for E2E.

- [ ] **Step 4: Commit**

```bash
git add static/js/utils.js static/js/app.js
git commit -S -m "feat(ui): debounced auto-submit for scoped search"
```

---

### Task 9: `mark_read_by_filter` model function

**Files:**
- Modify: `src/models/entry/mod.rs` (new fn)

**Interfaces:**
- Consumes: `apply_filter_conditions` (via `use filters::...` already present in this module).
- Produces: `pub fn mark_read_by_filter(conn: &Connection, user_id: i64, filter: &EntryFilter) -> AppResult<i64>` — marks matching, owned, currently-unread entries as read; returns rows affected.

- [ ] **Step 1: Write the failing test**

Add to the tests module in `src/models/entry/mod.rs`:

```rust
#[test]
fn mark_read_by_filter_marks_only_matching_unread_owned() {
    let conn = test_conn();
    let feed_id = seed_feed(&conn);             // belongs to user 1
    let m = upsert_entry_id(&conn, feed_id, "m", Some("超少女登場"), None, None, None, None, None).unwrap();
    let n = upsert_entry_id(&conn, feed_id, "n", Some("其他新聞"), None, None, None, None, None).unwrap();
    let m_id = match m { UpsertOutcome::Inserted(id) => id, _ => panic!() };
    let n_id = match n { UpsertOutcome::Inserted(id) => id, _ => panic!() };

    let filter = EntryFilter {
        feed_id: Some(feed_id),
        search: Some("超少女".to_string()),
        ..Default::default()
    };
    let affected = mark_read_by_filter(&conn, 1, &filter).unwrap();
    assert_eq!(affected, 1);

    let m_read: Option<String> = conn.query_row("SELECT read_at FROM entry WHERE id=?1", [m_id], |r| r.get(0)).unwrap();
    let n_read: Option<String> = conn.query_row("SELECT read_at FROM entry WHERE id=?1", [n_id], |r| r.get(0)).unwrap();
    assert!(m_read.is_some(), "matching entry marked read");
    assert!(n_read.is_none(), "non-matching entry untouched");

    // Idempotent: already-read matching row isn't recounted.
    assert_eq!(mark_read_by_filter(&conn, 1, &filter).unwrap(), 0);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p rdrs mark_read_by_filter_marks_only_matching_unread_owned`
Expected: FAIL — `mark_read_by_filter` not defined.

- [ ] **Step 3: Implement the function**

Add near `mark_all_read_by_user` in `src/models/entry/mod.rs`:

```rust
/// Mark every entry matching `filter` (and owned by `user_id`, and currently
/// unread) as read. Reuses the shared filter builder so scoped search + status
/// combine exactly as they do in the list query. Returns rows affected.
pub fn mark_read_by_filter(
    conn: &Connection,
    user_id: i64,
    filter: &EntryFilter,
) -> AppResult<i64> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
    apply_filter_conditions(&mut conditions, &mut params_vec, filter);
    let where_clause = conditions.join(" AND ");

    let sql = format!(
        r#"
        UPDATE entry
        SET read_at = datetime('now'), updated_at = datetime('now')
        WHERE read_at IS NULL AND id IN (
            SELECT e.id FROM entry e
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE {where_clause}
        )
        "#
    );

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = conn.execute(&sql, params_refs.as_slice())?;
    Ok(rows as i64)
}
```

Note: the subquery supplies the `e.`/`c.` aliases that `apply_filter_conditions` emits, and seeds `?1 = user_id` exactly as `list_by_user` does, so the builder's `?1` assumption (used by its `has_summary` subqueries) stays valid.

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run -p rdrs mark_read_by_filter`
Expected: PASS. Then `cargo fmt`; `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/mod.rs
git commit -S -m "feat(entry): mark_read_by_filter to bulk-read scoped-search matches"
```

---

### Task 10: Mark-matching-as-read routes + handler

**Files:**
- Modify: `src/handlers/pages/mod.rs` (new handler + form struct)
- Modify: `src/lib.rs` (register two POST routes)

**Interfaces:**
- Consumes: `mark_read_by_filter` (Task 9), `FlashRedirect::success` (`crate::middleware::flash::FlashRedirect`), `state.events.emit_sidebar`, `state.sidebar_cache.bust`.
- Produces: `POST /categories/{id}/entries/mark-read`, `POST /feeds/{id}/entries/mark-read`.

- [ ] **Step 1: Write the handler + form struct**

In `src/handlers/pages/mod.rs` add near the category/feed handlers:

```rust
#[derive(serde::Deserialize)]
pub struct MarkReadForm {
    pub q: Option<String>,
}

/// `POST /categories/{id}/entries/mark-read` — mark all entries in the category
/// matching the scoped-search `q` as read, then redirect back to the list
/// (keeping `?q=`).
pub async fn category_mark_read_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MarkReadForm>,
) -> Response {
    mark_read_scoped(&state, auth_user.user.id, Some(id), None, form.q, &format!("/categories/{}/entries", id)).await
}

/// `POST /feeds/{id}/entries/mark-read` — same, scoped to a feed.
pub async fn feed_mark_read_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MarkReadForm>,
) -> Response {
    mark_read_scoped(&state, auth_user.user.id, None, Some(id), form.q, &format!("/feeds/{}/entries", id)).await
}

async fn mark_read_scoped(
    state: &AppState,
    user_id: i64,
    category_id: Option<i64>,
    feed_id: Option<i64>,
    q: Option<String>,
    base_path: &str,
) -> Response {
    let search = q.as_ref().and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    });
    let filter = entry::EntryFilter {
        category_id,
        feed_id,
        search: search.clone(),
        ..Default::default()
    };
    let f = filter.clone();
    // `db.user` runs on the priority (write) connection and returns the
    // closure's value wrapped in `Result<_, DbError>`; the closure itself
    // returns `AppResult<i64>`, hence the double unwrap. Mirrors the GReader
    // mark-all handler (`state.db.user(...).await??`), but here we keep the
    // count for the flash instead of `?`-propagating.
    let affected = match state
        .db
        .user(move |conn| entry::mark_read_by_filter(conn, user_id, &f))
        .await
    {
        Ok(Ok(n)) => n,
        Ok(Err(_)) | Err(_) => 0,
    };
    if affected > 0 {
        state.sidebar_cache.bust(user_id);
        state.events.emit_sidebar(user_id);
    }
    // Re-encode the keyword for the redirect querystring using the `url` crate
    // already in the dependency tree (see services/sanitize.rs).
    let redirect = match search.as_ref() {
        Some(s) => format!(
            "{}?q={}",
            base_path,
            url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
        ),
        None => base_path.to_string(),
    };
    FlashRedirect::success(&redirect, format!("Marked {} matching entries as read.", affected))
        .into_response()
}
```

Notes:
- `state.db.user(...)` is the priority (write) connection accessor — the same one the GReader `mark_all_as_read` handler uses. `user_detached` is fire-and-forget (no return) and won't work here because we need the count for the flash.
- URL encoding uses `url::form_urlencoded::byte_serialize` (the `url` crate is already a dependency; see `src/services/sanitize.rs:247`). No new dependency.
- Add `use crate::middleware::flash::FlashRedirect;` at the top of the module if not already present (fully-qualify otherwise).

- [ ] **Step 2: Register the routes**

In `src/lib.rs`, near the existing `/categories/{id}/entries` (234) and `/feeds/{id}/entries` (241) routes add:

```rust
        .route("/categories/{id}/entries/mark-read", post(handlers::pages::category_mark_read_form))
        .route("/feeds/{id}/entries/mark-read", post(handlers::pages::feed_mark_read_form))
```

(Ensure `post` is imported — it is, given other POST routes exist.)

- [ ] **Step 3: Write a handler test**

Add an integration/handler test mirroring existing category-page handler tests (grep for how they build an app/router + auth). Assert: seed a category with a matching + non-matching entry, `POST /categories/{id}/entries/mark-read` with `q=超少女`, then the matching entry is read and the non-matching is not, and the response is a 303 to `/categories/{id}/entries?q=...`.

- [ ] **Step 4: Build + run**

Run: `cargo build && cargo nextest run -p rdrs mark_read`
Expected: PASS. Then `cargo fmt`; `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/pages/mod.rs src/lib.rs
git commit -S -m "feat(entries): POST mark-read action for scoped-search matches"
```

---

### Task 11: E2E BDD scenario

**Files:**
- Create/Modify: an `e2e/features/*.feature` file (add a scoped-search scenario) + any step definitions needed
- Run from `e2e/`

- [ ] **Step 1: Rebuild the binary (embedded assets)**

Run: `cargo build`
Expected: PASS. E2E global-setup skips the build if a binary exists, so build first to pick up template/JS/Rust changes.

- [ ] **Step 2: Add the scenario**

In an existing entries-related feature (grep `e2e/features` for category/feed list scenarios to match tags/background), add:

```gherkin
  Scenario: Scoped search within a category, then mark matching as read
    Given I am signed in with seeded feeds and entries
    And a category "動畫" containing entries titled "超少女登場" and "其他新聞"
    When I open the "動畫" category
    And I type "超少女" into the scoped search box
    Then the entry list shows "超少女登場"
    And the entry list does not show "其他新聞"
    When I click "Mark 1 matching as Read"
    Then "超少女登場" is no longer in the unread list
```

Reuse existing step definitions where possible (sign-in, seeding, opening a category). For new steps, target `[data-testid="scoped-search-input"]` and `[data-testid="mark-matching-btn"]`. If seeding a specific category/entries isn't already supported, extend the existing seeding step (grep `e2e/` for the seed helper) rather than inventing a new mechanism.

- [ ] **Step 3: Regenerate specs + run the scenario**

Run (from `e2e/`): `npx bddgen && npx playwright test --grep "Scoped search within a category"`
Expected: PASS.

- [ ] **Step 4: Run the entries E2E suite for regressions**

Run (from `e2e/`): `npx playwright test --grep "@entries" ` (or the tag the entries features use)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add e2e/features/<file>.feature e2e/steps/<changed>.ts
git commit -S -m "test(e2e): scoped search + mark matching as read"
```

---

## Self-Review Notes

- **Spec coverage:** A1→T1, A2→T2, A3→T3, A4→T4, A5→T5, B1→T6, B2→T6+T7, B3→T8, C1→T9, C2→T10, C3→T7(button render)+T10(handler), Testing→each task + T11, screenshots→Global Constraints (no regen).
- **Deviation from spec (intentional):** `content_text` is computed **inside** `upsert_entry_id` (Task 3), not passed as a new parameter — DRY, single strip site, zero caller churn. `feed_sync.rs` is untouched. The spec's "add a parameter … feed_sync computes" is superseded by this cleaner approach.
- **Deviation (correctness):** a `strip_to_search_text` variant (no tag-boundary space) is used for the searchable column so tag-split terms (`超<b>少女</b>`) match; `strip_to_plain_text` (with spaces) stays for display snippets. This wasn't explicit in the spec but is required for the "match plain text not HTML" goal to actually work for CJK inline markup.
- **Verification points resolved before hand-off:** write accessor is `state.db.user(...)` returning `Result<AppResult<i64>, DbError>` (Task 10 code updated); URL encoding uses the already-present `url` crate (no new dependency); `EntryFilter` already `#[derive(Clone)]` (mod.rs:58).
- **Still verify during execution (do not guess):** exact test-harness helper names for obtaining a conn + seeded feed/user in the entry/filters/schema test modules — the plan writes `test_conn()`/`seed_feed()` as placeholders; grep each target test module for the real helpers and substitute. Likewise confirm the E2E sign-in/seeding steps that already exist before adding new ones.
```
