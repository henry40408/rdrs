//! Page-cache-miss benchmark for the entry-list pages and the `/feeds`,
//! `/categories` and `/statistics` pages, through the real router.
//!
//! ```text
//! RDRS_FAST_HASH=1 cargo bench --bench entry_pages -- [--label before] [--entries 20000]
//!     [--categories 4] [--feeds-per-category 5]
//! ```
//!
//! Seeds one account (some feeds with a fetch error or an old date so the
//! Errors/Stale filters have rows) into a file-backed `SQLite` database with the
//! app's pragmas, except `mmap_size=0` so every page read goes through the pager.
//! Before each request the pager caches of all pool connections are emptied, then
//! the request's `SQLITE_DBSTATUS_CACHE_MISS` count is summed over the pool. A
//! cold cache stands in for a Raspberry Pi whose database exceeds its RAM; unlike
//! wall clock on an SSD machine, the count does not depend on the machine. Each route
//! prints the misses with the sidebar cache busted (`miss`) and primed (`hit`);
//! the admin DB figures of `/statistics` are primed once before measuring.
//!
//! The unsafe FFI is confined to this bench; the library keeps `unsafe_code = "deny"`.
#![allow(unsafe_code)]

#[path = "../tests/common/mod.rs"]
mod common;

use axum_test::TestServer;
use rdrs::config::Backend;
use rdrs::db::DbInner;
use rdrs::models::{category, entry, entry_summary, feed};
use rdrs::{Db, Role, create_router};
use sqlx::SqlitePool;

struct Args {
    label: String,
    entries: usize,
    categories: usize,
    feeds_per_category: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        label: "run".to_string(),
        entries: 20_000,
        categories: 4,
        feeds_per_category: 5,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut number = || {
            it.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{arg} needs a number"))
        };
        match arg.as_str() {
            "--entries" => args.entries = number(),
            "--categories" => args.categories = number(),
            "--feeds-per-category" => args.feeds_per_category = number(),
            "--label" => args.label = it.next().expect("--label needs a value"),
            // `cargo bench` passes `--bench` to every harness-less target.
            _ => {}
        }
    }
    assert!(
        args.categories > 0 && args.feeds_per_category > 0,
        "--categories and --feeds-per-category must be at least 1"
    );
    args
}

/// Returns the user, the first feed and category, for the scoped pages to aim
/// at, and a mid-list entry for the per-entry routes.
async fn seed(db: &Db, args: &Args) -> (i64, i64, i64, i64) {
    let n = args.entries;
    let user = common::seed_account(db, "bench", "vulture-mango-77-quilt", Role::Admin).await;
    let content = "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>".repeat(12);

    let mut feeds = Vec::new();
    let mut first_category = None;
    for c in 0..args.categories {
        let cat = category::create_category(db, user.id, &format!("Category {c}"))
            .await
            .unwrap();
        first_category.get_or_insert(cat.id);
        for f in 0..args.feeds_per_category {
            let url = format!("https://feed{c}-{f}.example.com/rss");
            let title = format!("Feed {c}-{f}");
            let params = feed::CreateFeedParams {
                category_id: cat.id,
                url: &url,
                title: Some(&title),
                ..Default::default()
            };
            feeds.push(feed::create_feed(db, &params).await.unwrap().id);
        }
    }

    let now = chrono::Utc::now();
    mark_fetch_states(db, &feeds, now).await;
    let mut read = Vec::new();
    let mut mid_entry = 0;
    for i in 0..n {
        let minutes = i64::try_from(i).unwrap();
        let (e, _) = entry::upsert_entry(
            db,
            &entry::EntryUpsert {
                feed_id: feeds[i % feeds.len()],
                guid: &format!("guid-{i}"),
                title: Some(&format!("Entry {i}")),
                link: Some(&format!("https://example.com/{i}")),
                content: Some(&content),
                summary: Some("A short summary."),
                author: None,
                published_at: Some(now - chrono::Duration::minutes(minutes)),
            },
        )
        .await
        .unwrap();
        // A read entry: opening an unread one spawns a detached mark-read write.
        if i == n / 2 - n / 2 % 3 {
            mid_entry = e.id;
        }
        if i % 3 == 0 {
            read.push(e.id);
        }
        if i % 10 == 0 {
            entry::set_starred_for_user(db, user.id, e.id, true)
                .await
                .unwrap();
        }
        if i % 20 == 0 {
            entry_summary::upsert_pending(db, user.id, e.id)
                .await
                .unwrap();
            entry_summary::set_completed(db, user.id, e.id, "Summary.")
                .await
                .unwrap();
        }
    }
    entry::mark_read_by_ids(db, user.id, &read).await.unwrap();
    (user.id, feeds[0], first_category.unwrap(), mid_entry)
}

/// Gives the Errors and Stale filters of `/feeds` something to match: every 7th
/// feed failed its last fetch, every 5th has not published for 120 days, the
/// rest published recently.
async fn mark_fetch_states(db: &Db, feeds: &[i64], now: chrono::DateTime<chrono::Utc>) {
    for (i, &id) in feeds.iter().enumerate() {
        let error = (i % 7 == 0).then_some("connection refused");
        let age_days = if i % 5 == 0 { 120 } else { 1 + (i % 20) as i64 };
        feed::update_fetch_result(
            db,
            &feed::FetchResult {
                fetch_error: error,
                feed_updated_at: Some(now - chrono::Duration::days(age_days)),
                ..feed::FetchResult::new(id, now)
            },
        )
        .await
        .unwrap();
    }
}

/// The first hidden input called `name`, as the Load-More form submits it.
fn hidden_value(html: &str, name: &str) -> Option<String> {
    let marker = format!(r#"name="{name}" value=""#);
    let start = html.find(&marker)? + marker.len();
    let len = html[start..].find('"')?;
    Some(html[start..start + len].to_string())
}

/// Empties the pager cache of every pool connection and zeroes its miss counter.
async fn go_cold(pool: &SqlitePool) {
    drain_misses(pool, true).await;
}

/// Cache misses since the last call, summed over every pool connection.
async fn read_misses(pool: &SqlitePool) -> i64 {
    drain_misses(pool, false).await
}

async fn drain_misses(pool: &SqlitePool, cold: bool) -> i64 {
    let mut held = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        held.push(pool.acquire().await.unwrap());
    }
    let mut total = 0;
    for conn in &mut held {
        if cold {
            sqlx::query("PRAGMA mmap_size = 0")
                .execute(&mut **conn)
                .await
                .unwrap();
            sqlx::query("PRAGMA shrink_memory")
                .execute(&mut **conn)
                .await
                .unwrap();
        }
        let mut handle = conn.lock_handle().await.unwrap();
        let (mut current, mut highwater) = (0, 0);
        // SAFETY: the pointer is the live connection locked by `handle`, and
        // the two out-pointers are valid for the call.
        let rc = unsafe {
            libsqlite3_sys::sqlite3_db_status(
                handle.as_raw_handle().as_ptr(),
                libsqlite3_sys::SQLITE_DBSTATUS_CACHE_MISS,
                &raw mut current,
                &raw mut highwater,
                1,
            )
        };
        assert_eq!(rc, libsqlite3_sys::SQLITE_OK);
        total += i64::from(current);
    }
    total
}

/// One cold-cache GET to measure, with the sidebar cache in the given state.
struct MissProbe<'a> {
    server: &'a TestServer,
    state: &'a rdrs::AppState,
    pool: &'a SqlitePool,
    user_id: i64,
    path: &'a str,
    sidebar_primed: bool,
}

async fn misses_of(probe: MissProbe<'_>) -> i64 {
    probe.state.sidebar_cache.bust(probe.user_id);
    if probe.sidebar_primed {
        probe.server.get("/categories").await;
    }
    go_cold(probe.pool).await;
    probe.server.get(probe.path).await;
    read_misses(probe.pool).await
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench.sqlite3");
    let mut state = common::test_state(common::default_test_config()).await;
    state.db = Db::connect(db_path.to_str().unwrap(), Backend::Sqlite)
        .await
        .unwrap();
    let DbInner::Sqlite(pool) = state.db.inner().clone() else {
        unreachable!("the bench connects SQLite");
    };
    let (user_id, feed_id, category_id, mid_entry) = seed(&state.db, &args).await;
    let bench_state = state.clone();
    let mut server = TestServer::builder()
        .save_cookies()
        .build(create_router(state));
    common::login(&mut server, "bench").await;
    // Fills `AdminDbStatsCache` (60 s TTL) so no route pays for it.
    server.get("/statistics").await;

    let mut routes = vec![
        ("unread", "/".to_string()),
        ("entries", "/entries".to_string()),
        ("read", "/entries/read".to_string()),
        ("starred", "/entries/starred".to_string()),
        ("summarized", "/entries/summarized".to_string()),
        ("feed", format!("/feeds/{feed_id}/entries")),
        (
            "feed ?status=unread",
            format!("/feeds/{feed_id}/entries?status=unread"),
        ),
        ("category", format!("/categories/{category_id}/entries")),
        (
            "category ?status=starred",
            format!("/categories/{category_id}/entries?status=starred"),
        ),
        // Every title matches: a full page of highlighted results.
        ("search", "/search?q=Entry".to_string()),
        // Content-only match against the Lorem body, the LIKE-heavy case.
        ("search content", "/search?q=adipiscing".to_string()),
        // What search-as-you-type fetches per keystroke.
        ("search live", "/search?q=Entry&fragment=1".to_string()),
        // The list pages that load every feed or category of the account.
        ("feeds", "/feeds".to_string()),
        ("feeds ?category", format!("/feeds?category={category_id}")),
        ("feeds ?filter=errors", "/feeds?filter=errors".to_string()),
        ("feeds ?filter=stale", "/feeds?filter=stale".to_string()),
        ("categories", "/categories".to_string()),
        // The bench user is an admin; the cached site-wide DB figures are primed above.
        ("statistics", "/statistics".to_string()),
        (
            "statistics ?period=30d",
            "/statistics?period=30d".to_string(),
        ),
        (
            "statistics ?period=all",
            "/statistics?period=all".to_string(),
        ),
    ];
    // The reading pane and its toolbar lookups, for a mid-list entry. `position`
    // (the "12 / 48" counts) has no counterpart before it existed.
    routes.push(("pane fragment", format!("/entries/{mid_entry}/fragment")));
    routes.push((
        "neighbors (all)",
        format!("/api/entries/{mid_entry}/neighbors"),
    ));
    routes.push((
        "position (all)",
        format!("/api/entries/{mid_entry}/position"),
    ));
    routes.push((
        "neighbors (unread)",
        format!("/api/entries/{mid_entry}/neighbors?unread_only=true"),
    ));
    routes.push((
        "position (unread)",
        format!("/api/entries/{mid_entry}/position?unread_only=true"),
    ));
    // Load More: page 2 of the busiest lists, through the cursor page 1 hands out.
    for (name, path) in [
        ("entries load-more", "/entries"),
        ("read load-more", "/entries/read"),
        ("summarized load-more", "/entries/summarized"),
        ("search load-more", "/search?q=Entry"),
    ] {
        let page = common::get_ok(&server, path).await;
        if let Some(after) = hidden_value(&page, "after") {
            let after: String = url::form_urlencoded::byte_serialize(after.as_bytes()).collect();
            let sep = if path.contains('?') { '&' } else { '?' };
            routes.push((name, format!("{path}{sep}fragment=1&after={after}")));
        }
    }

    println!(
        "{:<10} {:<26} {:>12} {:>12}",
        "label", "route", "misses miss", "misses hit"
    );
    for (name, path) in &routes {
        let probe = |sidebar_primed| MissProbe {
            server: &server,
            state: &bench_state,
            pool: &pool,
            user_id,
            path,
            sidebar_primed,
        };
        let miss = misses_of(probe(false)).await;
        let hit = misses_of(probe(true)).await;
        println!("{:<10} {:<26} {miss:>12} {hit:>12}", args.label, name);
    }
}
