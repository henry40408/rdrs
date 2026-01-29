use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::{AppError, AppResult};

/// Summary processing status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl SummaryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SummaryStatus::Pending => "pending",
            SummaryStatus::Processing => "processing",
            SummaryStatus::Completed => "completed",
            SummaryStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(SummaryStatus::Pending),
            "processing" => Some(SummaryStatus::Processing),
            "completed" => Some(SummaryStatus::Completed),
            "failed" => Some(SummaryStatus::Failed),
            _ => None,
        }
    }
}

/// An entry summary stored in the database
#[derive(Debug, Clone, Serialize)]
pub struct EntrySummary {
    pub id: i64,
    pub user_id: i64,
    pub entry_id: i64,
    pub status: SummaryStatus,
    pub summary_text: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_entry_summary(row: &rusqlite::Row) -> rusqlite::Result<EntrySummary> {
    let status_str: String = row.get(3)?;
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;

    Ok(EntrySummary {
        id: row.get(0)?,
        user_id: row.get(1)?,
        entry_id: row.get(2)?,
        status: SummaryStatus::parse(&status_str).unwrap_or(SummaryStatus::Failed),
        summary_text: row.get(4)?,
        error_message: row.get(5)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

const SELECT_COLUMNS: &str =
    "id, user_id, entry_id, status, summary_text, error_message, created_at, updated_at";

/// Find a summary by user and entry
pub fn find_by_user_and_entry(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntrySummary>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM entry_summary WHERE user_id = ?1 AND entry_id = ?2",
            SELECT_COLUMNS
        ),
        params![user_id, entry_id],
        row_to_entry_summary,
    )
    .optional()
    .map_err(AppError::Database)
}

/// Create or update a summary with pending status
pub fn upsert_pending(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<EntrySummary> {
    conn.execute(
        r#"
        INSERT INTO entry_summary (user_id, entry_id, status)
        VALUES (?1, ?2, 'pending')
        ON CONFLICT(user_id, entry_id) DO UPDATE SET
            status = 'pending',
            summary_text = NULL,
            error_message = NULL,
            updated_at = datetime('now')
        "#,
        params![user_id, entry_id],
    )?;

    find_by_user_and_entry(conn, user_id, entry_id)?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

/// Update status to processing
pub fn set_processing(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<()> {
    let rows = conn.execute(
        r#"
        UPDATE entry_summary
        SET status = 'processing', updated_at = datetime('now')
        WHERE user_id = ?1 AND entry_id = ?2
        "#,
        params![user_id, entry_id],
    )?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    Ok(())
}

/// Set summary as completed with the summary text
pub fn set_completed(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    summary_text: &str,
) -> AppResult<EntrySummary> {
    let rows = conn.execute(
        r#"
        UPDATE entry_summary
        SET status = 'completed', summary_text = ?3, error_message = NULL, updated_at = datetime('now')
        WHERE user_id = ?1 AND entry_id = ?2
        "#,
        params![user_id, entry_id, summary_text],
    )?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    find_by_user_and_entry(conn, user_id, entry_id)?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

/// Set summary as failed with error message
pub fn set_failed(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    error_message: &str,
) -> AppResult<EntrySummary> {
    let rows = conn.execute(
        r#"
        UPDATE entry_summary
        SET status = 'failed', error_message = ?3, updated_at = datetime('now')
        WHERE user_id = ?1 AND entry_id = ?2
        "#,
        params![user_id, entry_id, error_message],
    )?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    find_by_user_and_entry(conn, user_id, entry_id)?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

/// Delete a summary
pub fn delete(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let rows = conn.execute(
        "DELETE FROM entry_summary WHERE user_id = ?1 AND entry_id = ?2",
        params![user_id, entry_id],
    )?;

    Ok(rows > 0)
}

/// Check if an entry has a completed summary
pub fn has_completed_summary(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entry_summary WHERE user_id = ?1 AND entry_id = ?2 AND status = 'completed'",
        params![user_id, entry_id],
        |row| row.get(0),
    )?;

    Ok(count > 0)
}

/// Get summary statuses for multiple entries (batch query for list display)
pub fn get_statuses_for_entries(
    conn: &Connection,
    user_id: i64,
    entry_ids: &[i64],
) -> AppResult<HashMap<i64, SummaryStatus>> {
    if entry_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> = entry_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT entry_id, status FROM entry_summary WHERE user_id = ?1 AND entry_id IN ({})",
        in_clause
    );

    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
    for id in entry_ids {
        params_vec.push(Box::new(*id));
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let entry_id: i64 = row.get(0)?;
        let status_str: String = row.get(1)?;
        Ok((entry_id, status_str))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (entry_id, status_str) = row?;
        if let Some(status) = SummaryStatus::parse(&status_str) {
            map.insert(entry_id, status);
        }
    }

    Ok(map)
}

/// Get entry IDs that have completed summaries
pub fn get_completed_entry_ids(conn: &Connection, user_id: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT entry_id FROM entry_summary WHERE user_id = ?1 AND status = 'completed'",
    )?;

    let ids = stmt
        .query_map(params![user_id], |row| row.get(0))?
        .filter_map(Result::ok)
        .collect();

    Ok(ids)
}

/// Find incomplete summaries (pending or processing) for recovery on startup
/// Returns (user_id, entry_id, entry_link) tuples
pub fn find_incomplete(conn: &Connection) -> AppResult<Vec<(i64, i64, String)>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT es.user_id, es.entry_id, e.link
        FROM entry_summary es
        INNER JOIN entry e ON es.entry_id = e.id
        WHERE es.status IN ('pending', 'processing') AND e.link IS NOT NULL
        "#,
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .filter_map(Result::ok)
        .collect();

    Ok(rows)
}

/// Delete expired summaries (older than specified hours)
pub fn delete_expired(conn: &Connection, hours: i64) -> AppResult<usize> {
    let rows = conn.execute(
        &format!(
            "DELETE FROM entry_summary WHERE created_at < datetime('now', '-{} hours')",
            hours
        ),
        [],
    )?;

    Ok(rows)
}

/// Check if a summary record exists (any status)
pub fn exists(conn: &Connection, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entry_summary WHERE user_id = ?1 AND entry_id = ?2",
        params![user_id, entry_id],
        |row| row.get(0),
    )?;

    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::user::Role;
    use crate::models::{category, entry, feed, user};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn create_test_user(conn: &Connection, username: &str) -> i64 {
        user::create_user(conn, username, "hash123", Role::User)
            .unwrap()
            .id
    }

    fn create_test_entry(conn: &Connection, user_id: i64) -> i64 {
        let category_id = category::create_category(conn, user_id, "Tech").unwrap().id;
        let feed_id = feed::create_feed(
            conn,
            category_id,
            "https://example.com/feed.xml",
            Some("Test Feed"),
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id;

        let (entry, _) = entry::upsert_entry(
            conn,
            feed_id,
            "guid-123",
            Some("Test Entry"),
            Some("https://example.com/entry"),
            Some("Content"),
            None,
            None,
            None,
        )
        .unwrap();

        entry.id
    }

    #[test]
    fn test_upsert_pending() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        let summary = upsert_pending(&conn, user_id, entry_id).unwrap();
        assert_eq!(summary.status, SummaryStatus::Pending);
        assert!(summary.summary_text.is_none());
        assert!(summary.error_message.is_none());
    }

    #[test]
    fn test_status_transitions() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        // Create pending
        upsert_pending(&conn, user_id, entry_id).unwrap();

        // Set processing
        set_processing(&conn, user_id, entry_id).unwrap();
        let summary = find_by_user_and_entry(&conn, user_id, entry_id)
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Processing);

        // Set completed
        set_completed(&conn, user_id, entry_id, "This is the summary").unwrap();
        let summary = find_by_user_and_entry(&conn, user_id, entry_id)
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Completed);
        assert_eq!(summary.summary_text.as_deref(), Some("This is the summary"));
    }

    #[test]
    fn test_set_failed() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        upsert_pending(&conn, user_id, entry_id).unwrap();
        set_failed(&conn, user_id, entry_id, "API error").unwrap();

        let summary = find_by_user_and_entry(&conn, user_id, entry_id)
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Failed);
        assert_eq!(summary.error_message.as_deref(), Some("API error"));
    }

    #[test]
    fn test_delete() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        upsert_pending(&conn, user_id, entry_id).unwrap();
        assert!(exists(&conn, user_id, entry_id).unwrap());

        let deleted = delete(&conn, user_id, entry_id).unwrap();
        assert!(deleted);
        assert!(!exists(&conn, user_id, entry_id).unwrap());
    }

    #[test]
    fn test_has_completed_summary() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        assert!(!has_completed_summary(&conn, user_id, entry_id).unwrap());

        upsert_pending(&conn, user_id, entry_id).unwrap();
        assert!(!has_completed_summary(&conn, user_id, entry_id).unwrap());

        set_completed(&conn, user_id, entry_id, "Summary text").unwrap();
        assert!(has_completed_summary(&conn, user_id, entry_id).unwrap());
    }

    #[test]
    fn test_get_statuses_for_entries() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        // Create multiple entries
        let category_id = category::create_category(&conn, user_id, "Tech")
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            Some("Test Feed"),
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id;

        let mut entry_ids = Vec::new();
        for i in 0..3 {
            let (e, _) = entry::upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some(&format!("Entry {}", i)),
                Some("https://example.com/entry"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            entry_ids.push(e.id);
        }

        // Set different statuses
        upsert_pending(&conn, user_id, entry_ids[0]).unwrap();
        upsert_pending(&conn, user_id, entry_ids[1]).unwrap();
        set_completed(&conn, user_id, entry_ids[1], "Summary").unwrap();
        // entry_ids[2] has no summary

        let statuses = get_statuses_for_entries(&conn, user_id, &entry_ids).unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses.get(&entry_ids[0]), Some(&SummaryStatus::Pending));
        assert_eq!(statuses.get(&entry_ids[1]), Some(&SummaryStatus::Completed));
        assert_eq!(statuses.get(&entry_ids[2]), None);
    }

    #[test]
    fn test_find_incomplete() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        upsert_pending(&conn, user_id, entry_id).unwrap();

        let incomplete = find_incomplete(&conn).unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].0, user_id);
        assert_eq!(incomplete[0].1, entry_id);
    }

    #[test]
    fn test_upsert_resets_to_pending() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let entry_id = create_test_entry(&conn, user_id);

        // Create and complete
        upsert_pending(&conn, user_id, entry_id).unwrap();
        set_completed(&conn, user_id, entry_id, "Summary").unwrap();

        // Upsert should reset to pending
        let summary = upsert_pending(&conn, user_id, entry_id).unwrap();
        assert_eq!(summary.status, SummaryStatus::Pending);
        assert!(summary.summary_text.is_none());
    }

    #[test]
    fn test_status_string_conversion() {
        assert_eq!(SummaryStatus::Pending.as_str(), "pending");
        assert_eq!(SummaryStatus::Processing.as_str(), "processing");
        assert_eq!(SummaryStatus::Completed.as_str(), "completed");
        assert_eq!(SummaryStatus::Failed.as_str(), "failed");

        assert_eq!(
            SummaryStatus::parse("pending"),
            Some(SummaryStatus::Pending)
        );
        assert_eq!(
            SummaryStatus::parse("processing"),
            Some(SummaryStatus::Processing)
        );
        assert_eq!(
            SummaryStatus::parse("completed"),
            Some(SummaryStatus::Completed)
        );
        assert_eq!(SummaryStatus::parse("failed"), Some(SummaryStatus::Failed));
        assert_eq!(SummaryStatus::parse("invalid"), None);
    }
}
