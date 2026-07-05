use rusqlite::Connection;

use crate::error::AppResult;
use crate::models::feed::url_to_bucket;
use crate::utils::text::strip_to_search_text;

pub fn init_db(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS user (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
            disabled_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS session (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
            session_token TEXT NOT NULL UNIQUE,
            original_user_id INTEGER REFERENCES user(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_session_token ON session(session_token);
        CREATE INDEX IF NOT EXISTS idx_session_user_id ON session(user_id);
        CREATE INDEX IF NOT EXISTS idx_session_expires_at ON session(expires_at);

        CREATE TABLE IF NOT EXISTS category (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, name)
        );

        CREATE INDEX IF NOT EXISTS idx_category_user_id ON category(user_id);

        CREATE TABLE IF NOT EXISTS feed (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
            url TEXT NOT NULL,
            title TEXT,
            description TEXT,
            site_url TEXT,
            feed_updated_at TEXT,
            fetched_at TEXT,
            fetch_error TEXT,
            etag TEXT,
            last_modified TEXT,
            custom_user_agent TEXT,
            http2_disabled INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(category_id, url)
        );

        CREATE INDEX IF NOT EXISTS idx_feed_category_id ON feed(category_id);

        CREATE TABLE IF NOT EXISTS entry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            feed_id INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
            guid TEXT NOT NULL,
            title TEXT,
            link TEXT,
            content TEXT,
            summary TEXT,
            author TEXT,
            published_at TEXT,
            read_at TEXT,
            starred_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(feed_id, guid)
        );

        CREATE INDEX IF NOT EXISTS idx_entry_feed_id ON entry(feed_id);
        CREATE INDEX IF NOT EXISTS idx_entry_published_at ON entry(published_at);
        CREATE INDEX IF NOT EXISTS idx_entry_read_at ON entry(read_at);
        CREATE INDEX IF NOT EXISTS idx_entry_starred_at ON entry(starred_at);
        CREATE INDEX IF NOT EXISTS idx_entry_sort_ts ON entry(COALESCE(published_at, created_at));
        CREATE INDEX IF NOT EXISTS idx_entry_created_at ON entry(created_at);
        -- Partial indexes for the Starred / Read list pages. The list-by-user
        -- query orders by COALESCE(published_at, created_at) DESC with the
        -- selectivity predicate baked in; without these the planner falls back
        -- to a category->feed->entry walk over every row. See migration v5.
        CREATE INDEX IF NOT EXISTS idx_entry_starred_sort
            ON entry(COALESCE(published_at, created_at))
            WHERE starred_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_entry_read_sort
            ON entry(COALESCE(published_at, created_at))
            WHERE read_at IS NOT NULL;
        -- Partial index over only the unread rows, keyed by feed_id. The
        -- sidebar (per-category unread) and the feeds page (per-feed unread)
        -- otherwise walk `idx_entry_feed_id` over every entry and filter
        -- read_at after the fact; this index touches only the unread subset.
        -- The count queries pin it with `INDEXED BY` because, post-ANALYZE,
        -- the planner can otherwise flip to a far worse `idx_entry_read_at`
        -- plan. See migration v6.
        CREATE INDEX IF NOT EXISTS idx_entry_unread_feed
            ON entry(feed_id)
            WHERE read_at IS NULL;
        -- Composite index for the per-feed / per-category list pages, which
        -- filter by feed_id and ORDER BY COALESCE(published_at, created_at)
        -- DESC. Without it the planner uses idx_entry_feed_id to filter then
        -- builds a temp B-tree to sort; this index serves the filter and the
        -- order together as a range scan. See migration v7.
        CREATE INDEX IF NOT EXISTS idx_entry_feed_sort
            ON entry(feed_id, COALESCE(published_at, created_at));
        -- Ordered partial index over unread entries for the unread list page,
        -- mirroring idx_entry_read_sort. Without it the unread list does a
        -- whole-inbox temp-B-tree sort (~143ms at 1M entries). Used via an
        -- INDEXED BY hint in published_sort_entry_hint (a later task).
        CREATE INDEX IF NOT EXISTS idx_entry_unread_sort
            ON entry(COALESCE(published_at, created_at))
            WHERE read_at IS NULL;

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

        CREATE TABLE IF NOT EXISTS entry_summary (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
            entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
            status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
            summary_text TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, entry_id)
        );

        CREATE INDEX IF NOT EXISTS idx_entry_summary_user_entry ON entry_summary(user_id, entry_id);
        CREATE INDEX IF NOT EXISTS idx_entry_summary_user_status ON entry_summary(user_id, status);
        CREATE INDEX IF NOT EXISTS idx_entry_summary_entry_id ON entry_summary(entry_id);

        CREATE TABLE IF NOT EXISTS image (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entity_type TEXT NOT NULL,
            entity_id INTEGER NOT NULL,
            data BLOB NOT NULL,
            content_type TEXT NOT NULL,
            source_url TEXT,
            fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(entity_type, entity_id)
        );

        CREATE INDEX IF NOT EXISTS idx_image_entity ON image(entity_type, entity_id);

        CREATE TABLE IF NOT EXISTS user_settings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL UNIQUE REFERENCES user(id) ON DELETE CASCADE,
            entries_per_page INTEGER NOT NULL DEFAULT 30,
            save_services TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_user_settings_user_id ON user_settings(user_id);

        CREATE TABLE IF NOT EXISTS passkey (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES user(id) ON DELETE CASCADE,
            credential_id BLOB NOT NULL UNIQUE,
            public_key BLOB NOT NULL,
            counter INTEGER NOT NULL DEFAULT 0,
            name TEXT NOT NULL,
            transports TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_passkey_user_id ON passkey(user_id);
        CREATE INDEX IF NOT EXISTS idx_passkey_credential_id ON passkey(credential_id);

        CREATE TABLE IF NOT EXISTS webauthn_challenge (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            challenge BLOB NOT NULL UNIQUE,
            user_id INTEGER REFERENCES user(id) ON DELETE CASCADE,
            challenge_type TEXT NOT NULL CHECK (challenge_type IN ('registration', 'authentication')),
            state_data TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_webauthn_challenge_expires_at ON webauthn_challenge(expires_at);
        "#,
    )?;

    // Version-based migrations using PRAGMA user_version
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    // Migrations 1-3: Legacy migrations that may already exist in older databases.
    // Use `let _ =` to ignore "duplicate column" errors for databases that already
    // had these columns added before the user_version system was introduced.
    if version < 1 {
        let _ = conn.execute(
            "ALTER TABLE user_settings ADD COLUMN save_services TEXT",
            [],
        );
    }
    if version < 2 {
        let _ = conn.execute("ALTER TABLE user_settings ADD COLUMN theme TEXT", []);
    }
    if version < 3 {
        let _ = conn.execute("ALTER TABLE feed ADD COLUMN custom_referrer TEXT", []);
    }
    if version < 4 {
        conn.execute("ALTER TABLE feed ADD COLUMN bucket INTEGER", [])?;
        // Backfill bucket values for existing feeds
        let mut stmt = conn.prepare("SELECT id, url FROM feed")?;
        let feeds: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(Result::ok)
            .collect();
        for (id, url) in &feeds {
            let bucket = url_to_bucket(url) as i64;
            conn.execute(
                "UPDATE feed SET bucket = ?1 WHERE id = ?2",
                rusqlite::params![bucket, id],
            )?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_feed_bucket ON feed(bucket)",
            [],
        )?;
    }

    if version < 5 {
        // Partial indexes for the Starred / Read list pages. Defined alongside
        // the other entry indexes in the main `execute_batch` block above using
        // `CREATE INDEX IF NOT EXISTS`, so existing databases pick them up on
        // restart without a dedicated migration step. The version bump exists
        // so future migrations can rely on these indexes being present.
    }

    if version < 6 {
        // Partial index over unread entries (`idx_entry_unread_feed`). Like the
        // v5 indexes it is created via `CREATE INDEX IF NOT EXISTS` in the main
        // batch above, so existing databases pick it up on restart. The version
        // bump records that the unread-count queries can rely on it.
    }

    if version < 7 {
        // Composite index `idx_entry_feed_sort` for the per-feed/per-category
        // list pages. Like the v5/v6 indexes it is created via
        // `CREATE INDEX IF NOT EXISTS` in the main batch above, so existing
        // databases pick it up on restart. The version bump records that the
        // per-feed list query can rely on it.
    }

    if version < 8 {
        // The entry_tombstone table and idx_entry_unread_sort index are created
        // via CREATE ... IF NOT EXISTS in the main batch above (picked up on
        // restart, like the v5/v6/v7 indexes). retention_read_days is added here
        // only (mirrors how v4 adds `bucket`): the block runs exactly once per DB
        // when version < 8, so the column is never already present — no swallow.
        conn.execute(
            "ALTER TABLE user_settings ADD COLUMN retention_read_days INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    if version < 9 {
        // idx_entry_created_at is created via CREATE INDEX IF NOT EXISTS in the
        // main batch above (picked up on restart, like the v5/v6/v7 indexes).
        // The version bump records that MIN/MAX(created_at) admin stats can rely
        // on the index endpoint optimization being available.
    }

    if version < 10 {
        // Guard the ALTER with a column-existence check instead of just
        // running it: the backfill below can take minutes on a large entry
        // table, and if the process is killed mid-backfill (SIGINT/OOM/host
        // restart), `user_version` is still 9, so this block re-enters on
        // next boot. Without the guard, re-running the bare ALTER would fail
        // with "duplicate column name: content_text" and panic at init_db's
        // `.expect(...)`, leaving the app unable to boot. The backfill loop
        // itself is already re-entrant (`WHERE content_text IS NULL`).
        let has_content_text: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('entry') WHERE name = 'content_text')",
            [],
            |row| row.get(0),
        )?;
        if !has_content_text {
            conn.execute("ALTER TABLE entry ADD COLUMN content_text TEXT", [])?;
        }
        // Count the rows that still need backfilling up front so the progress
        // logs below can show "N/TOTAL". On a large entry table this backfill
        // takes minutes and blocks startup; without visible progress an
        // operator sees only a silent, slow boot and cannot tell the process
        // from a hang. The count reflects rows remaining, so a re-entered
        // migration (see the guard above) reports what is actually left.
        let total_to_backfill: i64 = conn.query_row(
            "SELECT COUNT(*) FROM entry WHERE content_text IS NULL AND content IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        if total_to_backfill > 0 {
            tracing::info!(
                "Migration v10: backfilling content_text for {} entries. \
                 Startup is blocked until this completes; this can take several \
                 minutes on a large database.",
                total_to_backfill
            );
        }
        // Backfill plain-text search content in batches so a large entry
        // table doesn't build one giant transaction. Rows with NULL content
        // stay NULL (nothing to search). strip_to_search_text joins across
        // tags so terms split by inline markup remain matchable.
        let mut backfilled: i64 = 0;
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
            backfilled += batch.len() as i64;
            tracing::info!(
                "Migration v10: content_text backfill progress {}/{} entries",
                backfilled,
                total_to_backfill
            );
        }
        if total_to_backfill > 0 {
            tracing::info!(
                "Migration v10: content_text backfill complete ({} entries)",
                backfilled
            );
        }
    }

    const LATEST_VERSION: i64 = 10;
    if version < LATEST_VERSION {
        conn.pragma_update(None, "user_version", LATEST_VERSION)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_db() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        assert!(tables.contains(&"user".to_string()));
        assert!(tables.contains(&"session".to_string()));
        assert!(tables.contains(&"passkey".to_string()));
        assert!(tables.contains(&"webauthn_challenge".to_string()));
        assert!(tables.contains(&"entry_summary".to_string()));
        assert!(tables.contains(&"entry_tombstone".to_string()));
    }

    #[test]
    fn test_init_db_idempotent() {
        // Running init_db twice should succeed (all CREATE IF NOT EXISTS)
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        init_db(&conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
    }

    #[test]
    fn test_init_db_sets_user_version() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);
    }

    #[test]
    fn test_init_db_feed_has_bucket_column() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Verify bucket column exists by inserting a row with it
        conn.execute(
            "INSERT INTO user (username, password_hash) VALUES ('test', 'hash')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (1, 'Test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feed (category_id, url, bucket) VALUES (1, 'https://example.com/feed', 42)",
            [],
        )
        .unwrap();

        let bucket: i64 = conn
            .query_row("SELECT bucket FROM feed WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(bucket, 42);
    }

    #[test]
    fn test_init_db_feed_has_custom_referrer_column() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        conn.execute(
            "INSERT INTO user (username, password_hash) VALUES ('test', 'hash')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (1, 'Test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feed (category_id, url, custom_referrer) VALUES (1, 'https://example.com/feed', 'https://ref.example.com')",
            [],
        )
        .unwrap();

        let referrer: Option<String> = conn
            .query_row("SELECT custom_referrer FROM feed WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(referrer, Some("https://ref.example.com".to_string()));
    }

    #[test]
    fn test_init_db_bucket_index_exists() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name='idx_feed_bucket'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(indexes.len(), 1);
    }

    #[test]
    fn test_init_db_entry_partial_indexes_exist() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' \
                 AND name IN ('idx_entry_starred_sort', 'idx_entry_read_sort') \
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            indexes,
            vec![
                "idx_entry_read_sort".to_string(),
                "idx_entry_starred_sort".to_string(),
            ]
        );
    }

    #[test]
    fn test_init_db_unread_feed_index_exists() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND name='idx_entry_unread_feed'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(indexes, vec!["idx_entry_unread_feed".to_string()]);
    }

    #[test]
    fn test_init_db_feed_sort_index_exists() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND name='idx_entry_feed_sort'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(indexes, vec!["idx_entry_feed_sort".to_string()]);
    }

    #[test]
    fn test_init_db_user_settings_has_retention_read_days_column() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Verify retention_read_days column exists and defaults to 0
        conn.execute(
            "INSERT INTO user (username, password_hash) VALUES ('test', 'hash')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO user_settings (user_id) VALUES (1)", [])
            .unwrap();

        let retention_read_days: i64 = conn
            .query_row(
                "SELECT retention_read_days FROM user_settings WHERE user_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retention_read_days, 0);
    }

    #[test]
    fn test_init_db_unread_sort_index_exists() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND name='idx_entry_unread_sort'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(indexes, vec!["idx_entry_unread_sort".to_string()]);
    }

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
             INSERT INTO entry (id, feed_id, guid, content) VALUES (1, 1, 'g', '超<b>少女</b>');",
        )
        .unwrap();
        // content_text already exists from the init_db call above (a fresh
        // in-memory DB runs the v10 block immediately); drop it so the
        // re-run below genuinely simulates a pre-v10 database.
        conn.execute("ALTER TABLE entry DROP COLUMN content_text", [])
            .unwrap();
        conn.pragma_update(None, "user_version", 9i64).unwrap();
        init_db(&conn).unwrap();
        let text: Option<String> = conn
            .query_row("SELECT content_text FROM entry WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(text.as_deref(), Some("超少女"));
    }

    #[test]
    fn test_v10_migration_is_crash_re_entrant() {
        // Simulate a process killed mid-backfill: user_version never got
        // bumped past 9, but content_text (and its ALTER) already landed on
        // disk from the interrupted attempt. Re-running init_db must not
        // panic/error on "duplicate column name: content_text", and the
        // already-backfilled data must survive untouched.
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO user (id, username, password_hash) VALUES (1, 'u', 'x');
             INSERT INTO category (id, user_id, name) VALUES (1, 1, 'c');
             INSERT INTO feed (id, category_id, url) VALUES (1, 1, 'http://x');
             INSERT INTO entry (id, feed_id, guid, content, content_text) \
                VALUES (1, 1, 'g', '<b>hello</b>', 'hello');",
        )
        .unwrap();
        // Force user_version back to 9 WITHOUT dropping content_text — this
        // is the exact re-entry state after an interrupted v10 migration
        // (ALTER succeeded, backfill may have partially run, but the crash
        // happened before the version bump at the end of init_db).
        conn.pragma_update(None, "user_version", 9i64).unwrap();

        // Must not panic or return an error (e.g. duplicate column name).
        init_db(&conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 10);

        let text: Option<String> = conn
            .query_row("SELECT content_text FROM entry WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            text.as_deref(),
            Some("hello"),
            "pre-existing content_text must survive a re-entered migration"
        );
    }
}
