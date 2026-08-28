use std::time::Duration;

use moka::ops::compute::Op;
use moka::sync::Cache;

use crate::handlers::user::SidebarCategoryDto;

/// Cached per-user chrome data. Excludes session-specific fields (the
/// masquerade admin flag), so the same entry serves every request from
/// the same `user_id` regardless of session.
#[derive(Clone, Default)]
pub struct CachedChrome {
    pub theme: Option<String>,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
    pub total_summarized: i64,
    /// How the client should order and filter the category / feed lists.
    /// Cached alongside the data it applies to, and busted by the preferences
    /// form like every other field here.
    pub sidebar_prefs: crate::models::user_settings::SidebarPrefs,
    /// Entries the reader keeps readable offline. Cached here for the same
    /// reason as `sidebar_prefs`: it lives in the row this cache already reads,
    /// so carrying it costs nothing and reading it separately would cost a
    /// query on every page render.
    pub offline_keep: i64,
}

/// Stamp identifying how many times a user's entry has been busted. Taken
/// before a read-through computation starts and handed back when it publishes,
/// so a publish that lost a race against a `bust` can be recognised and
/// dropped. Opaque on purpose — callers only ever round-trip it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation(u64);

/// One cache slot. `chrome` is `None` for a tombstone: `bust` keeps the slot
/// (with a bumped generation) rather than removing it, because the generation
/// is exactly what lets a slower concurrent read detect that it is stale.
#[derive(Clone)]
struct Slot {
    generation: u64,
    chrome: Option<CachedChrome>,
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
///
/// Population is read-through and therefore racy on its own: filling the entry
/// means several `await`s against the DB, and a `bust` landing inside that
/// window would be overwritten by the older snapshot and hidden for the whole
/// TTL. `begin_read` + `insert_if_current` close that window — see
/// `handlers::user::read_chrome_data`, the only reader.
#[derive(Clone)]
pub struct SidebarCache {
    cache: Cache<i64, Slot>,
    enabled: bool,
}

impl SidebarCache {
    pub fn new(max_capacity: u64, ttl_secs: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build();
        Self {
            cache,
            enabled: true,
        }
    }

    /// A cache that never serves or stores anything. Used by the E2E harness,
    /// which seeds straight into `SQLite` and so never runs the handlers that
    /// carry the `bust` hooks this cache depends on — leaving it on would let
    /// a page render that raced the seeding cache a half-written world.
    pub fn disabled() -> Self {
        Self {
            cache: Cache::builder().max_capacity(0).build(),
            enabled: false,
        }
    }

    pub fn get(&self, user_id: i64) -> Option<CachedChrome> {
        if !self.enabled {
            return None;
        }
        self.cache.get(&user_id).and_then(|slot| slot.chrome)
    }

    /// Snapshot the user's generation *before* a read-through computation
    /// starts reading the DB. Hand the result to `insert_if_current`.
    pub fn begin_read(&self, user_id: i64) -> Generation {
        Generation(self.cache.get(&user_id).map_or(0, |slot| slot.generation))
    }

    /// Publish `chrome`, unless a `bust` landed since `since` was taken.
    pub fn insert_if_current(&self, user_id: i64, since: Generation, chrome: CachedChrome) {
        if !self.enabled {
            return;
        }
        self.cache.entry(user_id).and_compute_with(|maybe| {
            let current = maybe.as_ref().map_or(0, |entry| entry.value().generation);
            if current == since.0 {
                Op::Put(Slot {
                    generation: current,
                    chrome: Some(chrome),
                })
            } else {
                // Lost the race: what we computed predates the bust. Dropping
                // it costs one recompute on the next request; publishing it
                // would hide the write for the whole TTL.
                Op::Nop
            }
        });
    }

    /// Invalidate `user_id`'s chrome. Safe to call from any handler that
    /// mutates chrome-affecting state.
    pub fn bust(&self, user_id: i64) {
        if !self.enabled {
            return;
        }
        self.cache.entry(user_id).and_compute_with(|maybe| {
            let generation = maybe
                .as_ref()
                .map_or(0, |entry| entry.value().generation)
                .wrapping_add(1);
            Op::Put(Slot {
                generation,
                chrome: None,
            })
        });
    }
}

impl Default for SidebarCache {
    fn default() -> Self {
        if std::env::var_os("RDRS_DISABLE_SIDEBAR_CACHE").is_some() {
            return Self::disabled();
        }
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
            total_summarized: 0,
            sidebar_prefs: crate::models::user_settings::SidebarPrefs::default(),
            offline_keep: crate::models::user_settings::OFFLINE_KEEP_OFF,
        }
    }

    /// Publish without a concurrent bust — the common path, and shorthand for
    /// the tests below that only care about the resulting value.
    fn publish(cache: &SidebarCache, user_id: i64, chrome: CachedChrome) {
        let generation = cache.begin_read(user_id);
        cache.insert_if_current(user_id, generation, chrome);
    }

    // Constructed explicitly rather than via `default()`: these assert on
    // caching behaviour, which `RDRS_DISABLE_SIDEBAR_CACHE` would switch off.
    fn cache() -> SidebarCache {
        SidebarCache::new(100, 60)
    }

    #[test]
    fn miss_returns_none() {
        assert!(cache().get(1).is_none());
    }

    #[test]
    fn insert_then_get_returns_value() {
        let cache = cache();
        publish(&cache, 42, sample_chrome(7));
        let got = cache.get(42).expect("hit");
        assert_eq!(got.total_unread, 7);
        assert_eq!(got.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn bust_evicts_entry() {
        let cache = cache();
        publish(&cache, 42, sample_chrome(7));
        cache.bust(42);
        assert!(cache.get(42).is_none());
    }

    #[test]
    fn bust_is_scoped_to_user() {
        let cache = cache();
        publish(&cache, 1, sample_chrome(1));
        publish(&cache, 2, sample_chrome(2));
        cache.bust(1);
        assert!(cache.get(1).is_none());
        assert_eq!(cache.get(2).expect("user 2 untouched").total_unread, 2);
    }

    #[test]
    fn ttl_expires_entry() {
        // 1-second TTL so the test is fast but still exercises moka's
        // time-based eviction.
        let cache = SidebarCache::new(100, 1);
        publish(&cache, 1, sample_chrome(1));
        assert!(cache.get(1).is_some());
        std::thread::sleep(Duration::from_millis(1100));
        cache.cache.run_pending_tasks();
        assert!(cache.get(1).is_none(), "entry should expire after TTL");
    }

    /// The race this cache's generation stamp exists to close: a read-through
    /// fill reads the DB, a write busts the entry, and only then does the fill
    /// publish. Without the stamp the pre-bust snapshot wins and the write
    /// stays invisible for the whole TTL.
    #[test]
    fn publish_that_lost_a_race_with_bust_is_dropped() {
        let cache = cache();
        publish(&cache, 1, sample_chrome(9));

        // Reader starts and snapshots the generation…
        let generation = cache.begin_read(1);
        // …then a writer marks everything read and busts.
        cache.bust(1);
        assert!(cache.get(1).is_none());

        // The slow reader now publishes what it read before the bust.
        cache.insert_if_current(1, generation, sample_chrome(9));
        assert!(
            cache.get(1).is_none(),
            "a publish from before the bust must not resurrect the stale entry"
        );
    }

    #[test]
    fn publish_started_after_a_bust_still_populates() {
        let cache = cache();
        publish(&cache, 1, sample_chrome(9));
        cache.bust(1);

        // A read that starts after the bust sees the bumped generation and is
        // free to publish — otherwise the entry could never refill.
        publish(&cache, 1, sample_chrome(0));
        assert_eq!(cache.get(1).expect("refilled").total_unread, 0);
    }

    #[test]
    fn back_to_back_busts_keep_invalidating() {
        let cache = cache();
        let generation = cache.begin_read(1);
        cache.bust(1);
        cache.bust(1);
        // Two busts, one stale publish: still dropped.
        cache.insert_if_current(1, generation, sample_chrome(9));
        assert!(cache.get(1).is_none());
    }

    #[test]
    fn disabled_cache_never_serves_a_value() {
        let cache = SidebarCache::disabled();
        publish(&cache, 1, sample_chrome(7));
        assert!(cache.get(1).is_none());
        // bust stays a no-op rather than panicking on the zero-capacity cache.
        cache.bust(1);
        assert!(cache.get(1).is_none());
    }
}
