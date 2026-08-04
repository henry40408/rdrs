use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::save::SaveServicesConfig;
use crate::{db_execute, query_opt};

pub const DEFAULT_ENTRIES_PER_PAGE: i64 = 30;
pub const MIN_ENTRIES_PER_PAGE: i64 = 10;
pub const MAX_ENTRIES_PER_PAGE: i64 = 100;

/// Upper bound for the per-user read-entry retention threshold, in days
/// (~10 years). Guards against absurd inputs; values this large already mean
/// "effectively never delete", which `0` expresses directly.
pub const MAX_RETENTION_READ_DAYS: i64 = 3650;

/// Sidebar ordering: categories (and the open category's feeds) A-Z by name.
/// The order the list queries already return, so the client leaves them alone.
pub const SIDEBAR_SORT_NAME: &str = "name";
/// Sidebar ordering: most unread first, ties keeping their A-Z order.
pub const SIDEBAR_SORT_UNREAD: &str = "unread";
pub const DEFAULT_SIDEBAR_SORT: &str = SIDEBAR_SORT_NAME;

/// Sidebar display preferences, the pair that decides what the category and
/// feed lists look like. Read together because both reach the client in the
/// same `/api/sidebar` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarPrefs {
    pub sort: &'static str,
    pub hide_read: bool,
}

impl Default for SidebarPrefs {
    fn default() -> Self {
        Self {
            sort: DEFAULT_SIDEBAR_SORT,
            hide_read: false,
        }
    }
}

/// Map a stored/submitted sort value onto one of the known orderings.
/// Unknown values (an older client, a hand-edited row) fall back to the
/// default rather than erroring — this is a display preference, and a
/// mis-ordered sidebar must not be able to break a page render.
pub fn parse_sidebar_sort(value: &str) -> &'static str {
    match value {
        SIDEBAR_SORT_UNREAD => SIDEBAR_SORT_UNREAD,
        _ => DEFAULT_SIDEBAR_SORT,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserSettings {
    pub id: i64,
    pub user_id: i64,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
    pub save_services: Option<String>,
    pub theme: Option<String>, // "dark", "light", or NULL (system)
    pub sidebar_sort: String,  // "name" or "unread"
    pub sidebar_hide_read: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UserSettings {
    /// Parse `save_services` JSON into `SaveServicesConfig`
    pub fn get_save_services_config(&self) -> SaveServicesConfig {
        self.save_services
            .as_ref()
            .and_then(|json| SaveServicesConfig::from_json(json).ok())
            .unwrap_or_default()
    }
}

pub async fn find_by_user_id(db: &Db, user_id: i64) -> AppResult<Option<UserSettings>> {
    query_opt!(
        db,
        UserSettings,
        "SELECT id, user_id, entries_per_page, retention_read_days, save_services, theme, sidebar_sort, sidebar_hide_read, created_at, updated_at FROM user_settings WHERE user_id = $1",
        user_id
    )
    .map_err(AppError::Database)
}

pub async fn get_entries_per_page(db: &Db, user_id: i64) -> AppResult<i64> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.entries_per_page),
        None => Ok(DEFAULT_ENTRIES_PER_PAGE),
    }
}

pub async fn upsert(db: &Db, user_id: i64, entries_per_page: i64) -> AppResult<UserSettings> {
    // Validate range
    if !(MIN_ENTRIES_PER_PAGE..=MAX_ENTRIES_PER_PAGE).contains(&entries_per_page) {
        return Err(AppError::Validation(format!(
            "entries_per_page must be between {MIN_ENTRIES_PER_PAGE} and {MAX_ENTRIES_PER_PAGE}"
        )));
    }

    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO UPDATE SET entries_per_page = $2, updated_at = $3",
        user_id,
        entries_per_page,
        Utc::now()
    )
    .map_err(AppError::Database)?;

    find_by_user_id(db, user_id)
        .await?
        .ok_or(AppError::Internal(
            "Failed to retrieve user settings after upsert".to_string(),
        ))
}

/// Get `SaveServicesConfig` for a user
pub async fn get_save_services_config(db: &Db, user_id: i64) -> AppResult<SaveServicesConfig> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.get_save_services_config()),
        None => Ok(SaveServicesConfig::default()),
    }
}

/// Update `save_services` configuration for a user
pub async fn update_save_services(
    db: &Db,
    user_id: i64,
    config: &SaveServicesConfig,
) -> AppResult<UserSettings> {
    let json = config
        .to_json()
        .map_err(|e| AppError::Internal(format!("Failed to serialize save_services: {e}")))?;

    // First ensure user_settings row exists
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;

    // Then update save_services
    db_execute!(
        db,
        "UPDATE user_settings SET save_services = $1, updated_at = $2 WHERE user_id = $3",
        &json,
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;

    find_by_user_id(db, user_id)
        .await?
        .ok_or(AppError::Internal(
            "Failed to retrieve user settings after update".to_string(),
        ))
}

/// Get theme preference for a user
pub async fn get_theme(db: &Db, user_id: i64) -> AppResult<Option<String>> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.theme),
        None => Ok(None),
    }
}

/// Update theme preference for a user
pub async fn update_theme(db: &Db, user_id: i64, theme: Option<String>) -> AppResult<()> {
    // First ensure user_settings row exists
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;

    // Then update theme
    db_execute!(
        db,
        "UPDATE user_settings SET theme = $1, updated_at = $2 WHERE user_id = $3",
        theme.as_deref(),
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;

    Ok(())
}

/// Get the per-user read-entry retention threshold in days (0 = disabled).
pub async fn get_retention_read_days(db: &Db, user_id: i64) -> AppResult<i64> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.retention_read_days),
        None => Ok(0),
    }
}

/// Set the per-user read-entry retention threshold in days. `0` disables
/// retention for the user; values outside `0..=MAX_RETENTION_READ_DAYS` are
/// rejected.
pub async fn update_retention_read_days(db: &Db, user_id: i64, days: i64) -> AppResult<()> {
    if !(0..=MAX_RETENTION_READ_DAYS).contains(&days) {
        return Err(AppError::Validation(format!(
            "retention_read_days must be between 0 and {MAX_RETENTION_READ_DAYS}"
        )));
    }
    // Ensure a row exists, then update (mirrors update_theme).
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;
    db_execute!(
        db,
        "UPDATE user_settings SET retention_read_days = $1, updated_at = $2 WHERE user_id = $3",
        days,
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Get the sidebar display preferences for a user. Accounts with no
/// `user_settings` row yet get the defaults.
pub async fn get_sidebar_prefs(db: &Db, user_id: i64) -> AppResult<SidebarPrefs> {
    Ok(find_by_user_id(db, user_id)
        .await?
        .as_ref()
        .map_or_else(SidebarPrefs::default, sidebar_prefs_of))
}

/// Read the sidebar preferences out of an already-loaded settings row, so
/// callers that need several fields (e.g. `read_chrome_data`, which also wants
/// the theme) pay for one query instead of one per field.
pub fn sidebar_prefs_of(settings: &UserSettings) -> SidebarPrefs {
    SidebarPrefs {
        sort: parse_sidebar_sort(&settings.sidebar_sort),
        hide_read: settings.sidebar_hide_read,
    }
}

/// Set the sidebar display preferences. `sort` is normalised rather than
/// rejected — see `parse_sidebar_sort`.
pub async fn update_sidebar_prefs(
    db: &Db,
    user_id: i64,
    sort: &str,
    hide_read: bool,
) -> AppResult<()> {
    let sort = parse_sidebar_sort(sort);
    // Ensure a row exists, then update (mirrors update_theme).
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;
    db_execute!(
        db,
        "UPDATE user_settings SET sidebar_sort = $1, sidebar_hide_read = $2, updated_at = $3 WHERE user_id = $4",
        sort,
        hide_read,
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_get_entries_per_page_default() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let entries_per_page = get_entries_per_page(&db, user.id).await.unwrap();
        assert_eq!(entries_per_page, DEFAULT_ENTRIES_PER_PAGE);
    }

    #[tokio::test]
    async fn test_upsert_and_find() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // Create settings
        let settings = upsert(&db, user.id, 50).await.unwrap();
        assert_eq!(settings.user_id, user.id);
        assert_eq!(settings.entries_per_page, 50);

        // Verify
        let found = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(found.entries_per_page, 50);

        // Update settings
        let updated = upsert(&db, user.id, 75).await.unwrap();
        assert_eq!(updated.entries_per_page, 75);

        // Verify get_entries_per_page
        let entries_per_page = get_entries_per_page(&db, user.id).await.unwrap();
        assert_eq!(entries_per_page, 75);
    }

    #[tokio::test]
    async fn test_upsert_validation() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // Too low
        let result = upsert(&db, user.id, 5).await;
        assert!(matches!(result, Err(AppError::Validation(_))));

        // Too high
        let result = upsert(&db, user.id, 150).await;
        assert!(matches!(result, Err(AppError::Validation(_))));

        // Valid boundaries
        let settings = upsert(&db, user.id, MIN_ENTRIES_PER_PAGE).await.unwrap();
        assert_eq!(settings.entries_per_page, MIN_ENTRIES_PER_PAGE);

        let settings = upsert(&db, user.id, MAX_ENTRIES_PER_PAGE).await.unwrap();
        assert_eq!(settings.entries_per_page, MAX_ENTRIES_PER_PAGE);
    }

    #[tokio::test]
    async fn test_get_theme_default() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // No settings exist yet, should return None
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, None);
    }

    #[tokio::test]
    async fn test_update_and_get_theme() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // Set dark theme
        update_theme(&db, user.id, Some("dark".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        // Set light theme
        update_theme(&db, user.id, Some("light".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("light".to_string()));

        // Set to system (None)
        update_theme(&db, user.id, None).await.unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, None);
    }

    #[tokio::test]
    async fn test_theme_with_existing_settings() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // Create settings first via upsert
        upsert(&db, user.id, 50).await.unwrap();

        // Update theme should work on existing settings
        update_theme(&db, user.id, Some("dark".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        // Verify entries_per_page is preserved
        let settings = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(settings.entries_per_page, 50);
        assert_eq!(settings.theme, Some("dark".to_string()));
    }

    #[tokio::test]
    async fn test_retention_read_days_default_zero() {
        let db = setup_db().await;
        let user = user::create_user(&db, "ret", "hash", Role::User)
            .await
            .unwrap();
        assert_eq!(get_retention_read_days(&db, user.id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_update_retention_read_days() {
        let db = setup_db().await;
        let user = user::create_user(&db, "ret", "hash", Role::User)
            .await
            .unwrap();

        update_retention_read_days(&db, user.id, 30).await.unwrap();
        assert_eq!(get_retention_read_days(&db, user.id).await.unwrap(), 30);

        // Preserves other settings.
        upsert(&db, user.id, 50).await.unwrap();
        update_retention_read_days(&db, user.id, 14).await.unwrap();
        let s = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(s.retention_read_days, 14);
        assert_eq!(s.entries_per_page, 50);

        // Negatives are rejected.
        assert!(matches!(
            update_retention_read_days(&db, user.id, -1).await,
            Err(AppError::Validation(_))
        ));

        // Values above the upper bound are rejected.
        assert!(matches!(
            update_retention_read_days(&db, user.id, MAX_RETENTION_READ_DAYS + 1).await,
            Err(AppError::Validation(_))
        ));

        // The boundary itself is accepted.
        update_retention_read_days(&db, user.id, MAX_RETENTION_READ_DAYS)
            .await
            .unwrap();
        assert_eq!(
            get_retention_read_days(&db, user.id).await.unwrap(),
            MAX_RETENTION_READ_DAYS
        );
    }

    #[tokio::test]
    async fn test_sidebar_prefs_default() {
        let db = setup_db().await;
        let user = user::create_user(&db, "sb", "hash", Role::User)
            .await
            .unwrap();

        // No settings row at all.
        assert_eq!(
            get_sidebar_prefs(&db, user.id).await.unwrap(),
            SidebarPrefs::default()
        );

        // A row created by another setting still reports the column defaults.
        upsert(&db, user.id, 50).await.unwrap();
        let prefs = get_sidebar_prefs(&db, user.id).await.unwrap();
        assert_eq!(prefs.sort, SIDEBAR_SORT_NAME);
        assert!(!prefs.hide_read);
    }

    #[tokio::test]
    async fn test_update_sidebar_prefs() {
        let db = setup_db().await;
        let user = user::create_user(&db, "sb", "hash", Role::User)
            .await
            .unwrap();

        update_sidebar_prefs(&db, user.id, SIDEBAR_SORT_UNREAD, true)
            .await
            .unwrap();
        let prefs = get_sidebar_prefs(&db, user.id).await.unwrap();
        assert_eq!(prefs.sort, SIDEBAR_SORT_UNREAD);
        assert!(prefs.hide_read);

        // Preserves the other settings.
        upsert(&db, user.id, 50).await.unwrap();
        update_sidebar_prefs(&db, user.id, SIDEBAR_SORT_NAME, false)
            .await
            .unwrap();
        let s = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(s.entries_per_page, 50);
        assert_eq!(s.sidebar_sort, SIDEBAR_SORT_NAME);
        assert!(!s.sidebar_hide_read);
    }

    #[tokio::test]
    async fn test_unknown_sidebar_sort_falls_back_to_default() {
        let db = setup_db().await;
        let user = user::create_user(&db, "sb", "hash", Role::User)
            .await
            .unwrap();

        assert_eq!(parse_sidebar_sort("nonsense"), DEFAULT_SIDEBAR_SORT);

        // A rejected value must not be persisted verbatim either.
        update_sidebar_prefs(&db, user.id, "nonsense", false)
            .await
            .unwrap();
        let s = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(s.sidebar_sort, DEFAULT_SIDEBAR_SORT);
    }
}
