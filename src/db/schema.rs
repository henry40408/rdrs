use rusqlite::Connection;

use crate::error::AppResult;

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

    // Migration: Add save_services column if not exists
    // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we ignore the error
    let _ = conn.execute(
        "ALTER TABLE user_settings ADD COLUMN save_services TEXT",
        [],
    );

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
    }
}
