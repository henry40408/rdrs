use std::time::Duration;

use moka::ops::compute::Op;
use moka::sync::Cache;

use crate::handlers::user::SidebarCategoryDto;

/// Cached per-user chrome data; no session-specific fields, so one entry serves
/// every session of a `user_id`.
#[derive(Clone, Default)]
pub struct CachedChrome {
    pub theme: Option<String>,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
    pub total_summarized: i64,
    /// How the client orders and filters the category / feed lists.
    pub sidebar_prefs: crate::models::user_settings::SidebarPrefs,
    /// Entries kept readable offline; same row as `sidebar_prefs`, so free to carry.
    pub offline_keep: i64,
}

/// Bust count taken before a read-through fill and checked on publish, so a
/// fill that raced a `bust` is dropped. Opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation(u64);

/// One cache slot. `chrome` is `None` for a tombstone: `bust` keeps the slot so
/// its bumped generation can reveal stale concurrent reads.
#[derive(Clone)]
struct Slot {
    generation: u64,
    chrome: Option<CachedChrome>,
}

/// In-memory per-user cache for sidebar chrome (theme, categories, unread
/// counts), saving 4 queries per page render.
///
/// Handlers that write chrome-affecting data bust it explicitly; a short TTL
/// bounds staleness from a missed bust. `begin_read` + `insert_if_current`
/// stop a racing fill from overwriting a bust (see
/// `handlers::user::read_chrome_data`, the only reader).
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

    /// A cache that never stores anything, for the E2E harness: it seeds straight
    /// into `SQLite`, bypassing the handlers' `bust` hooks.
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

    /// Snapshot the generation before a read-through fill; pass it to
    /// `insert_if_current`.
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
                // Lost the race: publishing would hide the write for the whole TTL.
                Op::Nop
            }
        });
    }

    /// Invalidate `user_id`'s chrome.
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
        // 10 000 users covers any single host; the 60 s TTL bounds a missed bust.
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

    /// Publish without a concurrent bust.
    fn publish(cache: &SidebarCache, user_id: i64, chrome: CachedChrome) {
        let generation = cache.begin_read(user_id);
        cache.insert_if_current(user_id, generation, chrome);
    }

    // Not `default()`: `RDRS_DISABLE_SIDEBAR_CACHE` would disable caching.
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
        // 1 s TTL still exercises moka's time-based eviction.
        let cache = SidebarCache::new(100, 1);
        publish(&cache, 1, sample_chrome(1));
        assert!(cache.get(1).is_some());
        std::thread::sleep(Duration::from_millis(1100));
        cache.cache.run_pending_tasks();
        assert!(cache.get(1).is_none(), "entry should expire after TTL");
    }

    /// The race the generation stamp closes: fill reads, write busts, fill
    /// publishes. Without the stamp the write stays invisible for the TTL.
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

        // A read after the bust may publish, or the entry could never refill.
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
