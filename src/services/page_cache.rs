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
