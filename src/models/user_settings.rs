use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::services::save::SaveServicesConfig;

pub const DEFAULT_ENTRIES_PER_PAGE: i64 = 30;
pub const MIN_ENTRIES_PER_PAGE: i64 = 10;
pub const MAX_ENTRIES_PER_PAGE: i64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub id: i64,
    pub user_id: i64,
    pub entries_per_page: i64,
    pub save_services: Option<String>,
    pub theme: Option<String>, // "dark", "light", or NULL (system)
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UserSettings {
    /// Parse save_services JSON into SaveServicesConfig
    pub fn get_save_services_config(&self) -> SaveServicesConfig {
        self.save_services
            .as_ref()
            .and_then(|json| SaveServicesConfig::from_json(json).ok())
            .unwrap_or_default()
    }

    /// Check if any save service is configured
    pub fn has_save_services(&self) -> bool {
        self.get_save_services_config().has_any_service()
    }
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
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    Ok(UserSettings {
        id: row.get(0)?,
        user_id: row.get(1)?,
        entries_per_page: row.get(2)?,
        save_services: row.get(3)?,
        theme: row.get(4)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

pub fn find_by_user_id(conn: &Connection, user_id: i64) -> AppResult<Option<UserSettings>> {
    conn.query_row(
        "SELECT id, user_id, entries_per_page, save_services, theme, created_at, updated_at FROM user_settings WHERE user_id = ?1",
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
    if !(MIN_ENTRIES_PER_PAGE..=MAX_ENTRIES_PER_PAGE).contains(&entries_per_page) {
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

/// Get SaveServicesConfig for a user
pub fn get_save_services_config(conn: &Connection, user_id: i64) -> AppResult<SaveServicesConfig> {
    match find_by_user_id(conn, user_id)? {
        Some(settings) => Ok(settings.get_save_services_config()),
        None => Ok(SaveServicesConfig::default()),
    }
}

/// Check if user has any save services configured
pub fn has_save_services(conn: &Connection, user_id: i64) -> AppResult<bool> {
    let config = get_save_services_config(conn, user_id)?;
    Ok(config.has_any_service())
}

/// Update save_services configuration for a user
pub fn update_save_services(
    conn: &Connection,
    user_id: i64,
    config: &SaveServicesConfig,
) -> AppResult<UserSettings> {
    let json = config
        .to_json()
        .map_err(|e| AppError::Internal(format!("Failed to serialize save_services: {}", e)))?;

    // First ensure user_settings row exists
    conn.execute(
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO NOTHING",
        params![user_id, DEFAULT_ENTRIES_PER_PAGE],
    )?;

    // Then update save_services
    conn.execute(
        "UPDATE user_settings SET save_services = ?1, updated_at = datetime('now') WHERE user_id = ?2",
        params![json, user_id],
    )?;

    find_by_user_id(conn, user_id)?.ok_or(AppError::Internal(
        "Failed to retrieve user settings after update".to_string(),
    ))
}

/// Get theme preference for a user
pub fn get_theme(conn: &Connection, user_id: i64) -> AppResult<Option<String>> {
    match find_by_user_id(conn, user_id)? {
        Some(settings) => Ok(settings.theme),
        None => Ok(None),
    }
}

/// Update theme preference for a user
pub fn update_theme(conn: &Connection, user_id: i64, theme: Option<String>) -> AppResult<()> {
    // First ensure user_settings row exists
    conn.execute(
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)
         ON CONFLICT(user_id) DO NOTHING",
        params![user_id, DEFAULT_ENTRIES_PER_PAGE],
    )?;

    // Then update theme
    conn.execute(
        "UPDATE user_settings SET theme = ?1, updated_at = datetime('now') WHERE user_id = ?2",
        params![theme, user_id],
    )?;

    Ok(())
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

    #[test]
    fn test_get_theme_default() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        // No settings exist yet, should return None
        let theme = get_theme(&conn, user.id).unwrap();
        assert_eq!(theme, None);
    }

    #[test]
    fn test_update_and_get_theme() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        // Set dark theme
        update_theme(&conn, user.id, Some("dark".to_string())).unwrap();
        let theme = get_theme(&conn, user.id).unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        // Set light theme
        update_theme(&conn, user.id, Some("light".to_string())).unwrap();
        let theme = get_theme(&conn, user.id).unwrap();
        assert_eq!(theme, Some("light".to_string()));

        // Set to system (None)
        update_theme(&conn, user.id, None).unwrap();
        let theme = get_theme(&conn, user.id).unwrap();
        assert_eq!(theme, None);
    }

    #[test]
    fn test_theme_with_existing_settings() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        // Create settings first via upsert
        upsert(&conn, user.id, 50).unwrap();

        // Update theme should work on existing settings
        update_theme(&conn, user.id, Some("dark".to_string())).unwrap();
        let theme = get_theme(&conn, user.id).unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        // Verify entries_per_page is preserved
        let settings = find_by_user_id(&conn, user.id).unwrap().unwrap();
        assert_eq!(settings.entries_per_page, 50);
        assert_eq!(settings.theme, Some("dark".to_string()));
    }
}
