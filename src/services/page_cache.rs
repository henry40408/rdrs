//! Thin helper around `moka::sync::Cache` for per-user, TTL-bounded page
//! caches. The caches live in `AppState`; CRUD paths invalidate explicitly.

use std::hash::Hash;
use std::time::Duration;

use moka::sync::Cache;

/// Build a page cache: `capacity` is an entry-count bound, `ttl` applies from
/// insertion.
pub fn new_page_cache<K, V>(capacity: u64, ttl: Duration) -> Cache<K, V>
where
    K: Hash + Eq + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    Cache::builder()
        .max_capacity(capacity)
        .time_to_live(ttl)
        .build()
}

/// How long `/statistics`' site-wide DB figures may lag. Nothing a request does
/// moves them, so the TTL is the whole invalidation strategy.
pub const ADMIN_DB_STATS_TTL: Duration = Duration::from_secs(60);

/// Memoizes `models::statistics::get_admin_database_stats`. Keyed on `()`: one
/// slot for every admin and period. Those full `COUNT(*)`s and PRAGMAs
/// dominated the page's page misses.
pub type AdminDbStatsCache = Cache<(), crate::models::statistics::AdminDatabaseStats>;

/// Build the [`AdminDbStatsCache`].
pub fn new_admin_db_stats_cache() -> AdminDbStatsCache {
    new_page_cache(1, ADMIN_DB_STATS_TTL)
}
