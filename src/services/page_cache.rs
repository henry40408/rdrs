//! Thin helper around `moka::sync::Cache` for per-user, TTL-bounded
//! page caches.
//!
//! Page handlers wire one `Cache` per logical kind of payload they
//! want to memoize (sidebar tree, feeds list, statistics rollup).
//! CRUD paths invalidate explicitly via `Cache::invalidate(&key)`.
//!
//! This module deliberately does not own any global state — the
//! caches live in `AppState` (added when the first per-page PR
//! needs one).

use std::hash::Hash;
use std::time::Duration;

use moka::sync::Cache;

/// Build a new page cache with the given capacity and per-entry
/// time-to-live.
///
/// `capacity` is the maximum number of entries (an LRU bound, not
/// a byte bound). `ttl` is the time-to-live applied to each entry
/// from insertion.
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

/// How long the site-wide database figures on `/statistics` may lag reality.
///
/// Matches `SidebarCache`'s TTL, and for the same reason: nothing busts this
/// cache explicitly, because nothing a *request* does moves these numbers.
/// Entries arrive from feed sync and leave via the retention worker's prune and
/// `VACUUM`, all on their own schedules, so the TTL is the whole invalidation
/// strategy rather than a backstop for one.
pub const ADMIN_DB_STATS_TTL: Duration = Duration::from_secs(60);

/// Memoizes the admin block's site-wide database figures — everything
/// `models::statistics::get_admin_database_stats` returns.
///
/// Site-wide and period-independent, so the key is `()`: one slot serves every
/// admin and every period button. That is the point — the figures are a full
/// `COUNT(*)` over `entry`, another over `entry_tombstone` and the page-count
/// PRAGMAs, and without this they are recomputed on every render of a page
/// whose other queries are all index-covered. On a 567 MB / 70k-entry database
/// they were 612 of the default view's ~1,370 page misses.
pub type AdminDbStatsCache = Cache<(), crate::models::statistics::AdminDatabaseStats>;

/// Build the [`AdminDbStatsCache`]. One slot, because the key is `()`.
pub fn new_admin_db_stats_cache() -> AdminDbStatsCache {
    new_page_cache(1, ADMIN_DB_STATS_TTL)
}
