use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const DEFAULT_ENTRIES_PER_PAGE: i64 = 30;
pub const MIN_ENTRIES_PER_PAGE: i64 = 10;
pub const MAX_ENTRIES_PER_PAGE: i64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub id: i64,
    pub user_id: i64,
    pub entries_per_page: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        .or_else(|_| dateparser::parse(s).map(|dt| dt.with_timezone(&Utc)))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_user_settings(row: &rusqlite::Row) -> rusqlite::Result<UserSettings> {
    let created_at: String = row.get(4)?;
    let updated_at: String = row.get(5)?;

    Ok(UserSettings {
        id: row.get(0)?,
        user_id: row.get(1)?,
        entries_per_page: row.get(2)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

pub fn find_by_user_id(conn: &Connection, user_id: i64) -> AppResult<Option<UserSettings>> {
    conn.query_row(
        "SELECT id, user_id, entries_per_page, entries_per_page, created_at, updated_at FROM user_settings WHERE user_id = ?1",
        params![user_id],
        row_to_user_settings,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn get_entries_per_page(conn: &Connection, user_id: i64) -> AppResult<i64> {
    match find_by_user_id(conn, user_id)? {
        Some(settings) => Ok(settings.entries_per_page),
        None => Ok(DEFAULT_ENTRIES_PER_PAGE),
    }
}

pub fn upsert(conn: &Connection, user_id: i64, entries_per_page: i64) -> AppResult<UserSettings> {
    // Validate range
    if entries_per_page < MIN_ENTRIES_PER_PAGE || entries_per_page > MAX_ENTRIES_PER_PAGE {
        return Err(AppError::Validation(format!(
            "entries_per_page must be between {} and {}",
            MIN_ENTRIES_PER_PAGE, MAX_ENTRIES_PER_PAGE
        )));
    }

    conn.execute(
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO UPDATE SET entries_per_page = ?2, updated_at = datetime('now')",
        params![user_id, entries_per_page],
    )?;

    find_by_user_id(conn, user_id)?.ok_or(AppError::Internal(
        "Failed to retrieve user settings after upsert".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::user::{self, Role};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn test_get_entries_per_page_default() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let entries_per_page = get_entries_per_page(&conn, user.id).unwrap();
        assert_eq!(entries_per_page, DEFAULT_ENTRIES_PER_PAGE);
    }

    #[test]
    fn test_upsert_and_find() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        // Create settings
        let settings = upsert(&conn, user.id, 50).unwrap();
        assert_eq!(settings.user_id, user.id);
        assert_eq!(settings.entries_per_page, 50);

        // Verify
        let found = find_by_user_id(&conn, user.id).unwrap().unwrap();
        assert_eq!(found.entries_per_page, 50);

        // Update settings
        let updated = upsert(&conn, user.id, 75).unwrap();
        assert_eq!(updated.entries_per_page, 75);

        // Verify get_entries_per_page
        let entries_per_page = get_entries_per_page(&conn, user.id).unwrap();
        assert_eq!(entries_per_page, 75);
    }

    #[test]
    fn test_upsert_validation() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        // Too low
        let result = upsert(&conn, user.id, 5);
        assert!(matches!(result, Err(AppError::Validation(_))));

        // Too high
        let result = upsert(&conn, user.id, 150);
        assert!(matches!(result, Err(AppError::Validation(_))));

        // Valid boundaries
        let settings = upsert(&conn, user.id, MIN_ENTRIES_PER_PAGE).unwrap();
        assert_eq!(settings.entries_per_page, MIN_ENTRIES_PER_PAGE);

        let settings = upsert(&conn, user.id, MAX_ENTRIES_PER_PAGE).unwrap();
        assert_eq!(settings.entries_per_page, MAX_ENTRIES_PER_PAGE);
    }
}
