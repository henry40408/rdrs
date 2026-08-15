//! Writing test data straight into the server's SQLite file.
//!
//! A port of `support/seed.js`, which used `better-sqlite3`. Going through the
//! database rather than the UI is what keeps the suite's `Given` steps to one
//! statement each: subscribing to a feed through the app would need a real
//! upstream, a sync pass and a wait, per scenario.
//!
//! It is also why the server runs with `RDRS_DISABLE_SIDEBAR_CACHE=1` — see
//! `server.rs`. Writes made here never run the handlers that bust that cache.

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::time::Duration;

/// One entry to insert.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub feed_id: i64,
    pub guid: String,
    pub title: String,
    pub link: String,
    pub content: String,
    pub summary: Option<String>,
    /// A SQLite modifier applied to `datetime('now', ?)`, e.g. `-2 hours`.
    pub published_offset: String,
}

impl NewEntry {
    /// An entry with the defaults the old helper applied.
    pub fn new(feed_id: i64, guid: &str, title: &str) -> Self {
        Self {
            feed_id,
            guid: guid.to_owned(),
            title: title.to_owned(),
            link: String::new(),
            content: String::new(),
            summary: None,
            published_offset: "0 seconds".to_owned(),
        }
    }

    pub fn link(mut self, link: impl Into<String>) -> Self {
        self.link = link.into();
        self
    }

    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn published_offset(mut self, offset: impl Into<String>) -> Self {
        self.published_offset = offset.into();
        self
    }
}

/// A connection to the server's database.
#[derive(Debug, Clone)]
pub struct Seed {
    pool: SqlitePool,
}

impl Seed {
    /// Opens the pool the scenarios seed through.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be opened.
    pub async fn open(db_path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(db_path)
            // The server has already created and migrated it; a seeder that
            // creates its own would silently write to an empty second file if
            // the path were ever wrong.
            .create_if_missing(false)
            // Scenarios run in parallel against one file, and the server holds
            // the writer as often as they do. Waiting is correct here; failing
            // fast would surface as a flake.
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .with_context(|| format!("opening {}", db_path.display()))?;
        Ok(Self { pool })
    }

    /// Inserts entries and returns their ids, in the order given.
    ///
    /// # Errors
    ///
    /// Fails when a statement is rejected.
    pub async fn insert_entries(&self, entries: &[NewEntry]) -> Result<Vec<i64>> {
        let mut tx = self.pool.begin().await?;
        let mut ids = Vec::with_capacity(entries.len());
        for entry in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO entry (feed_id, guid, title, link, content, summary, published_at)
                 VALUES (?, ?, ?, ?, ?, ?, datetime('now', ?))",
            )
            .bind(entry.feed_id)
            .bind(&entry.guid)
            .bind(&entry.title)
            .bind(&entry.link)
            .bind(&entry.content)
            .bind(entry.summary.as_deref())
            .bind(&entry.published_offset)
            .execute(&mut *tx)
            .await?;

            let id: i64 = sqlx::query_scalar("SELECT id FROM entry WHERE feed_id = ? AND guid = ?")
                .bind(entry.feed_id)
                .bind(&entry.guid)
                .fetch_one(&mut *tx)
                .await
                .with_context(|| format!("no entry with guid `{}` after insert", entry.guid))?;
            ids.push(id);
        }
        tx.commit().await?;
        Ok(ids)
    }

    /// Inserts `count` numbered entries into a feed, newest first.
    ///
    /// # Errors
    ///
    /// Fails when a statement is rejected.
    pub async fn seed_test_entries(&self, feed_id: i64, count: u32) -> Result<Vec<i64>> {
        let entries: Vec<_> = (1..=count)
            .map(|i| {
                NewEntry::new(
                    feed_id,
                    &format!("test-guid-{feed_id}-{i}"),
                    &format!("Test Entry {i}"),
                )
                .link(format!("https://example.com/entry/{i}"))
                .content(format!("<p>Content for test entry {i}</p>"))
                .summary(format!("Summary for entry {i}"))
                .published_offset(format!("-{i} hours"))
            })
            .collect();
        self.insert_entries(&entries).await
    }

    /// # Errors
    ///
    /// Fails when no such user exists.
    pub async fn user_id(&self, username: &str) -> Result<i64> {
        sqlx::query_scalar("SELECT id FROM user WHERE username = ?")
            .bind(username)
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("user `{username}` not found"))
    }

    /// Creates a category if it is not there, returning its id either way.
    ///
    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn create_category(&self, user_id: i64, name: &str) -> Result<i64> {
        sqlx::query("INSERT OR IGNORE INTO category (user_id, name) VALUES (?, ?)")
            .bind(user_id)
            .bind(name)
            .execute(&self.pool)
            .await?;
        self.category_id(user_id, name).await
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn delete_category(&self, user_id: i64, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM category WHERE user_id = ? AND name = ?")
            .bind(user_id)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when no such category exists.
    pub async fn category_id(&self, user_id: i64, name: &str) -> Result<i64> {
        sqlx::query_scalar("SELECT id FROM category WHERE user_id = ? AND name = ?")
            .bind(user_id)
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("category `{name}` not found"))
    }

    /// Creates a feed if its URL is not already taken, returning its id.
    ///
    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn create_feed(&self, category_id: i64, url: &str, title: Option<&str>) -> Result<i64> {
        sqlx::query("INSERT OR IGNORE INTO feed (category_id, url, title) VALUES (?, ?, ?)")
            .bind(category_id)
            .bind(url)
            .bind(title.unwrap_or(url))
            .execute(&self.pool)
            .await?;
        sqlx::query_scalar("SELECT id FROM feed WHERE url = ?")
            .bind(url)
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("feed `{url}` not found after insert"))
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn insert_icon(
        &self,
        feed_id: i64,
        data: &[u8],
        content_type: &str,
        source_url: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO image (entity_type, entity_id, data, content_type, source_url)
             VALUES ('feed', ?, ?, ?, ?)",
        )
        .bind(feed_id)
        .bind(data)
        .bind(content_type)
        .bind(source_url)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn mark_read(&self, entry_id: i64, relative_time: &str) -> Result<()> {
        sqlx::query("UPDATE entry SET read_at = datetime('now', ?) WHERE id = ?")
            .bind(relative_time)
            .bind(entry_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn mark_starred(&self, entry_id: i64, relative_time: &str) -> Result<()> {
        sqlx::query("UPDATE entry SET starred_at = datetime('now', ?) WHERE id = ?")
            .bind(relative_time)
            .bind(entry_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn insert_summary(&self, entry_id: i64, user_id: i64, text: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, summary_text)
             VALUES (?, ?, 'completed', ?)",
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn insert_failed_summary(
        &self,
        entry_id: i64,
        user_id: i64,
        error_message: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, error_message)
             VALUES (?, ?, 'failed', ?)",
        )
        .bind(user_id)
        .bind(entry_id)
        .bind(error_message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn insert_pending_summary(&self, entry_id: i64, user_id: i64) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status)
             VALUES (?, ?, 'pending')",
        )
        .bind(user_id)
        .bind(entry_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn set_entry_content(&self, entry_id: i64, html: &str) -> Result<()> {
        sqlx::query("UPDATE entry SET content = ? WHERE id = ?")
            .bind(html)
            .bind(entry_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn set_entry_link(&self, entry_id: i64, link: &str) -> Result<()> {
        sqlx::query("UPDATE entry SET link = ? WHERE id = ?")
            .bind(link)
            .bind(entry_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Seeds a fake Kagi config so the reading pane renders its Summarize
    /// button.
    ///
    /// The token is bogus, and the mock upstream in `server.rs` is what
    /// actually answers — enough for tests that assert UI state up through the
    /// in-flight placeholder.
    ///
    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn configure_kagi(&self, user_id: i64, session_token: &str) -> Result<()> {
        let payload = serde_json::json!({ "kagi": { "session_token": session_token } }).to_string();
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM user_settings WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        if existing.is_some() {
            sqlx::query("UPDATE user_settings SET save_services = ? WHERE user_id = ?")
                .bind(&payload)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query("INSERT INTO user_settings (user_id, save_services) VALUES (?, ?)")
                .bind(user_id)
                .bind(&payload)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn make_admin(&self, user_id: i64) -> Result<()> {
        sqlx::query("UPDATE user SET role = 'admin' WHERE id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the user has no entry with that title.
    pub async fn entry_id_by_title(&self, user_id: i64, title: &str) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT e.id FROM entry e
             JOIN feed f ON e.feed_id = f.id
             JOIN category c ON f.category_id = c.id
             WHERE c.user_id = ? AND e.title = ?",
        )
        .bind(user_id)
        .bind(title)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("entry `{title}` not found"))
    }

    /// # Errors
    ///
    /// Fails when the user has no feed with that title.
    pub async fn feed_id_by_title(&self, user_id: i64, feed_title: &str) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT f.id FROM feed f
             JOIN category c ON f.category_id = c.id
             WHERE c.user_id = ? AND f.title = ?",
        )
        .bind(user_id)
        .bind(feed_title)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("feed `{feed_title}` not found"))
    }

    /// # Errors
    ///
    /// Fails when the user has no feeds at all.
    pub async fn first_feed_id(&self, user_id: i64) -> Result<i64> {
        sqlx::query_scalar(
            "SELECT f.id FROM feed f
             JOIN category c ON f.category_id = c.id
             WHERE c.user_id = ? LIMIT 1",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .context("no feed found for user")
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn mark_category_read(&self, user_id: i64, category_name: &str) -> Result<()> {
        sqlx::query(
            "UPDATE entry SET read_at = datetime('now')
             WHERE feed_id IN (
               SELECT f.id FROM feed f
               JOIN category c ON f.category_id = c.id
               WHERE c.user_id = ? AND c.name = ?
             )",
        )
        .bind(user_id)
        .bind(category_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Fails when the statement is rejected.
    pub async fn mark_feed_read(&self, user_id: i64, feed_title: &str) -> Result<()> {
        sqlx::query(
            "UPDATE entry SET read_at = datetime('now')
             WHERE feed_id IN (
               SELECT f.id FROM feed f
               JOIN category c ON f.category_id = c.id
               WHERE c.user_id = ? AND f.title = ?
             )",
        )
        .bind(user_id)
        .bind(feed_title)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The titles of every entry a user can see, newest first — a read-side
    /// helper the JS suite did through the API.
    ///
    /// # Errors
    ///
    /// Fails when the query is rejected.
    pub async fn entry_titles(&self, user_id: i64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT e.title FROM entry e
             JOIN feed f ON e.feed_id = f.id
             JOIN category c ON f.category_id = c.id
             WHERE c.user_id = ?
             ORDER BY e.published_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|row| row.get::<String, _>(0)).collect())
    }
}
