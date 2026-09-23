use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::save::SaveServicesConfig;
use crate::{db_execute, query_opt};

pub const DEFAULT_ENTRIES_PER_PAGE: i64 = 30;
pub const MIN_ENTRIES_PER_PAGE: i64 = 10;
pub const MAX_ENTRIES_PER_PAGE: i64 = 100;

/// Upper bound for read-entry retention, in days (~10 years); `0` means never delete.
pub const MAX_RETENTION_READ_DAYS: i64 = 3650;

/// `<datalist>` values for `entries_per_page`, ascending; each must pass
/// [`upsert`]'s range check (enforced by a test).
pub const ENTRIES_PER_PAGE_SUGGESTIONS: &[i64] = &[10, 25, 50, 100];

/// Same contract as [`ENTRIES_PER_PAGE_SUGGESTIONS`], for [`update_retention_read_days`].
/// `0` (default) means "never delete".
pub const RETENTION_READ_DAYS_SUGGESTIONS: &[i64] = &[0, 7, 30, 90, 365];

/// Offline reading off (the default).
pub const OFFLINE_KEEP_OFF: i64 = 0;

/// Cap on entries mirrored offline; it spends the reader's device disk, images included.
pub const MAX_OFFLINE_KEEP: i64 = 200;

/// Same contract as [`ENTRIES_PER_PAGE_SUGGESTIONS`], for [`update_offline_keep`].
/// `0` (default) means off.
pub const OFFLINE_KEEP_SUGGESTIONS: &[i64] = &[0, 25, 50, 100, 200];

/// Sidebar ordering A-Z by name (the list queries' native order).
pub const SIDEBAR_SORT_NAME: &str = "name";
/// Sidebar ordering: most unread first, ties keeping their A-Z order.
pub const SIDEBAR_SORT_UNREAD: &str = "unread";
pub const DEFAULT_SIDEBAR_SORT: &str = SIDEBAR_SORT_NAME;

/// Sidebar display preferences, sent together in the `/api/sidebar` payload.
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

/// Map a sort value to a known ordering; unknown values fall back to the
/// default so a display preference can never break a page render.
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
    /// When open tracking was enabled (`None` = opted out); also the open-rate
    /// baseline — see [`update_pixel_tracking`].
    pub pixel_tracking_enabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The decrypted `save_services` column. `Undecryptable` is distinct from
/// empty so a rotated `RDRS_SECRET` doesn't invite the user to overwrite a
/// credential that is unreadable, not lost.
#[derive(Debug, Clone)]
pub enum StoredServices {
    /// Nothing stored, or stored and read successfully.
    Config(SaveServicesConfig),
    /// A value written by [`crate::secret::seal`] that this key cannot open.
    Undecryptable,
}

impl StoredServices {
    /// The config, or empty when undecryptable. Read-only paths only: writers
    /// must handle [`StoredServices::Undecryptable`] themselves.
    pub fn or_default(self) -> SaveServicesConfig {
        match self {
            StoredServices::Config(config) => config,
            StoredServices::Undecryptable => SaveServicesConfig::default(),
        }
    }

    pub fn is_undecryptable(&self) -> bool {
        matches!(self, StoredServices::Undecryptable)
    }

    /// The config, or an error saying it is undecryptable (not "not
    /// configured") for user-triggered paths.
    pub fn usable(self) -> AppResult<SaveServicesConfig> {
        match self {
            StoredServices::Config(config) => Ok(config),
            StoredServices::Undecryptable => Err(AppError::Validation(
                "Stored service credentials cannot be decrypted with the current RDRS_SECRET. \
                 Restore the previous secret, or re-enter the credentials in settings."
                    .to_string(),
            )),
        }
    }
}

impl UserSettings {
    /// Read `save_services`, accepting legacy plaintext JSON until the next
    /// write seals it. `key` is `None` when `RDRS_SECRET` was generated — see
    /// [`crate::config::Config::service_token_key`].
    pub fn get_save_services_config(&self, key: Option<&[u8]>) -> StoredServices {
        let Some(stored) = self.save_services.as_deref() else {
            return StoredServices::Config(SaveServicesConfig::default());
        };

        let json = if crate::secret::is_sealed(stored) {
            match key.and_then(|key| crate::secret::open(key, stored)) {
                Some(json) => json,
                None => return StoredServices::Undecryptable,
            }
        } else {
            stored.to_string()
        };

        StoredServices::Config(SaveServicesConfig::from_json(&json).unwrap_or_default())
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

/// Insert a default settings row if missing, so a following UPDATE has a target.
async fn ensure_row(db: &Db, user_id: i64) -> AppResult<()> {
    db_execute!(
        db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2) \
         ON CONFLICT(user_id) DO NOTHING",
        user_id,
        DEFAULT_ENTRIES_PER_PAGE
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Read a user's `SaveServicesConfig`; see [`StoredServices`].
pub async fn get_save_services_config(
    db: &Db,
    user_id: i64,
    key: Option<&[u8]>,
) -> AppResult<StoredServices> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.get_save_services_config(key)),
        None => Ok(StoredServices::Config(SaveServicesConfig::default())),
    }
}

/// Write a user's `SaveServicesConfig`, sealed with `key` if given. With no key
/// (generated `RDRS_SECRET`, new each boot) it stays plaintext, since sealing
/// would lose it on restart.
pub async fn update_save_services(
    db: &Db,
    user_id: i64,
    config: &SaveServicesConfig,
    key: Option<&[u8]>,
) -> AppResult<UserSettings> {
    let json = config
        .to_json()
        .map_err(|e| AppError::Internal(format!("Failed to serialize save_services: {e}")))?;
    let json = match key {
        Some(key) => crate::secret::seal(key, &json),
        None => json,
    };

    ensure_row(db, user_id).await?;

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

pub async fn get_theme(db: &Db, user_id: i64) -> AppResult<Option<String>> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.theme),
        None => Ok(None),
    }
}

pub async fn update_theme(db: &Db, user_id: i64, theme: Option<String>) -> AppResult<()> {
    ensure_row(db, user_id).await?;

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

/// Read-entry retention in days (0 = disabled).
pub async fn get_retention_read_days(db: &Db, user_id: i64) -> AppResult<i64> {
    match find_by_user_id(db, user_id).await? {
        Some(settings) => Ok(settings.retention_read_days),
        None => Ok(0),
    }
}

/// Set read-entry retention in days; rejects values outside `0..=MAX_RETENTION_READ_DAYS`.
pub async fn update_retention_read_days(db: &Db, user_id: i64, days: i64) -> AppResult<()> {
    if !(0..=MAX_RETENTION_READ_DAYS).contains(&days) {
        return Err(AppError::Validation(format!(
            "retention_read_days must be between 0 and {MAX_RETENTION_READ_DAYS}"
        )));
    }
    ensure_row(db, user_id).await?;
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

/// Sidebar preferences; defaults when no settings row exists.
pub async fn get_sidebar_prefs(db: &Db, user_id: i64) -> AppResult<SidebarPrefs> {
    Ok(find_by_user_id(db, user_id)
        .await?
        .as_ref()
        .map_or_else(SidebarPrefs::default, sidebar_prefs_of))
}

/// Sidebar preferences from an already-loaded row, saving a query.
pub fn sidebar_prefs_of(settings: &UserSettings) -> SidebarPrefs {
    SidebarPrefs {
        sort: parse_sidebar_sort(&settings.sidebar_sort),
        hide_read: settings.sidebar_hide_read,
    }
}

/// Set sidebar preferences; `sort` is normalised via [`parse_sidebar_sort`], not rejected.
pub async fn update_sidebar_prefs(
    db: &Db,
    user_id: i64,
    sort: &str,
    hide_read: bool,
) -> AppResult<()> {
    let sort = parse_sidebar_sort(sort);
    ensure_row(db, user_id).await?;
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

/// Offline entry budget; no row means [`OFFLINE_KEEP_OFF`] (opt-in).
pub async fn get_offline_keep(db: &Db, user_id: i64) -> AppResult<i64> {
    Ok(find_by_user_id(db, user_id)
        .await?
        .map_or(OFFLINE_KEEP_OFF, |settings| settings.offline_keep))
}

/// Set the offline budget; out-of-range is rejected, not clamped, since it spends the reader's disk.
pub async fn update_offline_keep(db: &Db, user_id: i64, keep: i64) -> AppResult<()> {
    if !(OFFLINE_KEEP_OFF..=MAX_OFFLINE_KEEP).contains(&keep) {
        return Err(AppError::Validation(format!(
            "offline_keep must be between {OFFLINE_KEEP_OFF} and {MAX_OFFLINE_KEEP}"
        )));
    }
    ensure_row(db, user_id).await?;
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

/// When open tracking was enabled; `None` (including no row) means opted out.
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
/// Enabling `COALESCE`s so re-saving the form doesn't reset the open-rate
/// baseline; disabling clears it but keeps `entry_open` rows. Uses
/// `datetime('now')`, not a bound `Utc::now()`, because `SQLite` encodes bound
/// timestamps in a format that doesn't compare with `entry.created_at`.
pub async fn update_pixel_tracking(db: &Db, user_id: i64, enabled: bool) -> AppResult<()> {
    ensure_row(db, user_id).await?;
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
    use crate::models::user::Role;
    use crate::test_support::{seed_user, setup_db};

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn linkding_config(token: &str) -> SaveServicesConfig {
        SaveServicesConfig {
            linkding: Some(crate::services::save::linkding::LinkdingConfig {
                api_url: "https://linkding.example.com".to_string(),
                api_token: token.to_string(),
            }),
            kagi: None,
        }
    }

    async fn stored_column(db: &Db, user_id: i64) -> String {
        crate::query_scalar!(
            db,
            String,
            "SELECT save_services FROM user_settings WHERE user_id = $1",
            user_id
        )
        .unwrap()
    }

    async fn seeded_user(db: &Db) -> i64 {
        seed_user(db, "settingsuser", Role::User).await.id
    }

    #[tokio::test]
    async fn a_stored_token_is_not_readable_in_the_column() {
        let db = setup_db().await;
        let user_id = seeded_user(&db).await;

        update_save_services(&db, user_id, &linkding_config("SUPERSECRET123"), Some(KEY))
            .await
            .unwrap();

        // The whole point: a database dump does not hand over the token.
        let column = stored_column(&db, user_id).await;
        assert!(!column.contains("SUPERSECRET123"), "column was: {column}");
        assert!(crate::secret::is_sealed(&column));

        let read = get_save_services_config(&db, user_id, Some(KEY))
            .await
            .unwrap()
            .or_default();
        assert_eq!(
            read.linkding.unwrap().api_token,
            "SUPERSECRET123",
            "the value must survive the round trip"
        );
    }

    /// Legacy plaintext rows stay readable and get sealed on the next write.
    #[tokio::test]
    async fn a_legacy_plaintext_row_is_read_then_sealed_on_the_next_write() {
        let db = setup_db().await;
        let user_id = seeded_user(&db).await;

        update_save_services(&db, user_id, &linkding_config("LEGACY123"), None)
            .await
            .unwrap();
        assert!(stored_column(&db, user_id).await.contains("LEGACY123"));

        let read = get_save_services_config(&db, user_id, Some(KEY))
            .await
            .unwrap()
            .or_default();
        assert_eq!(read.linkding.unwrap().api_token, "LEGACY123");

        update_save_services(&db, user_id, &linkding_config("LEGACY123"), Some(KEY))
            .await
            .unwrap();
        assert!(!stored_column(&db, user_id).await.contains("LEGACY123"));
    }

    /// A rotated secret must not read as "nothing configured".
    #[tokio::test]
    async fn a_wrong_key_reports_undecryptable_rather_than_empty() {
        let db = setup_db().await;
        let user_id = seeded_user(&db).await;

        update_save_services(&db, user_id, &linkding_config("SUPERSECRET123"), Some(KEY))
            .await
            .unwrap();

        let stored = get_save_services_config(&db, user_id, Some(b"a different key, long enough"))
            .await
            .unwrap();
        assert!(stored.is_undecryptable());
        assert!(stored.clone().usable().is_err());
        // `or_default` still yields the empty config for chrome that only asks
        // "is anything configured".
        assert!(!stored.or_default().has_any_service());
    }

    /// A generated `RDRS_SECRET` (no key) must keep storing plaintext.
    #[tokio::test]
    async fn without_a_key_the_value_stays_plaintext_and_readable() {
        let db = setup_db().await;
        let user_id = seeded_user(&db).await;

        update_save_services(&db, user_id, &linkding_config("PLAIN123"), None)
            .await
            .unwrap();

        assert!(!crate::secret::is_sealed(
            &stored_column(&db, user_id).await
        ));
        let read = get_save_services_config(&db, user_id, None)
            .await
            .unwrap()
            .or_default();
        assert_eq!(read.linkding.unwrap().api_token, "PLAIN123");
    }

    #[tokio::test]
    async fn pixel_tracking_defaults_to_opted_out() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

        // No settings row at all — a brand-new account must not be tracking.
        assert_eq!(
            get_pixel_tracking_enabled_at(&db, user.id).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn enabling_pixel_tracking_twice_keeps_the_original_baseline() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

        update_pixel_tracking(&db, user.id, true).await.unwrap();
        let first = get_pixel_tracking_enabled_at(&db, user.id)
            .await
            .unwrap()
            .expect("enabling records a baseline");

        // Other preference changes re-submit this form; the baseline must not move.
        update_pixel_tracking(&db, user.id, true).await.unwrap();
        assert_eq!(
            get_pixel_tracking_enabled_at(&db, user.id).await.unwrap(),
            Some(first)
        );
    }

    #[tokio::test]
    async fn disabling_pixel_tracking_clears_the_baseline() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;

        // No settings row at all.
        assert_eq!(
            get_offline_keep(&db, user.id).await.unwrap(),
            OFFLINE_KEEP_OFF
        );
    }

    #[tokio::test]
    async fn offline_keep_round_trips_and_rejects_out_of_range() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;

        let entries_per_page = get_entries_per_page(&db, user.id).await.unwrap();
        assert_eq!(entries_per_page, DEFAULT_ENTRIES_PER_PAGE);
    }

    #[tokio::test]
    async fn test_upsert_and_find() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;

        // No settings exist yet, should return None
        let theme = get_theme(&db, user.id).await.unwrap();
        assert_eq!(theme, None);
    }

    #[tokio::test]
    async fn test_update_and_get_theme() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;

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

    /// Every `<datalist>` suggestion for `entries_per_page` passes `upsert`.
    #[tokio::test]
    async fn entries_per_page_suggestions_are_all_accepted() {
        let db = setup_db().await;
        let user = seed_user(&db, "epp_sugg", Role::User).await;

        assert!(!ENTRIES_PER_PAGE_SUGGESTIONS.is_empty());
        for &v in ENTRIES_PER_PAGE_SUGGESTIONS {
            let settings = upsert(&db, user.id, v)
                .await
                .unwrap_or_else(|e| panic!("suggestion {v} rejected by upsert: {e:?}"));
            assert_eq!(settings.entries_per_page, v);
        }

        // Datalists render in document order.
        assert!(ENTRIES_PER_PAGE_SUGGESTIONS.windows(2).all(|w| w[0] < w[1]));
    }

    /// Same contract for `retention_read_days`, including `0`.
    #[tokio::test]
    async fn retention_read_days_suggestions_are_all_accepted() {
        let db = setup_db().await;
        let user = seed_user(&db, "rrd_sugg", Role::User).await;

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
        let user = seed_user(&db, "ret", Role::User).await;
        assert_eq!(get_retention_read_days(&db, user.id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_update_retention_read_days() {
        let db = setup_db().await;
        let user = seed_user(&db, "ret", Role::User).await;

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
        let user = seed_user(&db, "sb", Role::User).await;

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
        let user = seed_user(&db, "sb", Role::User).await;

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
        let user = seed_user(&db, "sb", Role::User).await;

        assert_eq!(parse_sidebar_sort("nonsense"), DEFAULT_SIDEBAR_SORT);

        // A rejected value must not be persisted verbatim either.
        update_sidebar_prefs(&db, user.id, "nonsense", false)
            .await
            .unwrap();
        let s = find_by_user_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(s.sidebar_sort, DEFAULT_SIDEBAR_SORT);
    }
}
