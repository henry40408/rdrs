# Statistics "Database" Section Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an admin-only "Database" section to `/statistics` showing DB storage and `entry`-table record stats, backed by a new `created_at` index.

**Architecture:** A period-independent model function (`get_admin_database_stats`) runs three `PRAGMA`s plus four cheap `entry`/`entry_tombstone` queries on the read connection. The handler gates it behind `show_admin_stats`, pre-formats bytes/percent/day values into a view struct, and the Askama template renders a 3×2 card grid below the existing Site-wide block. A schema migration adds `idx_entry_created_at` so `MIN/MAX(created_at)` hit index endpoints.

**Tech Stack:** Rust, Axum, Askama, rusqlite (SQLite), chrono. Tests via `cargo nextest`.

## Global Constraints

- Test runner: `cargo nextest run` (never `cargo test`). Prefix env: `RDRS_FAST_HASH=1` for local runs.
- `cargo fmt` before every commit; `cargo clippy -- -D warnings` must pass (warnings fail CI).
- Commits MUST be GPG-signed. Stage files explicitly by name — never `git add -A`/`.`.
- All work on branch `feat/statistics-database-section` (already created).
- Commit message footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Rebuild (`cargo build`) before any E2E run — assets are `include_str!`'d into the binary.
- `MIN(created_at)`/`MAX(created_at)` MUST be queried as bare lone aggregates (no `julianday()` wrapper) to preserve the index-endpoint optimization.
- All new metrics are period-independent (ignore the date picker).

---

## File Structure

- `src/db/schema.rs` — add `idx_entry_created_at` to the main batch; add `version < 9` migration record; bump `LATEST_VERSION` to 9. (Task 1)
- `src/models/statistics.rs` — add `AdminDatabaseStats` struct + `get_admin_database_stats()` + unit tests. (Task 2)
- `src/handlers/pages/mod.rs` — add `format_db_bytes()` helper, `AdminDatabaseStatsView` struct, gated fetch in the `read_user` closure, build the view, add `admin_db` field to `StatisticsTemplate`. (Task 3)
- `templates/statistics.html` — add the `{% if let Some(db) = admin_db %}` block. (Task 4)
- `static/css/app.css` — modify `.stats-admin-section`; add `.stats-cards--db` and `.stats-card-sub`. (Task 4)

---

## Task 1: Schema migration — `idx_entry_created_at`

**Files:**
- Modify: `src/db/schema.rs:83` (add index to main batch), `src/db/schema.rs:263-278` (migration record + version bump), `src/db/schema.rs:318` and `src/db/schema.rs:329` (test assertions).

**Interfaces:**
- Produces: index `idx_entry_created_at` on `entry(created_at)`; `PRAGMA user_version` == 9 after `init_db`.

- [ ] **Step 1: Update the two version-assertion tests to expect 9 (they will now fail)**

In `src/db/schema.rs`, change both occurrences of `assert_eq!(version, 8);` (around lines 318 and 329) to:

```rust
        assert_eq!(version, 9);
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs schema::tests`
Expected: FAIL — `test_init_db_sets_user_version` / the double-init test assert `9` but `init_db` still sets `8`.

- [ ] **Step 3: Add the index to the main `execute_batch` block**

In `src/db/schema.rs`, immediately after the existing line 83 (`CREATE INDEX IF NOT EXISTS idx_entry_sort_ts ON entry(COALESCE(published_at, created_at));`), add:

```sql
        CREATE INDEX IF NOT EXISTS idx_entry_created_at ON entry(created_at);
```

- [ ] **Step 4: Add the migration record and bump `LATEST_VERSION`**

In `src/db/schema.rs`, after the `if version < 8 { ... }` block (ends ~line 273) and before `const LATEST_VERSION`, add:

```rust
    if version < 9 {
        // idx_entry_created_at is created via CREATE INDEX IF NOT EXISTS in the
        // main batch above (picked up on restart, like the v5/v6/v7 indexes).
        // The version bump records that MIN/MAX(created_at) admin stats can rely
        // on the index endpoint optimization being available.
    }
```

Then change `const LATEST_VERSION: i64 = 8;` to:

```rust
    const LATEST_VERSION: i64 = 9;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs schema::tests`
Expected: PASS.

- [ ] **Step 6: Verify the index exists at runtime (add a focused test)**

In `src/db/schema.rs` `mod tests`, add:

```rust
    #[test]
    fn test_init_db_creates_entry_created_at_index() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_entry_created_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
```

- [ ] **Step 7: Run it**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_init_db_creates_entry_created_at_index`
Expected: PASS.

- [ ] **Step 8: Format + commit**

```bash
cargo fmt
git add src/db/schema.rs
git commit -m "feat(db): add idx_entry_created_at index (schema v9)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Model — `AdminDatabaseStats` + `get_admin_database_stats`

**Files:**
- Modify: `src/models/statistics.rs` (add struct near the other admin structs ~line 59; add function near `get_admin_entry_stats` ~line 318; add tests + helpers in `mod tests`).

**Interfaces:**
- Consumes: `idx_entry_created_at` (Task 1) for fast MIN/MAX.
- Produces:
  ```rust
  pub struct AdminDatabaseStats {
      pub db_size_bytes: i64,
      pub reclaimable_bytes: i64,
      pub fragmentation_ratio: f64,
      pub total_entries: i64,
      pub avg_new_entries_per_day: f64,
      pub coverage_days: f64,
      pub tombstone_count: i64,
  }
  pub fn get_admin_database_stats(conn: &Connection) -> AppResult<AdminDatabaseStats>;
  ```

- [ ] **Step 1: Add test helpers for explicit `created_at` and tombstones**

In `src/models/statistics.rs` `mod tests`, after the existing `insert_entry` helper (~line 371), add:

```rust
    /// Helper: insert an entry with an explicit created_at (YYYY-MM-DD HH:MM:SS).
    fn insert_entry_created_at(conn: &Connection, feed_id: i64, guid: &str, created_at: &str) -> i64 {
        conn.execute(
            "INSERT INTO entry (feed_id, guid, created_at) VALUES (?1, ?2, ?3)",
            params![feed_id, guid, created_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Helper: insert a tombstone row.
    fn insert_tombstone(conn: &Connection, feed_id: i64, guid: &str) {
        conn.execute(
            "INSERT INTO entry_tombstone (feed_id, guid) VALUES (?1, ?2)",
            params![feed_id, guid],
        )
        .unwrap();
    }
```

- [ ] **Step 2: Write the failing tests**

In `src/models/statistics.rs` `mod tests`, before the closing `}` of the module (after `test_admin_entry_stats`, ~line 558), add:

```rust
    #[test]
    fn test_admin_database_stats_empty() {
        let conn = setup_db();
        create_user_with_data(&conn);

        let s = get_admin_database_stats(&conn).unwrap();

        // A freshly-initialized DB still has pages, so size is positive.
        assert!(s.db_size_bytes > 0);
        assert!(s.reclaimable_bytes >= 0);
        assert!((0.0..=1.0).contains(&s.fragmentation_ratio));
        // No entries / tombstones yet → record metrics are zero.
        assert_eq!(s.total_entries, 0);
        assert_eq!(s.coverage_days, 0.0);
        assert_eq!(s.avg_new_entries_per_day, 0.0);
        assert_eq!(s.tombstone_count, 0);
    }

    #[test]
    fn test_admin_database_stats_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // 4 entries spanning exactly 3 days (2024-01-01 .. 2024-01-04).
        insert_entry_created_at(&conn, feed_id, "a", "2024-01-01 00:00:00");
        insert_entry_created_at(&conn, feed_id, "b", "2024-01-02 00:00:00");
        insert_entry_created_at(&conn, feed_id, "c", "2024-01-03 00:00:00");
        insert_entry_created_at(&conn, feed_id, "d", "2024-01-04 00:00:00");

        insert_tombstone(&conn, feed_id, "dead-1");
        insert_tombstone(&conn, feed_id, "dead-2");

        let s = get_admin_database_stats(&conn).unwrap();

        assert_eq!(s.total_entries, 4);
        assert_eq!(s.tombstone_count, 2);
        // span = 2024-01-04 - 2024-01-01 = 3 days exactly.
        assert!((s.coverage_days - 3.0).abs() < 1e-6, "coverage was {}", s.coverage_days);
        // created_at is in the past, so age > 0 and avg is positive & finite.
        assert!(s.avg_new_entries_per_day > 0.0 && s.avg_new_entries_per_day.is_finite());
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs admin_database_stats`
Expected: FAIL — `get_admin_database_stats` not found / `AdminDatabaseStats` undefined.

- [ ] **Step 4: Add the struct**

In `src/models/statistics.rs`, after the `AdminEntryStats` impl block (~line 69), add:

```rust
/// Admin database storage + record stats (period-independent).
pub struct AdminDatabaseStats {
    pub db_size_bytes: i64,
    pub reclaimable_bytes: i64,
    pub fragmentation_ratio: f64,
    pub total_entries: i64,
    pub avg_new_entries_per_day: f64,
    pub coverage_days: f64,
    pub tombstone_count: i64,
}
```

- [ ] **Step 5: Add the function**

In `src/models/statistics.rs`, after `get_admin_entry_stats` (~line 318), add. The file already imports `chrono::NaiveDate`; this code uses the fully-qualified `chrono::NaiveDateTime` so no import change is needed:

```rust
/// Get site-wide database storage + record stats (period-independent).
pub fn get_admin_database_stats(conn: &Connection) -> AppResult<AdminDatabaseStats> {
    let page_count: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let freelist: i64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;

    let db_size_bytes = page_count * page_size;
    let reclaimable_bytes = freelist * page_size;
    let fragmentation_ratio = if db_size_bytes > 0 {
        reclaimable_bytes as f64 / db_size_bytes as f64
    } else {
        0.0
    };

    let total_entries: i64 =
        conn.query_row("SELECT COUNT(*) FROM entry", [], |row| row.get(0))?;
    // Bare MIN/MAX so SQLite uses the idx_entry_created_at endpoint optimization.
    let min_created: Option<String> =
        conn.query_row("SELECT MIN(created_at) FROM entry", [], |row| row.get(0))?;
    let max_created: Option<String> =
        conn.query_row("SELECT MAX(created_at) FROM entry", [], |row| row.get(0))?;
    let tombstone_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM entry_tombstone", [], |row| row.get(0))?;

    let parse = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok();
    let (coverage_days, avg_new_entries_per_day) = match (
        min_created.as_deref().and_then(parse),
        max_created.as_deref().and_then(parse),
    ) {
        (Some(min), Some(max)) => {
            let coverage = (max - min).num_seconds() as f64 / 86_400.0;
            let now = chrono::Utc::now().naive_utc();
            let age_days = (now - min).num_seconds() as f64 / 86_400.0;
            let avg = if age_days > 0.0 {
                total_entries as f64 / age_days
            } else {
                0.0
            };
            (coverage, avg)
        }
        _ => (0.0, 0.0),
    };

    Ok(AdminDatabaseStats {
        db_size_bytes,
        reclaimable_bytes,
        fragmentation_ratio,
        total_entries,
        avg_new_entries_per_day,
        coverage_days,
        tombstone_count,
    })
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs admin_database_stats`
Expected: PASS (both tests).

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/models/statistics.rs
git commit -m "feat(stats): add get_admin_database_stats model query

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Handler — view struct, byte formatter, gated fetch, template field

**Files:**
- Modify: `src/handlers/pages/mod.rs` — add `format_db_bytes` (near other free helpers), `AdminDatabaseStatsView` struct (after `AdminStatsView` ~line 2163), `admin_db` field on `StatisticsTemplate` (~line 2185), fetch + view build in `statistics_page` (~lines 2556-2685).

**Interfaces:**
- Consumes: `crate::models::statistics::{get_admin_database_stats, AdminDatabaseStats}` (Task 2).
- Produces:
  ```rust
  pub struct AdminDatabaseStatsView {
      pub size_fmt: String,
      pub reclaimable_fmt: String,
      pub frag_pct: i64,
      pub total_entries: i64,
      pub avg_per_day_fmt: String,
      pub coverage_fmt: String,
      pub tombstone_count: i64,
  }
  ```
  `StatisticsTemplate.admin_db: Option<AdminDatabaseStatsView>`.

- [ ] **Step 1: Write the failing test for the byte formatter**

In `src/handlers/pages/mod.rs`, inside the existing `#[cfg(test)] mod tests` block (starts at line 2739), add:

```rust
    #[test]
    fn test_format_db_bytes() {
        assert_eq!(super::format_db_bytes(0), "0 B");
        assert_eq!(super::format_db_bytes(512), "512 B");
        assert_eq!(super::format_db_bytes(1536), "1.5 KB");
        assert_eq!(super::format_db_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(super::format_db_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_format_db_bytes`
Expected: FAIL — `format_db_bytes` not found.

- [ ] **Step 3: Add the byte formatter**

In `src/handlers/pages/mod.rs`, add a private free function (top-level, near other helpers):

```rust
/// Format a byte count for display (binary units, one decimal above 1 KB).
fn format_db_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}
```

- [ ] **Step 4: Run the formatter test to verify it passes**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_format_db_bytes`
Expected: PASS.

- [ ] **Step 5: Add the view struct**

In `src/handlers/pages/mod.rs`, after `AdminStatsView` (ends ~line 2163), add:

```rust
/// Database storage + record stats block (admin, non-masquerading).
pub struct AdminDatabaseStatsView {
    pub size_fmt: String,
    pub reclaimable_fmt: String,
    pub frag_pct: i64,
    pub total_entries: i64,
    pub avg_per_day_fmt: String,
    pub coverage_fmt: String,
    pub tombstone_count: i64,
}
```

- [ ] **Step 6: Add the template field**

In `StatisticsTemplate` (after `pub admin: Option<AdminStatsView>,` ~line 2185), add:

```rust
    pub admin_db: Option<AdminDatabaseStatsView>,
```

- [ ] **Step 7: Fetch inside the `read_user` closure**

In `statistics_page`, the closure returns a tuple `(overview, daily, cats, feeds, admin_counts, admin_entry_stats)`. Add a fetch and extend the tuple. Right after the `admin_entry_stats` binding (~line 2575-2579) add:

```rust
            let admin_db_stats = if show_admin_stats {
                crate::models::statistics::get_admin_database_stats(c).ok()
            } else {
                None
            };
```

Change the closure's `Ok::<_, AppError>((...))` to include it:

```rust
            Ok::<_, AppError>((
                overview,
                daily,
                cats,
                feeds,
                admin_counts,
                admin_entry_stats,
                admin_db_stats,
            ))
```

And change the destructuring binding (~line 2556) to:

```rust
    let (overview, daily, cats, feeds, admin_counts, admin_entry_stats, admin_db_stats) = state
        .db
        .read_user(move |c| {
```

- [ ] **Step 8: Build the view and pass it to the template**

In `statistics_page`, after the existing `let admin = match (admin_counts, admin_entry_stats) { ... };` block (~line 2664), add:

```rust
    let admin_db = admin_db_stats.map(|s| AdminDatabaseStatsView {
        size_fmt: format_db_bytes(s.db_size_bytes),
        reclaimable_fmt: format_db_bytes(s.reclaimable_bytes),
        frag_pct: (s.fragmentation_ratio * 100.0).round() as i64,
        total_entries: s.total_entries,
        avg_per_day_fmt: format!("{}", s.avg_new_entries_per_day.round() as i64),
        coverage_fmt: format!("{}d", s.coverage_days.round() as i64),
        tombstone_count: s.tombstone_count,
    });
```

Then add `admin_db,` to the `StatisticsTemplate { ... }` struct literal (after `admin,` ~line 2685):

```rust
            admin,
            admin_db,
```

- [ ] **Step 9: Build and run the full handler/model test set**

Run: `cargo build` then `RDRS_FAST_HASH=1 cargo nextest run -p rdrs`
Expected: compiles; all tests PASS. (Template still doesn't reference `admin_db` yet — an unused struct field is allowed by Askama.)

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add src/handlers/pages/mod.rs
git commit -m "feat(stats): wire admin database stats into statistics handler

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Template + CSS — render the Database section

**Files:**
- Modify: `templates/statistics.html:129` (add block after the Site-wide `{% endif %}`).
- Modify: `static/css/app.css:2457-2461` (`.stats-admin-section`), and add `.stats-cards--db` + `.stats-card-sub` near the other `.stats-*` rules (~line 2372).

**Interfaces:**
- Consumes: `admin_db: Option<AdminDatabaseStatsView>` with fields `size_fmt`, `reclaimable_fmt`, `frag_pct`, `total_entries`, `avg_per_day_fmt`, `coverage_fmt`, `tombstone_count` (Task 3).

- [ ] **Step 1: Add the template block**

In `templates/statistics.html`, immediately after the Site-wide section's closing `{% endif %}` (line 129) and before the closing `</div>` of `.page-content`, add:

```html
                {% if let Some(db) = admin_db %}
                <div class="stats-admin-section">
                    <h2>Database</h2>
                    <div class="stats-cards stats-cards--db">
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value">{{ db.size_fmt }}</div>
                            <div class="stats-card-label">Database Size</div>
                        </div>
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value">{{ db.reclaimable_fmt }}</div>
                            <div class="stats-card-sub">{{ db.frag_pct }}% of file</div>
                            <div class="stats-card-label">Reclaimable</div>
                        </div>
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value" data-testid="stat-db-total-entries">{{ db.total_entries }}</div>
                            <div class="stats-card-label">Total Entries</div>
                        </div>
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value">{{ db.avg_per_day_fmt }}</div>
                            <div class="stats-card-label">Avg New / Day</div>
                        </div>
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value">{{ db.coverage_fmt }}</div>
                            <div class="stats-card-label">Coverage</div>
                        </div>
                        <div class="stats-card stats-card-admin">
                            <div class="stats-card-value" data-testid="stat-db-pruned">{{ db.tombstone_count }}</div>
                            <div class="stats-card-label">Pruned Entries</div>
                        </div>
                    </div>
                </div>
                {% endif %}
```

- [ ] **Step 2: Modify `.stats-admin-section` (remove divider + heavy top spacing)**

In `static/css/app.css`, replace the existing rule (lines ~2457-2461):

```css
.stats-admin-section {
    border-top: 1px solid var(--color-border);
    padding-top: var(--space-6);
    margin-top: var(--space-6);
}
```

with:

```css
.stats-admin-section {
    margin-top: var(--space-6);
}
.stats-admin-section h2 {
    margin-top: 0;
}
```

- [ ] **Step 3: Add the grid + sub-line rules**

In `static/css/app.css`, after the `.stats-card-admin { ... }` rule (~line 2375), add:

```css
.stats-cards--db { grid-template-columns: repeat(3, 1fr); }
@media (max-width: 768px) { .stats-cards--db { grid-template-columns: repeat(2, 1fr); } }
@media (max-width: 480px) { .stats-cards--db { grid-template-columns: 1fr; } }
.stats-card-sub {
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    color: var(--color-accent);
    margin-top: var(--space-1);
    font-weight: 500;
}
```

- [ ] **Step 4: Build (compiles templates + embeds assets)**

Run: `cargo build`
Expected: success. Askama validates the new `{{ db.* }}` references against `AdminDatabaseStatsView`.

- [ ] **Step 5: Run the full test suite**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs`
Expected: all PASS.

- [ ] **Step 6: Visual sanity check (manual, optional but recommended)**

Run the app, log in as an admin (first registered user), open `/statistics`, and confirm the "Database" section renders 6 cards in a 3×2 grid below "Site-wide Statistics" with no divider lines, in both light and dark themes. A non-admin (or masquerading admin) must NOT see the section.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt
cargo clippy -- -D warnings
git add templates/statistics.html static/css/app.css
git commit -m "feat(stats): render admin Database section on /statistics

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] `RDRS_FAST_HASH=1 cargo nextest run` all green.
- [ ] `cargo build` succeeds (assets embedded).
- [ ] Manual: admin sees the Database section; non-admin / masquerading admin does not.

## Notes / deferred (from spec)

- No `dbstat` per-table breakdown, no `-wal` size, no caching — out of scope.
- `/statistics` is not among the four README screenshots, so no screenshot regeneration is required.
- Do not touch version fields or `CHANGELOG.md`.
