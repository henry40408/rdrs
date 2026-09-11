//! Server-side latency benchmark for the entry-list pages: handler, queries and
//! template render, driven in-process through the real router.
//!
//! The counterpart to `e2e`'s `dom-bench`, which times only the browser's half
//! of a swap. This is the half a change to the list handlers or the entry
//! queries can move, and what "capture a baseline before and after" measures
//! for them.
//!
//! ```text
//! RDRS_FAST_HASH=1 cargo bench --bench entry_pages -- [--label before] [--entries 2000] [--iterations 200]
//! ```
//!
//! Seeds one account with `--entries` entries spread over 4 categories × 5
//! feeds — every third read, every tenth starred, every twentieth summarized —
//! into an in-memory SQLite database, then prints p50/p90/mean per route in
//! microseconds. In-memory means it measures the code rather than the disk, so
//! numbers are only comparable between runs on the same machine; interleave
//! the before and after runs rather than trusting one of each.

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use axum_test::TestServer;
use rdrs::models::{category, entry, entry_summary, feed};
use rdrs::{Db, Role, create_router};

struct Args {
    label: String,
    entries: usize,
    iterations: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        label: "run".to_string(),
        entries: 2000,
        iterations: 200,
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
            "--iterations" => args.iterations = number(),
            "--label" => args.label = it.next().expect("--label needs a value"),
            // `cargo bench` passes `--bench` to every harness-less target.
            _ => {}
        }
    }
    args
}

/// Returns the first feed and category, for the scoped pages to aim at.
async fn seed(db: &Db, n: usize) -> (i64, i64) {
    let user = common::seed_account(db, "bench", "vulture-mango-77-quilt", Role::Admin).await;
    let content = "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>".repeat(12);

    let mut feeds = Vec::new();
    let mut first_category = None;
    for c in 0..4 {
        let cat = category::create_category(db, user.id, &format!("Category {c}"))
            .await
            .unwrap();
        first_category.get_or_insert(cat.id);
        for f in 0..5 {
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
    let mut read = Vec::new();
    for i in 0..n {
        let minutes = i64::try_from(i).unwrap();
        let (e, _) = entry::upsert_entry(
            db,
            feeds[i % feeds.len()],
            &format!("guid-{i}"),
            Some(&format!("Entry {i}")),
            Some(&format!("https://example.com/{i}")),
            Some(&content),
            Some("A short summary."),
            None,
            Some(now - chrono::Duration::minutes(minutes)),
        )
        .await
        .unwrap();
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
    (feeds[0], first_category.unwrap())
}

/// The value of the first hidden input called `name`, as the Load-More form
/// would submit it.
fn hidden_value(html: &str, name: &str) -> Option<String> {
    let marker = format!(r#"name="{name}" value=""#);
    let start = html.find(&marker)? + marker.len();
    let len = html[start..].find('"')?;
    Some(html[start..start + len].to_string())
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let state = common::test_state(common::default_test_config()).await;
    let (feed_id, category_id) = seed(&state.db, args.entries).await;
    let mut server = TestServer::builder()
        .save_cookies()
        .build(create_router(state));
    common::login(&mut server, "bench").await;

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
    ];
    // Load More: page 2 of the busiest lists, through the cursor page 1 hands out.
    for (name, path) in [
        ("entries load-more", "/entries"),
        ("read load-more", "/entries/read"),
    ] {
        let page = common::get_ok(&server, path).await;
        if let Some(after) = hidden_value(&page, "after") {
            let after: String = url::form_urlencoded::byte_serialize(after.as_bytes()).collect();
            routes.push((name, format!("{path}?fragment=1&after={after}")));
        }
    }

    println!(
        "{:<10} {:<26} {:>10} {:>10} {:>10} {:>9}",
        "label", "route", "p50 µs", "p90 µs", "mean µs", "bytes"
    );
    for (name, path) in &routes {
        let bytes = common::get_ok(&server, path).await.len();
        for _ in 0..args.iterations / 10 {
            server.get(path).await;
        }
        let mut samples = Vec::with_capacity(args.iterations);
        for _ in 0..args.iterations {
            let start = Instant::now();
            server.get(path).await;
            samples.push(start.elapsed());
        }
        samples.sort();
        let mean = samples.iter().sum::<Duration>() / u32::try_from(samples.len()).unwrap();
        println!(
            "{:<10} {:<26} {:>10.1} {:>10.1} {:>10.1} {:>9}",
            args.label,
            name,
            micros(samples[samples.len() / 2]),
            micros(samples[samples.len() * 9 / 10]),
            micros(mean),
            bytes
        );
    }
}
