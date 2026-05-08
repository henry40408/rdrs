//! Verifies the page_cache helper: insert/get round-trip, manual
//! invalidation, and TTL expiry. The helper is a thin wrapper over
//! `moka::sync::Cache`; these tests pin the contract page-handlers
//! will rely on.

use std::time::Duration;

use rdrs::services::page_cache::new_page_cache;

#[test]
fn insert_and_get_roundtrip() {
    let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_secs(60));

    cache.insert((1, "sidebar"), "payload-A".to_string());

    assert_eq!(cache.get(&(1, "sidebar")), Some("payload-A".to_string()));
    assert_eq!(cache.get(&(1, "feeds")), None);
    assert_eq!(cache.get(&(2, "sidebar")), None);
}

#[test]
fn invalidate_removes_entry() {
    let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_secs(60));

    cache.insert((1, "sidebar"), "payload-A".to_string());
    cache.invalidate(&(1, "sidebar"));

    assert_eq!(cache.get(&(1, "sidebar")), None);
}

#[tokio::test]
async fn ttl_expiry_drops_entry() {
    let cache = new_page_cache::<(i64, &'static str), String>(64, Duration::from_millis(50));

    cache.insert((1, "sidebar"), "payload-A".to_string());
    assert_eq!(cache.get(&(1, "sidebar")), Some("payload-A".to_string()));

    tokio::time::sleep(Duration::from_millis(120)).await;
    // moka requires a sync_or_pending operation to advance expiry;
    // a get() call is sufficient.
    assert_eq!(cache.get(&(1, "sidebar")), None);
}
