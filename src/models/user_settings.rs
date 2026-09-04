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

/// Values offered by the settings form's `<datalist>` for `entries_per_page`,
/// ascending. Every entry must satisfy [`upsert`]'s range check — a suggestion
/// the form would reject is worse than no suggestion at all, because the user
/// picked it out of the browser's own dropdown. Enforced by
/// `entries_per_page_suggestions_are_all_accepted`.
pub const ENTRIES_PER_PAGE_SUGGESTIONS: &[i64] = &[10, 25, 50, 100];

/// Same contract as [`ENTRIES_PER_PAGE_SUGGESTIONS`], for
/// `retention_read_days` against [`update_retention_read_days`]. `0` leads
/// because it is the default and means "never delete"; the form's help text
/// carries that meaning, since a `<datalist>` on a number input renders bare
/// values and `label` support is inconsistent across browsers.
pub const RETENTION_READ_DAYS_SUGGESTIONS: &[i64] = &[0, 7, 30, 90, 365];

/// Offline reading is off: the browser keeps nothing belonging to the reader.
/// The default, and what every account did before the setting existed.
pub const OFFLINE_KEEP_OFF: i64 = 0;

/// Upper bound on the entries a client mirrors for offline reading. The cap
/// exists because the reader is spending their *device's* disk, not the
/// server's, and every kept entry drags its images along with it.
pub const MAX_OFFLINE_KEEP: i64 = 200;

/// Same contract as [`ENTRIES_PER_PAGE_SUGGESTIONS`], for `offline_keep`
/// against [`update_offline_keep`]. `0` leads because it is the default and
/// means "off"; the form's help text carries that meaning, since a
/// `<datalist>` on a number input renders bare values.
pub const OFFLINE_KEEP_SUGGESTIONS: &[i64] = &[0, 25, 50, 100, 200];

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
    /// Newest unread entries to keep readable offline, or [`OFFLINE_KEEP_OFF`].
    pub offline_keep: i64,
    /// When this reader opted into open tracking, or `None` for opted out.
    /// Doubles as the baseline the open rate is measured from — see
    /// [`update_pixel_tracking`].
    pub pixel_tracking_enabled_at: Option<DateTime<Utc>>,
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
        "SELECT id, user_id, entries_per_page, retention_read_days, save_services, theme, sidebar_sort, sidebar_hide_read, offline_keep, pixel_tracking_enabled_at, created_at, updated_at FROM user_settings WHERE user_id = $1",
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

/// How many entries this user keeps readable offline. Accounts with no
/// `user_settings` row yet get [`OFFLINE_KEEP_OFF`] — offline reading is
/// opt-in, so "no row" must never mean "start writing articles to disk".
pub async fn get_offline_keep(db: &Db, user_id: i64) -> AppResult<i64> {
    Ok(find_by_user_id(db, user_id)
        .await?
        .map_or(OFFLINE_KEEP_OFF, |settings| settings.offline_keep))
}

/// Set the offline-reading budget. Out-of-range values are rejected rather
/// than clamped: this one spends the reader's disk, so a typo silently
/// becoming 200 entries is the wrong failure.
pub async fn update_offline_keep(db: &Db, user_id: i64, keep: i64) -> AppResult<()> {
    if !(OFFLINE_KEEP_OFF..=MAX_OFFLINE_KEEP).contains(&keep) {
        return Err(AppError::Validation(format!(
            "offline_keep must be between {OFFLINE_KEEP_OFF} and {MAX_OFFLINE_KEEP}"
        )));
    }
    // Ensure a row exists, then update (mirrors update_sidebar_prefs).
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
        "UPDATE user_settings SET offline_keep = $1, updated_at = $2 WHERE user_id = $3",
        keep,
        Utc::now(),
        user_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// When this reader opted into open tracking, or `None` for opted out.
/// Accounts with no `user_settings` row yet are opted out — tracking is
/// opt-in, so "no row" must never mean "start recording".
pub async fn get_pixel_tracking_enabled_at(
    db: &Db,
    user_id: i64,
) -> AppResult<Option<DateTime<Utc>>> {
    Ok(find_by_user_id(db, user_id)
        .await?
        .and_then(|settings| settings.pixel_tracking_enabled_at))
}

/// Turn open tracking on or off.
///
/// Enabling is `COALESCE`d against the stored value, so it takes effect only on
/// the NULL -> enabled transition: the timestamp is the baseline the open rate
/// is measured from, and re-saving the preferences form — which every other
/// preference change does — would otherwise reset the denominator and throw away
/// every entry tracked so far.
///
/// Disabling clears the baseline but keeps the `entry_open` rows, so turning it
/// back on resumes from the data already collected rather than starting over.
///
/// Written with the `datetime('now')` literal rather than a bound `Utc::now()`:
/// the value is compared against `entry.created_at` column-to-column, and on
/// `SQLite` a bound timestamp encodes in a different format that does not
/// compare correctly. See `models::entry_open`.
pub async fn update_pixel_tracking(db: &Db, user_id: i64, enabled: bool) -> AppResult<()> {
    // Ensure a row exists, then update (mirrors update_theme).
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;
    let sql = if enabled {
        "UPDATE user_settings \
         SET pixel_tracking_enabled_at = COALESCE(pixel_tracking_enabled_at, datetime('now')), \
             updated_at = datetime('now') \
         WHERE user_id = $1"
    } else {
        "UPDATE user_settings \
         SET pixel_tracking_enabled_at = NULL, updated_at = datetime('now') \
         WHERE user_id = $1"
    };
    db_execute!(db, sql, user_id).map_err(AppError::Database)?;
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
    async fn pixel_tracking_defaults_to_opted_out() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // No settings row at all — a brand-new account must not be tracking.
        assert_eq!(
            get_pixel_tracking_enabled_at(&db, user.id).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn enabling_pixel_tracking_twice_keeps_the_original_baseline() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        update_pixel_tracking(&db, user.id, true).await.unwrap();
        let first = get_pixel_tracking_enabled_at(&db, user.id)
            .await
            .unwrap()
            .expect("enabling records a baseline");

        // Every other preference change re-submits this form. Moving the
        // baseline forward each time would silently reset the denominator.
        update_pixel_tracking(&db, user.id, true).await.unwrap();
        assert_eq!(
            get_pixel_tracking_enabled_at(&db, user.id).await.unwrap(),
            Some(first)
        );
    }

    #[tokio::test]
    async fn disabling_pixel_tracking_clears_the_baseline() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        update_pixel_tracking(&db, user.id, true).await.unwrap();
        assert!(
            get_pixel_tracking_enabled_at(&db, user.id)
                .await
                .unwrap()
                .is_some()
        );

        update_pixel_tracking(&db, user.id, false).await.unwrap();
        assert_eq!(
            get_pixel_tracking_enabled_at(&db, user.id).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn offline_keep_defaults_to_off() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        // No settings row at all — the case a brand-new account is in, and the
        // one where "on" would mean writing articles to a disk nobody asked to
        // have them written to.
        assert_eq!(
            get_offline_keep(&db, user.id).await.unwrap(),
            OFFLINE_KEEP_OFF
        );
    }

    #[tokio::test]
    async fn offline_keep_round_trips_and_rejects_out_of_range() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        update_offline_keep(&db, user.id, 50).await.unwrap();
        assert_eq!(get_offline_keep(&db, user.id).await.unwrap(), 50);

        for bad in [-1, MAX_OFFLINE_KEEP + 1] {
            assert!(
                update_offline_keep(&db, user.id, bad).await.is_err(),
                "{bad} should be rejected"
            );
        }
        assert_eq!(
            get_offline_keep(&db, user.id).await.unwrap(),
            50,
            "a rejected write must not disturb the stored value"
        );
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

        let settings = upsert(&db, user.id, 50).await.unwrap();
        assert_eq!(settings.user_id, user.id);
        assert_eq!(settings.entries_per_page, 50);

        let found = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(found.entries_per_page, 50);

        let updated = upsert(&db, user.id, 75).await.unwrap();
        assert_eq!(updated.entries_per_page, 75);

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

        update_theme(&db, user.id, Some("dark".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        update_theme(&db, user.id, Some("light".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("light".to_string()));

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

        upsert(&db, user.id, 50).await.unwrap();

        update_theme(&db, user.id, Some("dark".to_string()))
            .await
            .unwrap();
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, Some("dark".to_string()));

        let settings = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(settings.entries_per_page, 50);
        assert_eq!(settings.theme, Some("dark".to_string()));
    }

    /// The `<datalist>` promise: every value the settings form offers for
    /// `entries_per_page` round-trips through `upsert`. A suggestion the form
    /// rejects is worse than none — the user picked it out of the browser's own
    /// dropdown, so a validation error there reads as a bug, not as a typo.
    #[tokio::test]
    async fn entries_per_page_suggestions_are_all_accepted() {
        let db = setup_db().await;
        let user = user::create_user(&db, "epp_sugg", "hash", Role::User)
            .await
            .unwrap();

        assert!(!ENTRIES_PER_PAGE_SUGGESTIONS.is_empty());
        for &v in ENTRIES_PER_PAGE_SUGGESTIONS {
            let settings = upsert(&db, user.id, v)
                .await
                .unwrap_or_else(|e| panic!("suggestion {v} rejected by upsert: {e:?}"));
            assert_eq!(settings.entries_per_page, v);
        }

        // A browser renders a datalist in document order, so an unsorted list
        // reads as arbitrary.
        assert!(ENTRIES_PER_PAGE_SUGGESTIONS.windows(2).all(|w| w[0] < w[1]));
    }

    /// Same contract for `retention_read_days`, including the leading `0`
    /// ("never delete") the form deliberately offers.
    #[tokio::test]
    async fn retention_read_days_suggestions_are_all_accepted() {
        let db = setup_db().await;
        let user = user::create_user(&db, "rrd_sugg", "hash", Role::User)
            .await
            .unwrap();

        assert!(RETENTION_READ_DAYS_SUGGESTIONS.contains(&0));
        for &v in RETENTION_READ_DAYS_SUGGESTIONS {
            update_retention_read_days(&db, user.id, v)
                .await
                .unwrap_or_else(|e| panic!("suggestion {v} rejected: {e:?}"));
            assert_eq!(get_retention_read_days(&db, user.id).await.unwrap(), v);
        }

        assert!(
            RETENTION_READ_DAYS_SUGGESTIONS
                .windows(2)
                .all(|w| w[0] < w[1])
        );
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
