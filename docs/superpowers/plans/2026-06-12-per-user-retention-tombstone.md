# Per-User Read-Entry Retention + Tombstones Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, per-user retention policy that deletes old *read* entries, backed by an `entry_tombstone` table that prevents deleted entries from being re-imported as unread on the next feed refresh, plus a gated-VACUUM maintenance step so the database file actually shrinks.

**Architecture:** A new `entry_tombstone(feed_id, guid, created_at)` table records deletions. The feed-sync insert path gains an atomic `WHERE NOT EXISTS (entry_tombstone)` guard. A new background worker (`entry_retention`) periodically prunes read+aged+non-starred entries per each user's `user_settings.retention_read_days` threshold (batches of 500, tombstone+delete per batch in one transaction), then runs maintenance (`PRAGMA optimize`, a `VACUUM` gated at ≥20% freelist, `wal_checkpoint(TRUNCATE)`). Opt-in lives in the existing user-settings UI; `0` = disabled (default).

**Tech Stack:** Rust, rusqlite (raw SQL, `prepare_cached`), SQLite (WAL), tokio background worker + `CancellationToken`, Askama SSR template, playwright-bdd.

**Spec:** `docs/superpowers/specs/2026-06-12-per-user-retention-tombstone-design.md`

---

## Environment notes (this box)

- **Before every cargo command**, re-source the OpenSSL env: `source /tmp/rdrs-env.sh && <cargo cmd>`.
- Run tests with **`cargo nextest run`** (not `cargo test`).
- Run **`cargo fmt`** before each commit.
- Run a **`pwd`** check first; all cargo/git commands run from `/home/nixos/Develop/claude/rdrs`.
- Commits are **GPG-signed** (default). Stage files **explicitly by name** (never `git add -A`/`.`).

## File Structure

- `src/db/schema.rs` — add `entry_tombstone` table + `user_settings.retention_read_days` column; bump schema version 7→8. (Migration)
- `src/models/user_settings.rs` — `retention_read_days` field, getter, `update_retention_read_days()`. (Per-user opt-in storage)
- `src/models/entry/mod.rs` — `UpsertOutcome` enum + tombstone guard in `upsert_entry_id`; adapt `upsert_entry` wrapper; `insert_tombstone()` test helper; `prune_read_retention_batch()`. (Tombstone guard + pruning core)
- `src/services/feed_sync.rs` — map `UpsertOutcome` (new/updated/skipped counters). (Refresh adaptation)
- `src/services/entry_retention.rs` — **new**: `start_retention_worker()` + `run_maintenance()`. (Background worker)
- `src/services/mod.rs` — register/export the new module.
- `src/main.rs` — start the worker; add to graceful-shutdown join set.
- `src/handlers/pages/mod.rs` — load `retention_read_days` into `UserSettingsTemplate`.
- `src/handlers/user.rs` — accept `retention_read_days` in `UpdatePreferencesForm`.
- `templates/user_settings.html` — number input in the preferences form.
- `e2e/` — a BDD scenario for the new settings field.

---

### Task 1: Migration — `entry_tombstone` table + `retention_read_days` column

**Files:**
- Modify: `src/db/schema.rs` (main `execute_batch` block; migration block ~`src/db/schema.rs:236-247`; tests ~`:256-298`)

- [ ] **Step 1: Update the version tests to expect 8 and assert the new table**

In `src/db/schema.rs`, in `test_init_db` add an assertion, and change both `assert_eq!(version, 7)` to `8`:

```rust
        assert!(tables.contains(&"entry_summary".to_string()));
        assert!(tables.contains(&"entry_tombstone".to_string()));
```

```rust
        assert_eq!(version, 8);
```

(There are two `assert_eq!(version, 7)` occurrences — in `test_init_db_idempotent` and `test_init_db_sets_user_version`. Change both to `8`.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs db::schema`
Expected: FAIL — `entry_tombstone` not in table list; version is 7 not 8.

- [ ] **Step 3: Add the table to the main batch and the column to `user_settings`**

In the big `conn.execute_batch(r#" ... "#)` block: add the `retention_read_days` column to the `user_settings` CREATE TABLE (after `entries_per_page`):

```sql
        CREATE TABLE IF NOT EXISTS user_settings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL UNIQUE REFERENCES user(id) ON DELETE CASCADE,
            entries_per_page INTEGER NOT NULL DEFAULT 30,
            retention_read_days INTEGER NOT NULL DEFAULT 0,
            save_services TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
```

And add the tombstone table near the `entry` indexes (anywhere inside the batch):

```sql
        -- Tombstones for entries deleted by retention. Keyed by (feed_id, guid),
        -- mirroring entry's UNIQUE(feed_id, guid). The refresh insert path checks
        -- this so a deleted entry the feed still serves is not re-imported as
        -- unread. created_at follows the schema-wide convention; kept forever
        -- (lightweight, no GC). Cascades when the feed is deleted.
        CREATE TABLE IF NOT EXISTS entry_tombstone (
            feed_id    INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
            guid       TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (feed_id, guid)
        ) WITHOUT ROWID;
```

- [ ] **Step 4: Add the v8 migration block and bump `LATEST_VERSION`**

Replace the `if version < 7 { ... }` ... `const LATEST_VERSION: i64 = 7;` tail with an added block and bumped constant:

```rust
    if version < 8 {
        // Add the per-user retention threshold to existing databases. The
        // entry_tombstone table is created via CREATE TABLE IF NOT EXISTS in the
        // main batch above (picked up on restart). `let _ =` swallows the
        // duplicate-column error on fresh DBs where the main batch already added
        // the column (mirrors the v1/v2 legacy ALTERs).
        let _ = conn.execute(
            "ALTER TABLE user_settings ADD COLUMN retention_read_days INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }

    const LATEST_VERSION: i64 = 8;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs db::schema`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/db/schema.rs
git commit -m "feat(db): add entry_tombstone table and retention_read_days column (schema v8)"
```

---

### Task 2: `user_settings` model — retention threshold storage

**Files:**
- Modify: `src/models/user_settings.rs` (struct ~`:12-21`, `row_to_user_settings` ~`:43-56`, SELECT in `find_by_user_id` ~`:60`, add fns near `update_theme` ~`:140`, tests)

- [ ] **Step 1: Write failing tests for the getter and updater**

Add to the `tests` module in `src/models/user_settings.rs`:

```rust
    #[test]
    fn test_retention_read_days_default_zero() {
        let conn = setup_db();
        let user = user::create_user(&conn, "ret", "hash", Role::User).unwrap();
        assert_eq!(get_retention_read_days(&conn, user.id).unwrap(), 0);
    }

    #[test]
    fn test_update_retention_read_days() {
        let conn = setup_db();
        let user = user::create_user(&conn, "ret", "hash", Role::User).unwrap();

        update_retention_read_days(&conn, user.id, 30).unwrap();
        assert_eq!(get_retention_read_days(&conn, user.id).unwrap(), 30);

        // Preserves other settings.
        upsert(&conn, user.id, 50).unwrap();
        update_retention_read_days(&conn, user.id, 14).unwrap();
        let s = find_by_user_id(&conn, user.id).unwrap().unwrap();
        assert_eq!(s.retention_read_days, 14);
        assert_eq!(s.entries_per_page, 50);

        // Negatives are rejected.
        assert!(matches!(
            update_retention_read_days(&conn, user.id, -1),
            Err(AppError::Validation(_))
        ));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs user_settings`
Expected: FAIL — `get_retention_read_days`/`update_retention_read_days` not found; field `retention_read_days` missing.

- [ ] **Step 3: Add the field, mapping, SELECT column, and functions**

In `struct UserSettings`, add after `entries_per_page`:

```rust
    pub retention_read_days: i64,
```

In `row_to_user_settings`, the column order changes (new column is index 2 in the SELECT below, shifting the rest). Update the SELECT in `find_by_user_id` and the row mapping together:

```rust
pub fn find_by_user_id(conn: &Connection, user_id: i64) -> AppResult<Option<UserSettings>> {
    conn.query_row(
        "SELECT id, user_id, entries_per_page, retention_read_days, save_services, theme, created_at, updated_at FROM user_settings WHERE user_id = ?1",
        params![user_id],
        row_to_user_settings,
    )
    .optional()
    .map_err(AppError::Database)
}
```

```rust
fn row_to_user_settings(row: &rusqlite::Row) -> rusqlite::Result<UserSettings> {
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;

    Ok(UserSettings {
        id: row.get(0)?,
        user_id: row.get(1)?,
        entries_per_page: row.get(2)?,
        retention_read_days: row.get(3)?,
        save_services: row.get(4)?,
        theme: row.get(5)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}
```

Add the two functions after `update_theme` (~`:155`):

```rust
/// Get the per-user read-entry retention threshold in days (0 = disabled).
pub fn get_retention_read_days(conn: &Connection, user_id: i64) -> AppResult<i64> {
    match find_by_user_id(conn, user_id)? {
        Some(settings) => Ok(settings.retention_read_days),
        None => Ok(0),
    }
}

/// Set the per-user read-entry retention threshold in days. `0` disables
/// retention for the user; negative values are rejected.
pub fn update_retention_read_days(conn: &Connection, user_id: i64, days: i64) -> AppResult<()> {
    if days < 0 {
        return Err(AppError::Validation(
            "retention_read_days must be >= 0".to_string(),
        ));
    }
    // Ensure a row exists, then update (mirrors update_theme).
    conn.execute(
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO NOTHING",
        params![user_id, DEFAULT_ENTRIES_PER_PAGE],
    )?;
    conn.execute(
        "UPDATE user_settings SET retention_read_days = ?1, updated_at = datetime('now') WHERE user_id = ?2",
        params![days, user_id],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run to verify pass**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs user_settings`
Expected: PASS (existing `user_settings` tests still pass — the struct gained a field but all constructors go through SQL).

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/models/user_settings.rs
git commit -m "feat(settings): per-user retention_read_days getter/updater"
```

---

### Task 3: Tombstone guard in the entry insert path

**Files:**
- Modify: `src/models/entry/mod.rs` (`upsert_entry_id` ~`:484-539`, `upsert_entry` ~`:547-571`, add `insert_tombstone`, update test `test_upsert_entry_id_returns_id_and_is_new` ~`:1751`)

- [ ] **Step 1: Write failing tests for the guard and the new outcome type**

Add to the `tests` module in `src/models/entry/mod.rs` (the helpers `setup_db`, feed/category/user creation already exist in that module's tests — follow the existing pattern used by nearby tests like `test_upsert_entry_id_returns_id_and_is_new`):

```rust
    #[test]
    fn test_upsert_skips_tombstoned_guid() {
        let conn = setup_db();
        let feed_id = setup_feed(&conn); // existing test helper in this module

        // Tombstone a guid, then a refresh that serves it must be skipped.
        insert_tombstone(&conn, feed_id, "ghost").unwrap();
        let outcome = upsert_entry_id(
            &conn, feed_id, "ghost", Some("Ghost"), None, None, None, None, None,
        )
        .unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));

        // The entry must not exist.
        assert!(find_by_guid_and_feed(&conn, "ghost", feed_id).unwrap().is_none());
    }

    #[test]
    fn test_upsert_inserts_then_updates() {
        let conn = setup_db();
        let feed_id = setup_feed(&conn);

        let first = upsert_entry_id(
            &conn, feed_id, "g1", Some("First"), None, None, None, None, None,
        )
        .unwrap();
        let id = match first {
            UpsertOutcome::Inserted(id) => id,
            other => panic!("expected Inserted, got {other:?}"),
        };

        let second = upsert_entry_id(
            &conn, feed_id, "g1", Some("Updated"), None, None, None, None, None,
        )
        .unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(uid) if uid == id));
    }
```

Also update the existing `test_upsert_entry_id_returns_id_and_is_new` (it destructures `(id1, is_new)`): replace its body's `upsert_entry_id` assertions to match the enum, e.g.:

```rust
        let first = upsert_entry_id(&conn, feed_id, "guid-1", Some("Title"), None, None, None, None, None).unwrap();
        let id1 = match first { UpsertOutcome::Inserted(id) => id, o => panic!("expected Inserted, got {o:?}") };

        let second = upsert_entry_id(&conn, feed_id, "guid-1", Some("Title 2"), None, None, None, None, None).unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(id) if id == id1));
```

If `setup_feed` does not already exist as a helper in this test module, add it near the top of the `tests` module:

```rust
    fn setup_feed(conn: &Connection) -> i64 {
        let user_id = crate::models::user::create_user(conn, "u", "h", crate::models::user::Role::User).unwrap().id;
        let category_id = crate::models::category::create_category(conn, user_id, "C").unwrap().id;
        crate::models::feed::create_feed(conn, &crate::models::feed::CreateFeedParams {
            category_id, url: "https://example.com/f.xml", title: Some("F"),
            description: None, site_url: None, custom_user_agent: None,
            http2_disabled: None, custom_referrer: None,
        }).unwrap().id
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs entry::`
Expected: FAIL — `UpsertOutcome`/`insert_tombstone` not defined; `upsert_entry_id` still returns a tuple.

- [ ] **Step 3: Define `UpsertOutcome`, add the guard, add `insert_tombstone`**

Above `upsert_entry_id`, add the enum:

```rust
/// Result of an entry upsert. The insert path is guarded against tombstones,
/// so a third "skipped" state exists alongside insert/update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted(i64),
    Updated(i64),
    SkippedTombstoned,
}
```

Change `upsert_entry_id`'s return type and its two `Ok(...)` returns and the INSERT:

```rust
) -> AppResult<UpsertOutcome> {
```

```rust
        .execute(params![title, link, content, summary, author, id])?;

        return Ok(UpsertOutcome::Updated(id));
    }

    // Insert, but never resurrect a tombstoned guid. The WHERE NOT EXISTS makes
    // the tombstone check atomic with the insert, so a retention delete that
    // commits between a separate check and this statement cannot bring the
    // entry back as unread.
    let inserted = conn
        .prepare_cached(
            r#"
            INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at)
            SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
            WHERE NOT EXISTS (
                SELECT 1 FROM entry_tombstone WHERE feed_id = ?1 AND guid = ?2
            )
            "#,
        )?
        .execute(params![
            feed_id, guid, title, link, content, summary, author, published_at_str
        ])?;

    if inserted == 0 {
        return Ok(UpsertOutcome::SkippedTombstoned);
    }
    Ok(UpsertOutcome::Inserted(conn.last_insert_rowid()))
}
```

Adapt the `upsert_entry` wrapper so its `(Entry, bool)` signature is unchanged (its ~30 callers stay compiling):

```rust
    let (id, is_new) = match upsert_entry_id(
        conn, feed_id, guid, title, link, content, summary, author, published_at,
    )? {
        UpsertOutcome::Inserted(id) => (id, true),
        UpsertOutcome::Updated(id) => (id, false),
        UpsertOutcome::SkippedTombstoned => {
            return Err(AppError::Internal(
                "upsert_entry called for a tombstoned guid".to_string(),
            ))
        }
    };
    let entry = find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)?;
    Ok((entry, is_new))
```

Add `insert_tombstone` near `upsert_entry_id` (used by retention and tests):

```rust
/// Record a tombstone for `(feed_id, guid)`. Idempotent.
pub fn insert_tombstone(conn: &Connection, feed_id: i64, guid: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO entry_tombstone (feed_id, guid) VALUES (?1, ?2)
         ON CONFLICT(feed_id, guid) DO NOTHING",
        params![feed_id, guid],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run to verify pass**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs entry::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/models/entry/mod.rs
git commit -m "feat(entry): tombstone-guarded insert via UpsertOutcome"
```

---

### Task 4: Adapt feed-sync to `UpsertOutcome`

**Files:**
- Modify: `src/services/feed_sync.rs` (entry loop ~`:286-374`)

- [ ] **Step 1: Update the closure to match the enum and count skips**

Replace the tuple destructure and counters. Change the closure's return tuple to include a skipped count and the `match`:

```rust
    let (new_entries, updated_entries, skipped_entries) = db
        .background(move |conn| {
            let mut new_entries = 0i64;
            let mut updated_entries = 0i64;
            let mut skipped_entries = 0i64;
            let mut latest_entry_date: Option<chrono::DateTime<Utc>> = None;

            let tx = conn.unchecked_transaction()?;

            for item in parsed_feed.entries {
                // ... unchanged extraction of guid/title/link/content/summary/author/published_at ...

                match entry::upsert_entry_id(
                    &tx,
                    feed_id,
                    &guid,
                    title.as_deref(),
                    link.as_deref(),
                    content.as_deref(),
                    summary.as_deref(),
                    author.as_deref(),
                    published_at,
                )? {
                    entry::UpsertOutcome::Inserted(_) => new_entries += 1,
                    entry::UpsertOutcome::Updated(_) => updated_entries += 1,
                    entry::UpsertOutcome::SkippedTombstoned => skipped_entries += 1,
                }
            }

            // ... unchanged effective_updated_at + feed::update_fetch_result + tx.commit() ...

            Ok::<_, AppError>((new_entries, updated_entries, skipped_entries))
        })
        .await??;

    info!(
        "Feed {} refreshed: {} new, {} updated, {} skipped (tombstoned)",
        feed_id, new_entries, updated_entries, skipped_entries
    );
```

(Leave the `published_at` "track latest_entry_date" block and the metadata update exactly as they are — only the upsert call/counters and the log line change. `SyncResult` keeps its existing `new_entries`/`updated_entries` fields; `skipped_entries` is log-only.)

- [ ] **Step 2: Build to verify it compiles**

Run: `source /tmp/rdrs-env.sh && cargo build -p rdrs`
Expected: PASS (no type errors).

- [ ] **Step 3: Run feed-sync tests**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs feed_sync`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/services/feed_sync.rs
git commit -m "feat(feed-sync): handle tombstoned-skip outcome, log skip count"
```

---

### Task 5: Pruning core — `prune_read_retention_batch`

**Files:**
- Modify: `src/models/entry/mod.rs` (add function + tests)

- [ ] **Step 1: Write failing tests**

Add to the entry `tests` module:

```rust
    #[test]
    fn test_prune_respects_threshold_star_and_optin() {
        let conn = setup_db();
        let feed_id = setup_feed(&conn); // feed belongs to the user created in setup_feed

        // Helper: insert a read entry aged `days_old`.
        let mk = |guid: &str, days: i64, starred: bool| {
            upsert_entry_id(&conn, feed_id, guid, Some(guid), None, None, None, None, None).unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now', ?2) WHERE guid = ?1 AND feed_id = ?3",
                rusqlite::params![guid, format!("-{days} days"), feed_id],
            ).unwrap();
            if starred {
                conn.execute(
                    "UPDATE entry SET starred_at = datetime('now') WHERE guid = ?1 AND feed_id = ?2",
                    rusqlite::params![guid, feed_id],
                ).unwrap();
            }
        };
        mk("old", 40, false);     // read, 40d, not starred -> victim once enabled
        mk("oldstar", 40, true);  // starred -> never deleted
        mk("fresh", 1, false);    // too recent -> kept
        // "unread" entry: insert without read_at
        upsert_entry_id(&conn, feed_id, "unread", Some("u"), None, None, None, None, None).unwrap();

        // Opt-in disabled (default 0): nothing pruned.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 0);

        // Enable retention at 30 days for the feed's owner.
        let user_id: i64 = conn.query_row(
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = ?1",
            rusqlite::params![feed_id], |r| r.get(0),
        ).unwrap();
        crate::models::user_settings::update_retention_read_days(&conn, user_id, 30).unwrap();

        // Only "old" is pruned; a tombstone is written for it.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 1);
        assert!(find_by_guid_and_feed(&conn, "old", feed_id).unwrap().is_none());
        assert!(find_by_guid_and_feed(&conn, "oldstar", feed_id).unwrap().is_some());
        assert!(find_by_guid_and_feed(&conn, "fresh", feed_id).unwrap().is_some());
        assert!(find_by_guid_and_feed(&conn, "unread", feed_id).unwrap().is_some());

        // Tombstone present -> a refresh serving "old" again is skipped.
        let outcome = upsert_entry_id(&conn, feed_id, "old", Some("Old"), None, None, None, None, None).unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));

        // Idempotent: nothing left to prune.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 0);
    }

    #[test]
    fn test_prune_batch_size_limits_rows() {
        let conn = setup_db();
        let feed_id = setup_feed(&conn);
        let user_id: i64 = conn.query_row(
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = ?1",
            rusqlite::params![feed_id], |r| r.get(0),
        ).unwrap();
        crate::models::user_settings::update_retention_read_days(&conn, user_id, 1).unwrap();
        for i in 0..5 {
            let g = format!("g{i}");
            upsert_entry_id(&conn, feed_id, &g, Some(&g), None, None, None, None, None).unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now', '-10 days') WHERE guid = ?1 AND feed_id = ?2",
                rusqlite::params![g, feed_id],
            ).unwrap();
        }
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 1);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs entry::`
Expected: FAIL — `prune_read_retention_batch` not defined.

- [ ] **Step 3: Implement the function**

Add to `src/models/entry/mod.rs` (near `insert_tombstone`):

```rust
/// Delete up to `batch_size` read, aged, non-starred entries belonging to users
/// who have opted into retention (`user_settings.retention_read_days > 0`),
/// recording a tombstone for each. Returns the number of entries deleted.
///
/// One batch runs in a single transaction so the tombstone+delete pair is
/// atomic against a concurrent feed refresh. Victims are gathered first (Rust
/// side) so the delete targets exact ids rather than re-running a `LIMIT`
/// without `ORDER BY`. Each user's own threshold is applied via the join.
pub fn prune_read_retention_batch(conn: &Connection, batch_size: usize) -> AppResult<u64> {
    let tx = conn.unchecked_transaction()?;

    let victims: Vec<(i64, i64, String)> = {
        let mut stmt = tx.prepare_cached(
            r#"
            SELECT e.id, e.feed_id, e.guid
            FROM entry e
            JOIN feed f           ON f.id = e.feed_id
            JOIN category c       ON c.id = f.category_id
            JOIN user_settings us ON us.user_id = c.user_id
            WHERE us.retention_read_days > 0
              AND e.read_at    IS NOT NULL
              AND e.starred_at IS NULL
              AND e.read_at < datetime('now', '-' || us.retention_read_days || ' days')
            LIMIT ?1
            "#,
        )?;
        stmt.query_map(params![batch_size as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    if victims.is_empty() {
        return Ok(0);
    }

    {
        let mut ins = tx.prepare_cached(
            "INSERT INTO entry_tombstone (feed_id, guid) VALUES (?1, ?2)
             ON CONFLICT(feed_id, guid) DO NOTHING",
        )?;
        let mut del = tx.prepare_cached("DELETE FROM entry WHERE id = ?1")?;
        for (id, feed_id, guid) in &victims {
            ins.execute(params![feed_id, guid])?;
            del.execute(params![id])?;
        }
    }

    tx.commit()?;
    Ok(victims.len() as u64)
}
```

- [ ] **Step 4: Run to verify pass**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs entry::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/models/entry/mod.rs
git commit -m "feat(entry): prune_read_retention_batch (delete + tombstone)"
```

---

### Task 6: Retention worker + maintenance

**Files:**
- Create: `src/services/entry_retention.rs`
- Modify: `src/services/mod.rs` (add `pub mod` + `pub use`)

- [ ] **Step 1: Write the worker module with failing tests**

Create `src/services/entry_retention.rs`:

```rust
use std::time::Duration;

use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::DbPool;
use crate::error::AppResult;
use crate::models::entry;

/// Entries deleted per transaction during a drain.
const BATCH_SIZE: usize = 500;
/// Run a full VACUUM only when freed pages reach this fraction of the file. A
/// full VACUUM rewrites the whole database under a write lock (~db_size/650
/// seconds), so it is not worth doing for the handful of pages a routine prune
/// frees — only after a large drain.
const VACUUM_FREELIST_RATIO: f64 = 0.20;

/// Start the retention worker. Every `interval_hours` it prunes read+aged
/// +non-starred entries for users who opted in (`user_settings.retention_read_days
/// > 0`), then runs maintenance. A no-op when nobody opted in.
pub fn start_retention_worker(
    db: DbPool,
    interval_hours: u64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!("Retention worker started: interval={}h", interval_hours);
        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Retention worker stopping...");
                    break;
                }
                _ = interval.tick() => {
                    let mut total = 0u64;
                    loop {
                        if cancel_token.is_cancelled() {
                            break;
                        }
                        let deleted = match db
                            .background(move |conn| entry::prune_read_retention_batch(conn, BATCH_SIZE))
                            .await
                        {
                            Ok(Ok(n)) => n,
                            Ok(Err(e)) => { tracing::error!("Retention prune failed: {}", e); break; }
                            Err(e) => { tracing::error!("Retention DB access failed: {}", e); break; }
                        };
                        total += deleted;
                        if deleted < BATCH_SIZE as u64 {
                            break;
                        }
                    }

                    if total > 0 {
                        tracing::info!("Retention pruned {} read entries", total);
                        match db.background(run_maintenance).await {
                            Ok(Ok(true)) => tracing::info!("Retention maintenance: VACUUM ran"),
                            Ok(Ok(false)) => {}
                            Ok(Err(e)) => tracing::error!("Retention maintenance failed: {}", e),
                            Err(e) => tracing::error!("Retention maintenance DB access failed: {}", e),
                        }
                    }
                }
            }
        }

        tracing::info!("Retention worker stopped");
    })
}

/// Post-prune maintenance: refresh planner stats, gated full VACUUM, truncating
/// WAL checkpoint. Returns whether a VACUUM ran. Must run outside a transaction.
pub fn run_maintenance(conn: &Connection) -> AppResult<bool> {
    conn.execute_batch("PRAGMA optimize;")?;

    let page_count: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let freelist: i64 = conn.pragma_query_value(None, "freelist_count", |r| r.get(0))?;
    let vacuumed = page_count > 0 && (freelist as f64 / page_count as f64) >= VACUUM_FREELIST_RATIO;
    if vacuumed {
        conn.execute_batch("VACUUM;")?;
    }

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(vacuumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::{category, feed, user, user_settings};
    use crate::models::user::Role;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn setup_pool() -> DbPool {
        let conn = setup_db();
        let read_conn = Connection::open_in_memory().unwrap();
        let (pool, _h) = DbPool::new(conn, read_conn);
        pool
    }

    #[test]
    fn test_run_maintenance_no_vacuum_below_ratio() {
        let conn = setup_db();
        // Fresh DB: ~0 freelist -> no VACUUM, but must not error.
        assert!(!run_maintenance(&conn).unwrap());
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancellation() {
        let db = setup_pool();
        let token = CancellationToken::new();
        let handle = start_retention_worker(db, 1000, token.clone());
        token.cancel();
        let res = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(res.is_ok(), "retention worker should stop after cancellation");
    }

    #[tokio::test]
    async fn test_drain_deletes_opted_in_aged_read_entries() {
        let db = setup_pool();
        db.user(|conn| {
            let uid = user::create_user(conn, "u", "h", Role::User).unwrap().id;
            let cid = category::create_category(conn, uid, "C").unwrap().id;
            let fid = feed::create_feed(conn, &feed::CreateFeedParams {
                category_id: cid, url: "https://e.com/f.xml", title: Some("F"),
                description: None, site_url: None, custom_user_agent: None,
                http2_disabled: None, custom_referrer: None,
            }).unwrap().id;
            entry::upsert_entry_id(conn, fid, "old", Some("o"), None, None, None, None, None).unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now','-40 days') WHERE guid='old' AND feed_id=?1",
                rusqlite::params![fid],
            ).unwrap();
            user_settings::update_retention_read_days(conn, uid, 30).unwrap();
        }).await.unwrap();

        // Simulate one worker tick's drain.
        let deleted: u64 = db
            .background(|conn| entry::prune_read_retention_batch(conn, BATCH_SIZE).unwrap())
            .await
            .unwrap();
        assert_eq!(deleted, 1);
    }
}
```

- [ ] **Step 2: Register the module**

In `src/services/mod.rs`, add (alphabetical, near `feed_sync`):

```rust
pub mod entry_retention;
```

and after the `feed_sync` re-export:

```rust
pub use entry_retention::start_retention_worker;
```

- [ ] **Step 3: Run to verify it builds and passes**

Run: `source /tmp/rdrs-env.sh && cargo nextest run -p rdrs entry_retention`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/services/entry_retention.rs src/services/mod.rs
git commit -m "feat(retention): background worker with batched prune + gated VACUUM"
```

---

### Task 7: Start the worker in `main.rs`

**Files:**
- Modify: `src/main.rs` (start near the cleanup worker ~`:69-71`; shutdown join ~`:115`)

- [ ] **Step 1: Start the worker after the summary cleanup worker**

After the `cleanup_worker_handle` lines, add:

```rust
    // Start read-entry retention worker (every 24h; per-user opt-in via
    // user_settings.retention_read_days, 0 = disabled). No-op when nobody opted in.
    let retention_worker_handle =
        services::start_retention_worker(db.clone(), 24, cancel_token.clone());
```

- [ ] **Step 2: Add it to the graceful-shutdown join set**

In the `tokio::join!(...)` shutdown block, add `retention_worker_handle,`:

```rust
        let _ = tokio::join!(
            background_handle,
            summary_worker_handle,
            cleanup_worker_handle,
            retention_worker_handle,
        );
```

- [ ] **Step 3: Build**

Run: `source /tmp/rdrs-env.sh && cargo build -p rdrs`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/main.rs
git commit -m "feat(main): start retention worker, wire into graceful shutdown"
```

---

### Task 8: Settings UI — expose `retention_read_days`

**Files:**
- Modify: `src/handlers/pages/mod.rs` (read closure ~`:656-700`, struct `UserSettingsTemplate` ~`:1998-2015`, construction ~`:725-742`)
- Modify: `src/handlers/user.rs` (`UpdatePreferencesForm` ~`:413-417`, `update_preferences_form` ~`:419-450`)
- Modify: `templates/user_settings.html` (preferences form ~`:72-76`)

- [ ] **Step 1: Add the field to the page template struct**

In `UserSettingsTemplate` (`src/handlers/pages/mod.rs:1998`), after `entries_per_page`:

```rust
    pub retention_read_days: i64,
```

- [ ] **Step 2: Load it in the page read closure and construct it**

In the `read_user` closure (~`:664-689`), add to the loaded tuple. Change the tuple binding, the closure return, the `unwrap_or` default, and the struct construction to thread `retention_read_days`:

```rust
    let (
        theme,
        entries_per_page,
        retention_read_days,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
    ) = state
        .db
        .read_user(move |conn| {
            let theme = user_settings::get_theme(conn, user_id).unwrap_or(None);
            let entries_per_page = user_settings::get_entries_per_page(conn, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let retention_read_days =
                user_settings::get_retention_read_days(conn, user_id).unwrap_or(0);
            let save_config =
                user_settings::get_save_services_config(conn, user_id).unwrap_or_default();

            let linkding = save_config.linkding.as_ref();
            let linkding_configured = linkding.map(|c| c.is_configured()).unwrap_or(false);
            let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

            let kagi = save_config.kagi.as_ref();
            let kagi_configured = kagi.map(|c| c.is_configured()).unwrap_or(false);
            let kagi_language = kagi.and_then(|c| c.language.clone());

            Ok::<_, AppError>((
                theme,
                entries_per_page,
                retention_read_days,
                linkding_configured,
                linkding_api_url,
                kagi_configured,
                kagi_language,
            ))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or((
            None,
            user_settings::DEFAULT_ENTRIES_PER_PAGE,
            0,
            false,
            String::new(),
            false,
            None,
        ));
```

In the `UserSettingsTemplate { ... }` construction (~`:725`), add after `entries_per_page,`:

```rust
            retention_read_days,
```

- [ ] **Step 3: Accept the field in the preferences form handler**

In `src/handlers/user.rs`, extend `UpdatePreferencesForm`:

```rust
#[derive(Debug, Deserialize)]
pub struct UpdatePreferencesForm {
    pub theme: String,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
}
```

In `update_preferences_form`, persist it alongside theme/epp:

```rust
    let epp = req.entries_per_page;
    let retention_read_days = req.retention_read_days;

    let result = state
        .db
        .user(move |conn| {
            user_settings::upsert(conn, user_id, epp)?;
            user_settings::update_theme(conn, user_id, theme)?;
            user_settings::update_retention_read_days(conn, user_id, retention_read_days)?;
            Ok::<_, AppError>(())
        })
        .await;
```

(The existing `match` already maps `AppError::Validation` to a flash error, so a negative value surfaces a message.)

- [ ] **Step 4: Add the input to the template**

In `templates/user_settings.html`, inside the preferences `<form>` after the entries-per-page `form-group` (~`:76`):

```html
                    <div class="form-group">
                        <label for="retention-read-days">Delete read articles older than (days)</label>
                        <input type="number" id="retention-read-days" name="retention_read_days" value="{{ retention_read_days }}" min="0" data-testid="retention-read-days" required>
                        <span class="muted" style="font-size:var(--font-xs);">(0 = never delete)</span>
                    </div>
```

- [ ] **Step 5: Build and run the handler/page tests**

Run: `source /tmp/rdrs-env.sh && cargo build -p rdrs && cargo nextest run -p rdrs pages user`
Expected: PASS (Askama compiles the template against the struct; a missing/renamed field fails the build).

- [ ] **Step 6: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs && cargo fmt
git add src/handlers/pages/mod.rs src/handlers/user.rs templates/user_settings.html
git commit -m "feat(settings-ui): retention days field in preferences form"
```

---

### Task 9: BDD scenario for the settings field

**Files:**
- Modify: `e2e/features/preferences.feature` (add a scenario)
- Modify: `e2e/steps/preferences.steps.js` (add two step defs)

The `preferences.feature` `Background` already does `Given I am signed in` + `And I am on the user settings page`, so the scenario reuses it.

- [ ] **Step 1: Add the scenario**

Append to `e2e/features/preferences.feature`:

```gherkin
  Scenario: Setting a read-article retention period persists
    When I set the retention period to "30" days
    Then the retention period field shows "30"
```

- [ ] **Step 2: Add the step definitions**

Append to `e2e/steps/preferences.steps.js` (mirrors the existing `I switch the theme to` step — same form selector + `waitForURL`):

```js
When("I set the retention period to {string} days", async ({ page, serverUrl }, days) => {
  await page.getByTestId("retention-read-days").fill(days);
  await page.locator('form[action="/user-settings/preferences"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
});

Then("the retention period field shows {string}", async ({ page }, value) => {
  await expect(page.getByTestId("retention-read-days")).toHaveValue(value);
});
```

- [ ] **Step 3: Run the BDD suite for this feature**

Run: `cd /home/nixos/Develop/claude/rdrs/e2e && npm test -- preferences`
Expected: all `Preferences` scenarios PASS, including the new one. (The e2e runner builds and launches the app via `global-setup.js`; if it shells out to cargo, ensure `/tmp/rdrs-env.sh` is sourced in the environment first.)

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/features/preferences.feature e2e/steps/preferences.steps.js
git commit -m "test(e2e): retention days settings field round-trips"
```

---

### Task 10: Full verification + docs

**Files:**
- Modify: `README.md` (document the per-user retention setting, if README documents user settings — check first; do not add a new doc file)

- [ ] **Step 1: Full test + lint sweep**

Run:
```
source /tmp/rdrs-env.sh && cargo fmt --check && cargo clippy -p rdrs --all-targets -- -D warnings && cargo nextest run -p rdrs
```
Expected: all green.

- [ ] **Step 2: Manual smoke (optional but recommended)**

Start the app, set retention to a small value in user settings, seed an old read entry (or set one's `read_at` back), wait for / trigger the worker tick, confirm the entry is gone, an `entry_tombstone` row exists, and a refresh that still serves that guid does not re-create it. Confirm the `.sqlite3` file shrinks only when freelist ≥ 20%.

- [ ] **Step 3: Docs**

If `README.md` has a settings/configuration section, add a line describing `retention_read_days` (per-user, `0` = disabled, deletes read non-starred entries older than N days, file space reclaimed via gated VACUUM). If it does not, skip — do not create a new doc file.

- [ ] **Step 4: Commit (if README changed)**

```bash
cd /home/nixos/Develop/claude/rdrs
git add README.md
git commit -m "docs: document per-user read-entry retention setting"
```

---

## Notes for the implementer

- **Atomicity:** every prune batch and the refresh guard rely on single-statement / single-transaction execution on the actor's serialized write connection. Do not split the tombstone-insert and entry-delete across transactions.
- **VACUUM constraints:** `VACUUM` cannot run inside a transaction and needs ~`db_size` free temp disk; it briefly holds an exclusive lock. The 20% gate keeps it rare. `run_maintenance` must therefore be called outside any open transaction (it is — via its own `db.background` closure).
- **Why `upsert_entry` still returns `(Entry, bool)`:** to avoid churning its ~30 (mostly test) call sites. Its tombstone branch errors loudly because no live caller upserts a tombstoned guid.
- **No new index:** benchmarks (in the spec) show `idx_entry_read_at` already serves the victim query at ~0.75 ms/500-row batch over 1M entries; do not add a retention index.
