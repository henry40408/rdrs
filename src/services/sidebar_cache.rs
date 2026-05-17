use std::time::Duration;

use moka::sync::Cache;

use crate::handlers::user::SidebarCategoryDto;

/// Cached per-user chrome data. Excludes session-specific fields (the
/// masquerade admin flag), so the same entry serves every request from
/// the same user_id regardless of session.
#[derive(Clone, Default)]
pub struct CachedChrome {
    pub theme: Option<String>,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
}

/// In-memory per-user cache for sidebar chrome data — replaces the 4 SQL
/// queries that every page render previously ran against the read pool
/// (theme + categories + per-category unread + total unread).
///
/// Cache entries are invalidated explicitly by handlers that write data
/// affecting any of those fields (mark-read, category CRUD, feed CRUD,
/// theme update, feed sync, account deletion). A short TTL backs the
/// explicit busts up — anything we forget to invalidate becomes stale
/// for at most `ttl_secs`, not forever.
#[derive(Clone)]
pub struct SidebarCache {
    cache: Cache<i64, CachedChrome>,
}

impl SidebarCache {
    pub fn new(max_capacity: u64, ttl_secs: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();
        Self { cache }
    }

    pub fn get(&self, user_id: i64) -> Option<CachedChrome> {
        self.cache.get(&user_id)
    }

    pub fn insert(&self, user_id: i64, chrome: CachedChrome) {
        self.cache.insert(user_id, chrome);
    }

    /// Drop the cached entry for `user_id`. Safe to call from any
    /// handler that mutates chrome-affecting state.
    pub fn bust(&self, user_id: i64) {
        self.cache.invalidate(&user_id);
    }
}

impl Default for SidebarCache {
    fn default() -> Self {
        // 10 000 distinct users comfortably covers any single-host
        // deployment; the 60 s TTL bounds stale data when a bust is
        // missed (e.g. a write path we haven't yet wired up).
        Self::new(10_000, 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chrome(unread: i64) -> CachedChrome {
        CachedChrome {
            theme: Some("dark".to_string()),
            categories: vec![SidebarCategoryDto {
                id: 1,
                name: "News".to_string(),
                unread_count: unread,
            }],
            total_unread: unread,
        }
    }

    #[test]
    fn miss_returns_none() {
        let cache = SidebarCache::default();
        assert!(cache.get(1).is_none());
    }

    #[test]
    fn insert_then_get_returns_value() {
        let cache = SidebarCache::default();
        cache.insert(42, sample_chrome(7));
        let got = cache.get(42).expect("hit");
        assert_eq!(got.total_unread, 7);
        assert_eq!(got.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn bust_evicts_entry() {
        let cache = SidebarCache::default();
        cache.insert(42, sample_chrome(7));
        cache.bust(42);
        assert!(cache.get(42).is_none());
    }

    #[test]
    fn bust_is_scoped_to_user() {
        let cache = SidebarCache::default();
        cache.insert(1, sample_chrome(1));
        cache.insert(2, sample_chrome(2));
        cache.bust(1);
        assert!(cache.get(1).is_none());
        assert_eq!(cache.get(2).expect("user 2 untouched").total_unread, 2);
    }

    #[test]
    fn ttl_expires_entry() {
        // 1-second TTL so the test is fast but still exercises moka's
        // time-based eviction.
        let cache = SidebarCache::new(100, 1);
        cache.insert(1, sample_chrome(1));
        assert!(cache.get(1).is_some());
        std::thread::sleep(Duration::from_millis(1100));
        cache.cache.run_pending_tasks();
        assert!(cache.get(1).is_none(), "entry should expire after TTL");
    }
}
