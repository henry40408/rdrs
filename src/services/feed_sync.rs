use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, USER_AGENT};
use serde::Serialize;
use tracing::{debug, error, info, warn};

use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::models::{entry, feed, image};
use crate::services::http::{
    FEED_SYNC_TIMEOUT, RetryConfig, SHARED_CLIENT, SHARED_CLIENT_H1, send_with_retry_on_error,
};
use crate::services::icon_fetcher;
use crate::utils::datetime::parse_timestamp;

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_entries: i64,
    pub updated_entries: i64,
}

/// Pick the freshest available "last updated" signal for a feed.
///
/// Combines the feed-level timestamp (RSS `<lastBuildDate>` / Atom `<updated>`),
/// the newest entry date, and the HTTP `Last-Modified` header, returning the most
/// recent one. The HTTP header guards against feeds whose in-feed dates are stale
/// or bogus (e.g. a frozen `<lastBuildDate>` on a feed with no entries).
fn effective_feed_updated_at(
    feed_timestamp: Option<chrono::DateTime<Utc>>,
    latest_entry_date: Option<chrono::DateTime<Utc>>,
    http_last_modified: Option<chrono::DateTime<Utc>>,
) -> Option<chrono::DateTime<Utc>> {
    [feed_timestamp, latest_entry_date, http_last_modified]
        .into_iter()
        .flatten()
        .max()
}

pub async fn refresh_feed(
    db: DbPool,
    feed_id: i64,
    default_user_agent: &str,
) -> AppResult<SyncResult> {
    let feed_data = db
        .background(move |conn| feed::find_by_id(conn, feed_id))
        .await??
        .ok_or(AppError::FeedNotFound)?;

    // Use per-feed custom user agent if set, otherwise use global default
    let effective_user_agent = feed_data
        .custom_user_agent
        .as_deref()
        .unwrap_or(default_user_agent);

    // Per-feed opt-out of HTTP/2 needs the HTTP/1.1-only pooled client; all
    // other feeds share the default pooled client. `http1_only()` is a
    // client-level setting and cannot be applied per request, which is why this
    // one knob still selects between two shared clients.
    let client = if feed_data.http2_disabled {
        &*SHARED_CLIENT_H1
    } else {
        &*SHARED_CLIENT
    };

    let mut headers = HeaderMap::new();

    // User-Agent (possibly a per-feed override) is sent per request now that the
    // client is shared rather than rebuilt with `.user_agent(...)` each call.
    if let Ok(value) = HeaderValue::from_str(effective_user_agent) {
        headers.insert(USER_AGENT, value);
    }

    if let Some(ref etag) = feed_data.etag
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        headers.insert(IF_NONE_MATCH, value);
    }

    if let Some(ref last_modified) = feed_data.last_modified
        && let Ok(value) = HeaderValue::from_str(last_modified)
    {
        headers.insert(IF_MODIFIED_SINCE, value);
    }

    let retry_config = RetryConfig::default();
    let response = match send_with_retry_on_error(&retry_config, || {
        client.get(&feed_data.url).headers(headers.clone())
    })
    .await
    {
        Ok(resp) => resp,
        Err(e) => {
            let error_msg = e.to_string();
            let err_clone = error_msg.clone();
            let _ = db
                .background(move |conn| {
                    feed::update_fetch_result(
                        conn,
                        feed_id,
                        Utc::now(),
                        Some(&err_clone),
                        None,
                        None,
                        None,
                    )
                })
                .await;
            return Err(AppError::FetchError(error_msg));
        }
    };

    let status = response.status();

    // Handle 304 Not Modified
    if status == reqwest::StatusCode::NOT_MODIFIED {
        debug!("Feed {} not modified (304)", feed_id);
        let etag = feed_data.etag.clone();
        let last_modified = feed_data.last_modified.clone();
        db.background(move |conn| {
            feed::update_fetch_result(
                conn,
                feed_id,
                Utc::now(),
                None,
                etag.as_deref(),
                last_modified.as_deref(),
                None,
            )
        })
        .await??;
        return Ok(SyncResult {
            new_entries: 0,
            updated_entries: 0,
        });
    }

    if !status.is_success() {
        let error_msg = format!("HTTP {}", status);
        let err_clone = error_msg.clone();
        db.background(move |conn| {
            feed::update_fetch_result(
                conn,
                feed_id,
                Utc::now(),
                Some(&err_clone),
                None,
                None,
                None,
            )
        })
        .await??;
        return Err(AppError::FetchError(error_msg));
    }

    // Extract headers before consuming response
    let new_etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let new_last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let body = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            let error_msg = e.to_string();
            let err_clone = error_msg.clone();
            let _ = db
                .background(move |conn| {
                    feed::update_fetch_result(
                        conn,
                        feed_id,
                        Utc::now(),
                        Some(&err_clone),
                        None,
                        None,
                        None,
                    )
                })
                .await;
            return Err(AppError::FetchError(error_msg));
        }
    };

    // Parse feed with custom timestamp parser for Chinese date support
    // Note: Parser is not Send, so we must drop it before any .await
    let parse_result = {
        let parser = feed_rs::parser::Builder::new()
            .timestamp_parser(parse_timestamp)
            .build();
        parser.parse(body.as_bytes())
    };

    let parsed_feed = match parse_result {
        Ok(feed) => feed,
        Err(e) => {
            let error_msg = e.to_string();
            let err_clone = error_msg.clone();
            let _ = db
                .background(move |conn| {
                    feed::update_fetch_result(
                        conn,
                        feed_id,
                        Utc::now(),
                        Some(&err_clone),
                        None,
                        None,
                        None,
                    )
                })
                .await;
            return Err(AppError::FeedParseError(error_msg));
        }
    };

    // Extract icon URLs before consuming parsed_feed
    let icon_url = parsed_feed.icon.as_ref().map(|i| i.uri.clone());
    let logo_url = parsed_feed.logo.as_ref().map(|l| l.uri.clone());

    // Check if icon refresh is needed
    let needs_icon_refresh = db
        .background(move |conn| image::needs_refresh(conn, image::ENTITY_FEED, feed_id, 7))
        .await??;

    // Fetch icon if needed (every 7 days)
    if needs_icon_refresh {
        match icon_fetcher::fetch_feed_icon(
            icon_url.as_deref(),
            logo_url.as_deref(),
            feed_data.site_url.as_deref(),
            effective_user_agent,
        )
        .await
        {
            Ok(Some(fetched)) => {
                let source_url = fetched.source_url.clone();
                let save_result = db
                    .background(move |conn| {
                        image::upsert(
                            conn,
                            image::ENTITY_FEED,
                            feed_id,
                            &fetched.data,
                            &fetched.content_type,
                            Some(&fetched.source_url),
                        )
                    })
                    .await;
                match save_result {
                    Ok(Ok(())) => {
                        debug!("Saved icon for feed {} from {}", feed_id, source_url);
                    }
                    Ok(Err(e)) => {
                        warn!("Failed to save icon for feed {}: {}", feed_id, e);
                    }
                    Err(e) => {
                        warn!("Failed to save icon for feed {}: {}", feed_id, e);
                    }
                }
            }
            Ok(None) => {
                debug!("No icon found for feed {}", feed_id);
            }
            Err(e) => {
                warn!("Failed to fetch icon for feed {}: {}", feed_id, e);
            }
        }
    }

    // Extract feed-level timestamp as fallback for entries without dates
    let feed_timestamp = parsed_feed
        .updated
        .or(parsed_feed.published)
        .map(|dt| dt.with_timezone(&Utc));

    // Parse the HTTP Last-Modified header as an additional freshness signal.
    // Some feeds report a stale/bogus in-feed date (e.g. a frozen <lastBuildDate>
    // on a feed with no entries) while the server still serves a recent
    // Last-Modified; without this the feed's "last updated" would be misjudged.
    let http_last_modified = new_last_modified.as_deref().and_then(parse_timestamp);

    let (new_entries, updated_entries, skipped_entries) = db
        .background(move |conn| {
            let mut new_entries = 0i64;
            let mut updated_entries = 0i64;
            let mut skipped_entries = 0i64;
            let mut latest_entry_date: Option<chrono::DateTime<Utc>> = None;

            // Wrap the whole feed's upserts plus the fetch-result update in one
            // transaction: collapses N per-entry commits into a single commit
            // and makes each sync atomic (the read connection never observes a
            // half-applied feed). `unchecked_transaction` because the actor
            // closure only receives `&Connection`.
            let tx = conn.unchecked_transaction()?;

            for item in parsed_feed.entries {
                let guid = item.id;

                let title = item.title.map(|t| t.content);

                let link = item.links.first().map(|l| l.href.clone());

                let content = item
                    .content
                    .and_then(|c| c.body)
                    .or_else(|| item.summary.clone().map(|s| s.content));

                let summary = item.summary.map(|s| s.content);

                let author = item.authors.first().map(|a| a.name.clone());

                // Use published date, fall back to updated date, then feed timestamp
                // If no date is available, use None so sorting falls back to created_at
                let published_at = item
                    .published
                    .or(item.updated)
                    .map(|dt| dt.with_timezone(&Utc))
                    .or(feed_timestamp);

                // Track the latest entry date for feed_updated_at
                if let Some(dt) = published_at {
                    latest_entry_date = Some(match latest_entry_date {
                        Some(current) if current > dt => current,
                        _ => dt,
                    });
                }

                match entry::upsert_entry_id(
                    &tx,
                    feed_id,
                    &guid,
                    title.as_deref(),
                    link.as_deref(),
                    content.as_deref(),
                    summary.as_deref(),
                    author.as_deref(),
                    published_at,
                )? {
                    entry::UpsertOutcome::Inserted(_) => new_entries += 1,
                    entry::UpsertOutcome::Updated(_) => updated_entries += 1,
                    entry::UpsertOutcome::SkippedTombstoned => skipped_entries += 1,
                }
            }

            // Use the most recent of feed-level timestamp, latest entry date, and
            // the HTTP Last-Modified header.
            let effective_updated_at =
                effective_feed_updated_at(feed_timestamp, latest_entry_date, http_last_modified);

            feed::update_fetch_result(
                &tx,
                feed_id,
                Utc::now(),
                None,
                new_etag.as_deref(),
                new_last_modified.as_deref(),
                effective_updated_at,
            )?;

            tx.commit()?;

            Ok::<_, AppError>((new_entries, updated_entries, skipped_entries))
        })
        .await??;

    info!(
        "Feed {} refreshed: {} new, {} updated, {} skipped (tombstoned)",
        feed_id, new_entries, updated_entries, skipped_entries
    );

    Ok(SyncResult {
        new_entries,
        updated_entries,
    })
}

pub async fn refresh_bucket(
    db: DbPool,
    bucket: u8,
    user_agent: &str,
) -> Vec<(i64, Result<SyncResult, String>)> {
    let feeds = match db
        .background(move |conn| feed::list_by_bucket(conn, bucket))
        .await
    {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
            error!("Failed to list feeds for bucket {}: {}", bucket, e);
            return vec![];
        }
        Err(e) => {
            error!("Failed to access DB for bucket {}: {}", bucket, e);
            return vec![];
        }
    };

    if feeds.is_empty() {
        debug!("No feeds in bucket {}", bucket);
        return vec![];
    }

    info!("Refreshing {} feeds in bucket {}", feeds.len(), bucket);

    let mut results = Vec::new();
    let concurrency_limit = 4;

    for chunk in feeds.chunks(concurrency_limit) {
        let mut set = tokio::task::JoinSet::new();

        for feed_data in chunk {
            let db = db.clone();
            let ua = user_agent.to_string();
            let feed_id = feed_data.id;
            set.spawn(async move {
                let result =
                    tokio::time::timeout(FEED_SYNC_TIMEOUT, refresh_feed(db, feed_id, &ua)).await;
                (feed_id, result)
            });
        }

        while let Some(join_result) = set.join_next().await {
            match join_result {
                Ok((feed_id, Ok(inner))) => {
                    match &inner {
                        Ok(sync) => {
                            debug!(
                                "Feed {} synced: {} new, {} updated",
                                feed_id, sync.new_entries, sync.updated_entries
                            );
                        }
                        Err(e) => {
                            warn!("Feed {} sync failed: {}", feed_id, e);
                        }
                    }
                    results.push((feed_id, inner.map_err(|e| e.to_string())));
                }
                Ok((feed_id, Err(_))) => {
                    warn!(
                        "Feed {} sync timed out after {:?}",
                        feed_id, FEED_SYNC_TIMEOUT
                    );
                    results.push((
                        feed_id,
                        Err(format!(
                            "Feed sync timed out after {}s",
                            FEED_SYNC_TIMEOUT.as_secs()
                        )),
                    ));
                }
                Err(e) => {
                    error!("Feed sync task panicked: {}", e);
                }
            }
        }
    }

    // A full bucket sync fetches and parses many feeds concurrently, leaving
    // transient per-feed buffers behind. Reclaim them now so steady-state RSS
    // does not creep up over long uptime.
    crate::reclaim_memory();

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DbPool, init_db};
    use crate::error::AppError;
    use crate::models::entry;
    use crate::models::user::Role;
    use crate::models::{category, feed, user};
    use crate::utils::datetime::normalize_timezone_format;
    use chrono::{Datelike, Timelike};
    use rusqlite::Connection;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---------------------------------------------------------------------------
    // Test infrastructure
    // ---------------------------------------------------------------------------

    /// Open a named shared-memory SQLite connection. Both write and read
    /// connections must use the same name so the pool sees one database.
    fn open_shared_memory(name: &str) -> Connection {
        let uri = format!("file:{}?mode=memory&cache=shared", name);
        Connection::open_with_flags(
            uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .unwrap()
    }

    /// Build a fully-initialized DbPool backed by a shared in-memory database.
    /// Returns `(pool, handle)` — the caller **must** keep `handle` alive for
    /// the duration of the test, otherwise the actor stops and every pool
    /// operation returns `DbError::ActorStopped`.
    fn seeded_pool(name: &str) -> (DbPool, tokio::task::JoinHandle<()>) {
        let write_conn = open_shared_memory(name);
        init_db(&write_conn).unwrap();
        let read_conn = open_shared_memory(name);
        DbPool::new(write_conn, read_conn)
    }

    /// Seed one user → category → feed whose URL points at `url`.
    /// Returns the feed id.
    async fn seed_feed(pool: &DbPool, url: &str) -> i64 {
        let url = url.to_string();
        pool.user(move |conn| {
            let u = user::create_user(conn, "syncuser", "hash", Role::User).unwrap();
            let cat = category::create_category(conn, u.id, "Tech").unwrap();
            feed::create_feed(
                conn,
                &feed::CreateFeedParams {
                    category_id: cat.id,
                    url: &url,
                    title: Some("F"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap()
            .id
        })
        .await
        .unwrap()
    }

    /// Like `seed_feed` but allows a custom user agent override.
    async fn seed_feed_with_ua(pool: &DbPool, url: &str, custom_user_agent: &str) -> i64 {
        let url = url.to_string();
        let ua = custom_user_agent.to_string();
        pool.user(move |conn| {
            let u = user::create_user(conn, "uauser", "hash", Role::User).unwrap();
            let cat = category::create_category(conn, u.id, "Tech").unwrap();
            feed::create_feed(
                conn,
                &feed::CreateFeedParams {
                    category_id: cat.id,
                    url: &url,
                    title: Some("F"),
                    description: None,
                    site_url: None,
                    custom_user_agent: Some(&ua),
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap()
            .id
        })
        .await
        .unwrap()
    }

    /// Minimal two-item RSS fixture — no `<icon>` or `<logo>` elements so the
    /// icon fetcher is never triggered. `site_url: None` on the feed achieves
    /// the same for the favicon fallback.
    const RSS_TWO: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
  <item><guid>g1</guid><title>One</title><link>https://e/1</link><description>c1</description>
        <pubDate>Tue, 10 Jun 2025 10:00:00 GMT</pubDate></item>
  <item><guid>g2</guid><title>Two</title><link>https://e/2</link><description>c2</description>
        <pubDate>Tue, 10 Jun 2025 11:00:00 GMT</pubDate></item>
</channel></rss>"#;

    /// Same guids as RSS_TWO but with changed descriptions, to drive the
    /// "updated entries" path.
    const RSS_TWO_UPDATED: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
  <item><guid>g1</guid><title>One</title><link>https://e/1</link><description>c1-v2</description>
        <pubDate>Tue, 10 Jun 2025 10:00:00 GMT</pubDate></item>
  <item><guid>g2</guid><title>Two</title><link>https://e/2</link><description>c2-v2</description>
        <pubDate>Tue, 10 Jun 2025 11:00:00 GMT</pubDate></item>
</channel></rss>"#;

    // ---------------------------------------------------------------------------
    // Happy-path: new entries are inserted
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn refresh_feed_inserts_new_entries() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_happy");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(result.new_entries, 2);
        assert_eq!(result.updated_entries, 0);
    }

    // ---------------------------------------------------------------------------
    // Feed not found
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn feed_not_found() {
        let (pool, _handle) = seeded_pool("feed_sync_not_found");
        let err = refresh_feed(pool, 999_999, "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FeedNotFound),
            "expected FeedNotFound, got {err:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Existing entries are updated on second sync
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn updates_existing_entries() {
        let server = MockServer::start().await;

        // First request: original RSS
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Second request: same guids, changed descriptions
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO_UPDATED),
            )
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_update");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let first = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(first.new_entries, 2);

        let second = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(second.new_entries, 0);
        assert_eq!(second.updated_entries, 2);
    }

    // ---------------------------------------------------------------------------
    // Tombstoned guids are skipped
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn skips_tombstoned_guid() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_tombstone");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        // Tombstone g1 before the sync
        pool.user(move |conn| entry::insert_tombstone(conn, feed_id, "g1").unwrap())
            .await
            .unwrap();

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(result.new_entries, 1, "g1 should be skipped");

        // Verify g1 was not inserted
        let found = pool
            .user(move |conn| entry::find_by_guid_and_feed(conn, "g1", feed_id).unwrap())
            .await
            .unwrap();
        assert!(found.is_none(), "g1 must not exist (tombstoned)");
    }

    // ---------------------------------------------------------------------------
    // 304 Not Modified returns zero counts
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn not_modified_304() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_304");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(result.new_entries, 0);
        assert_eq!(result.updated_entries, 0);
    }

    // ---------------------------------------------------------------------------
    // HTTP 4xx/5xx persists a fetch error on the feed row
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn http_error_persists_fetch_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_fetch_err");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FetchError(_)),
            "expected FetchError, got {err:?}"
        );

        // Fetch error must have been written to the feed row
        let fetch_error = pool
            .user(move |conn| {
                feed::find_by_id(conn, feed_id)
                    .unwrap()
                    .unwrap()
                    .fetch_error
            })
            .await
            .unwrap();
        assert!(
            fetch_error.is_some(),
            "feed.fetch_error should be populated after HTTP error"
        );
    }

    // ---------------------------------------------------------------------------
    // Malformed XML returns a parse error
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn malformed_xml_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string("<rss><broken"),
            )
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_parse_err");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FeedParseError(_)),
            "expected FeedParseError, got {err:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Conditional-GET headers are sent when etag/last_modified are set
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn sends_conditional_get_headers() {
        let server = MockServer::start().await;

        // The mock verifies that If-None-Match is included; `.expect(1)` makes
        // wiremock fail the test if the header is never seen.
        Mock::given(method("GET"))
            .and(header("if-none-match", "\"abc123\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1)
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_cond_get");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        // Write etag + last_modified directly onto the feed row
        pool.user(move |conn| {
            feed::update_fetch_result(
                conn,
                feed_id,
                chrono::Utc::now(),
                None,
                Some("\"abc123\""),
                Some("Mon, 09 Jun 2025 00:00:00 GMT"),
                None,
            )
            .unwrap()
        })
        .await
        .unwrap();

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0")
            .await
            .unwrap();
        assert_eq!(result.new_entries, 0);
        // wiremock verifies the expectation on server drop
    }

    // ---------------------------------------------------------------------------
    // Per-feed custom User-Agent is sent
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn uses_custom_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("user-agent", "Custom/9"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .expect(1)
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_custom_ua");
        let feed_id = seed_feed_with_ua(&pool, &server.uri(), "Custom/9").await;

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Default/1.0")
            .await
            .unwrap();
        assert_eq!(result.new_entries, 2);
        // wiremock verifies the User-Agent expectation on server drop
    }

    // ---------------------------------------------------------------------------
    // refresh_bucket: empty bucket returns empty vec
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn refresh_bucket_empty() {
        let (pool, _handle) = seeded_pool("feed_sync_bucket_empty");
        // Use bucket 255 — no feeds hashed there in our empty DB
        let results = refresh_bucket(pool, 255, "RDRS-Test/1.0").await;
        assert!(results.is_empty());
    }

    // ---------------------------------------------------------------------------
    // refresh_bucket: feeds in the bucket are synced
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn refresh_bucket_runs_feeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .mount(&server)
            .await;

        let (pool, _handle) = seeded_pool("feed_sync_bucket_feeds");
        let feed_id = seed_feed(&pool, &server.uri()).await;

        // Discover which bucket the seeded feed was assigned
        let bucket = pool
            .user(move |conn| feed::find_by_id(conn, feed_id).unwrap().unwrap().bucket)
            .await
            .unwrap()
            .expect("bucket should be set on create") as u8;

        let results = refresh_bucket(pool, bucket, "RDRS-Test/1.0").await;

        assert!(!results.is_empty(), "expected at least one result");
        let matching = results.iter().find(|(id, _)| *id == feed_id);
        assert!(matching.is_some(), "seeded feed should appear in results");
        let (_, outcome) = matching.unwrap();
        let sync = outcome.as_ref().expect("sync should succeed");
        assert!(
            sync.new_entries > 0,
            "expected new entries from bucket sync"
        );
    }

    #[test]
    fn test_normalize_timezone_format() {
        // Should convert +08:00 to +0800
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +08:00"),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );

        // Should convert -05:30 to -0530
        assert_eq!(
            normalize_timezone_format("Mon, 01 Jan 2026 12:00:00 -05:30"),
            "Mon, 01 Jan 2026 12:00:00 -0530"
        );

        // Should leave already correct format unchanged
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +0800"),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );

        // Should handle trailing whitespace
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +08:00  "),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );
    }

    #[test]
    fn test_parse_timestamp_colon_timezone() {
        // This format was previously failing
        let result = parse_timestamp("Thu, 22 Jan 2026 15:09:47 +08:00");
        assert!(
            result.is_some(),
            "Should parse RFC2822-like format with colon timezone"
        );

        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 22);
        // The time should be converted to UTC (15:09:47 +08:00 = 07:09:47 UTC)
        assert_eq!(dt.hour(), 7);
        assert_eq!(dt.minute(), 9);
    }

    #[test]
    fn test_parse_timestamp_various_formats() {
        // Standard RFC2822
        assert!(parse_timestamp("Thu, 22 Jan 2026 15:09:47 +0800").is_some());

        // ISO 8601 / RFC 3339
        assert!(parse_timestamp("2026-01-22T15:09:47+08:00").is_some());

        // Chinese format
        assert!(parse_timestamp("週四, 22 一月 2026 15:09:47 +0800").is_some());
    }

    #[test]
    fn test_parse_timestamp_http_last_modified() {
        // HTTP-date (RFC 7231 IMF-fixdate) uses the "GMT" zone name
        let result = parse_timestamp("Mon, 01 Jun 2026 17:46:41 GMT");
        assert!(
            result.is_some(),
            "Should parse HTTP-date Last-Modified format"
        );
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 6);
        assert_eq!(dt.day(), 1);
        assert_eq!(dt.hour(), 17);
    }

    fn dt(y: i32, mo: u32, d: u32) -> chrono::DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn test_effective_feed_updated_at_uses_http_last_modified_when_feed_date_is_stale() {
        // Regression: blocktempo serves lastBuildDate=2019 with no entries, but the
        // HTTP Last-Modified header is recent. The freshest signal must win.
        let feed_ts = Some(dt(2019, 1, 22));
        let latest_entry = None;
        let http_lm = Some(dt(2026, 6, 1));
        assert_eq!(
            effective_feed_updated_at(feed_ts, latest_entry, http_lm),
            Some(dt(2026, 6, 1))
        );
    }

    #[test]
    fn test_effective_feed_updated_at_picks_maximum() {
        assert_eq!(
            effective_feed_updated_at(
                Some(dt(2026, 3, 1)),
                Some(dt(2026, 5, 1)),
                Some(dt(2026, 4, 1))
            ),
            Some(dt(2026, 5, 1))
        );
    }

    #[test]
    fn test_effective_feed_updated_at_all_none() {
        assert_eq!(effective_feed_updated_at(None, None, None), None);
    }

    #[test]
    fn test_effective_feed_updated_at_ignores_missing_signals() {
        // Only entry dates present -> use them; no HTTP/feed-level date available.
        assert_eq!(
            effective_feed_updated_at(None, Some(dt(2026, 2, 2)), None),
            Some(dt(2026, 2, 2))
        );
    }
}
