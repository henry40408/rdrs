//! Live-PostgreSQL integration test for the dual-backend data layer (Phase C).
//!
//! Gated on `TEST_DATABASE_URL` (a `postgres://` URL): when unset the test is a
//! no-op so the default `cargo nextest` run — which only has `SQLite` — stays
//! green. CI sets it to a throwaway Postgres service. Locally:
//!
//! ```sh
//! docker run -d --name pg -e POSTGRES_PASSWORD=rdrs -e POSTGRES_USER=rdrs \
//!     -e POSTGRES_DB=rdrs -p 55432:5432 postgres:17-alpine
//! TEST_DATABASE_URL=postgres://rdrs:rdrs@localhost:55432/rdrs \
//!     cargo nextest run --test postgres_test
//! ```
//!
//! Everything runs in ONE test function so it stays isolated on a shared server
//! (parallel tests would race on the same tables). It drives the SQL paths that
//! dialect-fork between `SQLite` and PG — the composite-cursor `to_char`
//! comparison, `datetime('now')`→`now()`, interval arithmetic (`make_interval`),
//! `pg_database_size`, the `DATE()`/`to_char` stats bucket, the `"user"`
//! reserved-word quoting, the `is_unique_violation` mapping, and the `/search`
//! boolean query language's `ILIKE ... ESCAPE` free-text/`is:` matching plus
//! its `EXTRACT(EPOCH FROM ...)::bigint` date-bound comparison — against a
//! real server.

use rdrs::config::Backend;
use rdrs::db::Db;
use rdrs::error::AppError;
use rdrs::models::user::Role;
use rdrs::models::{
    category, entry,
    entry::{ContinuationCursor, ContinuationParams, EntryFilter, EntrySortOrder},
    feed, statistics, user, user_settings,
};

/// Connect to the test Postgres and wipe every table so the run is isolated and
/// repeatable against a persistent container. Returns `None` (skip) when
/// `TEST_DATABASE_URL` is unset.
async fn setup() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url, Backend::Postgres)
        .await
        .expect("connect to TEST_DATABASE_URL Postgres");
    rdrs::db_execute!(
        &db,
        "TRUNCATE \"user\", category, feed, entry, entry_summary, entry_tombstone, \
         image, passkey, session, user_settings, webauthn_challenge RESTART IDENTITY CASCADE"
    )
    .expect("truncate");
    Some(db)
}

async fn seed_feed(db: &Db) -> (i64, i64, i64) {
    let user_id = user::create_user(db, "pguser", "hash", Role::User)
        .await
        .unwrap()
        .id;
    let category_id = category::create_category(db, user_id, "Tech")
        .await
        .unwrap()
        .id;
    let feed_id = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id,
            url: "https://example.com/feed.xml",
            title: Some("Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap()
    .id;
    (user_id, category_id, feed_id)
}

fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

#[tokio::test]
async fn pg_dialect_smoke() {
    let Some(db) = setup().await else {
        eprintln!("TEST_DATABASE_URL unset; skipping Postgres integration test");
        return;
    };
    let (user_id, _cat, feed_id) = seed_feed(&db).await;

    // --- composite-cursor pagination (to_char cursor + %Y-%m-%d %H:%M:%S) -----
    let mut ids = Vec::new();
    for (guid, pub_at) in [
        ("g1", "2026-07-07T12:00:00Z"),
        ("g2", "2026-07-07T12:00:00Z"), // shares a second → id tie-break
        ("g3", "2026-07-06T09:30:00Z"),
    ] {
        let (e, _) = entry::upsert_entry(
            &db,
            feed_id,
            guid,
            Some("t"),
            Some("l"),
            None,
            None,
            None,
            Some(utc(pub_at)),
        )
        .await
        .unwrap();
        ids.push(e.id);
    }

    // --- guarded upsert UPDATE (IS DISTINCT FROM fork) ------------------------
    // The no-op guard is the one predicate that differs per dialect (SQLite
    // spells it `IS NOT`), so exercise both arms of it here: identical values
    // must not rewrite the row, a changed column must.
    let unchanged = entry::upsert_entry_id(
        &db,
        feed_id,
        "g1",
        Some("t"),
        Some("l"),
        None,
        None,
        None,
        Some(utc("2026-07-07T12:00:00Z")),
    )
    .await
    .unwrap();
    assert!(
        matches!(unchanged, entry::UpsertOutcome::Unchanged(id) if id == ids[0]),
        "identical re-upsert must be Unchanged on PG, got {unchanged:?}"
    );

    let updated = entry::upsert_entry_id(
        &db,
        feed_id,
        "g1",
        Some("t changed"),
        Some("l"),
        None,
        None,
        None,
        Some(utc("2026-07-07T12:00:00Z")),
    )
    .await
    .unwrap();
    assert!(
        matches!(updated, entry::UpsertOutcome::Updated(id) if id == ids[0]),
        "changed column must be Updated on PG, got {updated:?}"
    );
    // Restore the original title so the pagination assertions below are unaffected.
    entry::upsert_entry_id(
        &db,
        feed_id,
        "g1",
        Some("t"),
        Some("l"),
        None,
        None,
        None,
        Some(utc("2026-07-07T12:00:00Z")),
    )
    .await
    .unwrap();

    let filter = EntryFilter::default();
    let page1 = entry::list_by_user_with_continuation(
        &db,
        user_id,
        &filter,
        &ContinuationParams {
            oldest_first: false,
            limit: 1,
            sort_order: EntrySortOrder::PublishedAt,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page1.len(), 1);
    let last = &page1[0];
    let sort_ts = entry::fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
        .await
        .unwrap()
        .expect("sort_ts");
    // The cursor string must be the SQLite-compatible space-format, not RFC3339.
    assert_eq!(
        sort_ts, "2026-07-07 12:00:00",
        "cursor ts must be %Y-%m-%d %H:%M:%S"
    );

    let page2 = entry::list_by_user_with_continuation(
        &db,
        user_id,
        &filter,
        &ContinuationParams {
            oldest_first: false,
            limit: 10,
            sort_order: EntrySortOrder::PublishedAt,
            continuation: Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        page2.len(),
        2,
        "cursor must return the two remaining entries"
    );
    assert!(
        page2[0].entry.id < last.entry.id || page2[0].entry.published_at < last.entry.published_at,
        "continuation must not repeat or precede the cursor row"
    );

    // --- personal overview + category/feed stats (date-range vs timestamp) ----
    let overview = statistics::get_personal_overview(&db, user_id, "2026-07-01", "2026-07-31")
        .await
        .unwrap();
    assert_eq!(overview.total_entries, 3, "all 3 entries fall in range");
    let by_cat = statistics::get_entries_by_category(&db, user_id, "2026-07-01", "2026-07-31")
        .await
        .unwrap();
    assert_eq!(by_cat.len(), 1);
    assert_eq!(by_cat[0].count, 3);
    let top_feeds = statistics::get_top_feeds(&db, user_id, "2026-07-01", "2026-07-31", 10)
        .await
        .unwrap();
    assert_eq!(top_feeds.len(), 1);
    assert_eq!(top_feeds[0].count, 3);
    let admin_entries = statistics::get_admin_entry_stats(&db, "2026-07-01", "2026-07-31")
        .await
        .unwrap();
    assert_eq!(admin_entries.total_entries, 3);

    // --- find_neighbors (cursor to_char comparison + sort_ts read) ------------
    // ids[2] is g3 (oldest by published_at): it has a newer neighbour (prev) and
    // no older one (next).
    let oldest = ids[2];
    let neigh = entry::find_neighbors(&db, user_id, oldest, &EntryFilter::default())
        .await
        .unwrap();
    assert!(
        neigh.prev_id.is_some(),
        "oldest entry has a newer neighbour"
    );
    assert!(
        neigh.next_id.is_none(),
        "oldest entry has no older neighbour"
    );

    // --- datetime('now')->now() shim + snapshot read_after (to_char) ----------
    let target = last.entry.id;
    let (_, changed) = entry::set_read_for_user(&db, user_id, target, true)
        .await
        .unwrap()
        .unwrap();
    assert!(changed);
    let refreshed = entry::find_by_id(&db, target).await.unwrap().unwrap();
    assert!(refreshed.read_at.is_some(), "read_at set via now()");

    // Unread filter with a snapshot boundary just before the read instant keeps
    // the just-read entry in-snapshot: exercises the to_char read_at comparison.
    let snapshot = EntryFilter {
        unread_only: true,
        read_after: Some("2000-01-01 00:00:00".to_string()),
        ..Default::default()
    };
    let in_snapshot = entry::list_by_user_with_continuation(
        &db,
        user_id,
        &snapshot,
        &ContinuationParams {
            oldest_first: false,
            limit: 10,
            sort_order: EntrySortOrder::PublishedAt,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        in_snapshot.iter().any(|e| e.entry.id == target),
        "read-after snapshot must keep the just-read entry visible"
    );

    // --- mark_all_read_by_feed age cutoff (make_interval fork) ----------------
    let affected = entry::mark_all_read_by_feed(&db, feed_id, Some(3650))
        .await
        .unwrap();
    assert_eq!(affected, 0, "no entry is older than 3650 days");

    // --- retention prune (column-driven make_interval fork) -------------------
    // Backdate the read entry 40 days (PG-only test SQL) and set 30-day
    // retention so it becomes a prune victim.
    rdrs::db_execute!(
        &db,
        "UPDATE entry SET read_at = now() - make_interval(days => 40) WHERE id = $1",
        target
    )
    .unwrap();
    user_settings::update_retention_read_days(&db, user_id, 30)
        .await
        .unwrap();
    let deleted = entry::prune_read_retention_batch(&db, 100).await.unwrap();
    assert_eq!(deleted, 1, "the 40-day-old read entry must be pruned");
    assert!(entry::find_by_id(&db, target).await.unwrap().is_none());

    // --- statistics: DATE()/to_char bucket + read_at range --------------------
    // Read one of the remaining entries "today" for a non-empty daily bucket.
    let read_today = page2[0].entry.id;
    entry::set_read_for_user(&db, user_id, read_today, true)
        .await
        .unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let counts = statistics::get_daily_read_counts(&db, user_id, &today, &tomorrow)
        .await
        .unwrap();
    let total: i64 = counts.iter().map(|c| c.count).sum();
    assert_eq!(total, 1, "exactly one entry read today");

    // --- admin stats: pg_database_size + MIN/MAX(created_at) to_char ----------
    let admin = statistics::get_admin_database_stats(&db).await.unwrap();
    assert!(admin.db_size_bytes > 0, "pg_database_size must be positive");
    assert_eq!(admin.total_entries, 2, "two entries remain after prune");
    // PG cannot measure reclaimable space without an extension, so it reports
    // None and `/statistics` drops the card. It must never report a zero, which
    // would render as "0 B · 0% of file" and read as a bloat-free database.
    assert!(
        admin.reclaimable.is_none(),
        "PG must not claim a reclaimable figure it cannot measure"
    );

    // --- unique-violation mapping (is_unique_violation on PG) ------------------
    let err = category::create_category(&db, user_id, "Tech")
        .await
        .expect_err("duplicate category must fail");
    assert!(
        matches!(err, AppError::CategoryExists),
        "expected CategoryExists, got {err:?}"
    );

    // --- /search boolean query language: is:unread + free text (ILIKE ESCAPE) -
    // Everything in this file shares one live server and one test function by
    // design (see the module doc comment) so these query-language assertions
    // extend `pg_dialect_smoke` with more seed data rather than adding
    // sibling `#[tokio::test]` fns, which would TRUNCATE the same tables
    // concurrently and race.
    entry::upsert_entry(
        &db,
        feed_id,
        "guid-rust-news",
        Some("rust news"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (rust_read, _) = entry::upsert_entry(
        &db,
        feed_id,
        "guid-rust-read",
        Some("rust read already"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    entry::set_read_for_user(&db, user_id, rust_read.id, true)
        .await
        .unwrap();
    entry::upsert_entry(
        &db,
        feed_id,
        "guid-go-news",
        Some("go news"),
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let unread_filter = EntryFilter {
        query: Some(entry::query::parse("is:unread rust").unwrap()),
        ..Default::default()
    };
    let unread_rows = entry::list_by_user(
        &db,
        user_id,
        &unread_filter,
        EntrySortOrder::PublishedAt,
        50,
        0,
    )
    .await
    .unwrap();
    let unread_titles: Vec<_> = unread_rows
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(unread_titles.iter().any(|t| t == "rust news"));
    assert!(!unread_titles.iter().any(|t| t == "rust read already")); // read -> excluded by is:unread
    assert!(!unread_titles.iter().any(|t| t == "go news")); // no "rust" -> excluded by free text

    // --- /search boolean query language: after:2026-01-01 (epoch comparison) --
    // Exercises the PG `EXTRACT(EPOCH FROM ...)::bigint` fork for date bounds
    // (SQLite uses `strftime('%s', ...)`; see `query_after_date_filters` in
    // `models::entry::mod`'s unit tests for the SQLite-side mirror).
    entry::upsert_entry(
        &db,
        feed_id,
        "guid-old",
        Some("old"),
        None,
        None,
        None,
        None,
        Some(utc("2025-06-01T00:00:00Z")),
    )
    .await
    .unwrap();
    entry::upsert_entry(
        &db,
        feed_id,
        "guid-new",
        Some("new"),
        None,
        None,
        None,
        None,
        Some(utc("2026-03-01T00:00:00Z")),
    )
    .await
    .unwrap();

    let after_filter = EntryFilter {
        query: Some(entry::query::parse("after:2026-01-01").unwrap()),
        ..Default::default()
    };
    let after_rows = entry::list_by_user(
        &db,
        user_id,
        &after_filter,
        EntrySortOrder::PublishedAt,
        50,
        0,
    )
    .await
    .unwrap();
    let after_titles: Vec<_> = after_rows
        .iter()
        .filter_map(|r| r.entry.title.clone())
        .collect();
    assert!(after_titles.iter().any(|t| t == "new"));
    assert!(!after_titles.iter().any(|t| t == "old"));
}
