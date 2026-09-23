use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, USER_AGENT};
use serde::Serialize;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::{entry, feed, image};
use crate::services::fetch::Fetcher;
use crate::services::http::{FEED_SYNC_TIMEOUT, RetryConfig, send_with_retry_on_error};
use crate::services::icon_fetcher;
use crate::utils::datetime::parse_timestamp;

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_entries: i64,
    pub updated_entries: i64,
}

/// Pick the freshest "last updated" signal: feed-level timestamp, newest entry
/// date, or HTTP `Last-Modified` (which guards against frozen in-feed dates).
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
    db: Db,
    feed_id: i64,
    default_user_agent: &str,
    fetcher: &Fetcher,
) -> AppResult<SyncResult> {
    let feed_data = feed::find_by_id(&db, feed_id)
        .await?
        .ok_or(AppError::FeedNotFound)?;

    // Re-checked every refresh: older rows and OPML imports bypass the write-time
    // guard.
    let parsed_url = Url::parse(&feed_data.url).map_err(|_e| AppError::InvalidUrl)?;
    if let Err(e) = fetcher.validate(&parsed_url) {
        let error_msg = e.to_string();
        let _ =
            feed::update_fetch_result(&db, feed_id, Utc::now(), Some(&error_msg), None, None, None)
                .await;
        return Err(AppError::InvalidUrl);
    }

    let effective_user_agent = feed_data
        .custom_user_agent
        .as_deref()
        .unwrap_or(default_user_agent);

    // `http1_only()` is client-level, so the per-feed HTTP/2 opt-out selects a
    // separate shared client.
    let client = fetcher.client(feed_data.http2_disabled);

    let mut headers = HeaderMap::new();

    // UA is per request since the client is shared.
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
            let _ = feed::update_fetch_result(
                &db,
                feed_id,
                Utc::now(),
                Some(&error_msg),
                None,
                None,
                None,
            )
            .await;
            return Err(AppError::FetchError(error_msg));
        }
    };

    let status = response.status();

    if status == reqwest::StatusCode::NOT_MODIFIED {
        debug!(
            event = "feed.not_modified",
            feed_id, "feed not modified (304)"
        );
        feed::update_fetch_result(
            &db,
            feed_id,
            Utc::now(),
            None,
            feed_data.etag.as_deref(),
            feed_data.last_modified.as_deref(),
            None,
        )
        .await?;
        return Ok(SyncResult {
            new_entries: 0,
            updated_entries: 0,
        });
    }

    if !status.is_success() {
        let error_msg = format!("HTTP {status}");
        feed::update_fetch_result(&db, feed_id, Utc::now(), Some(&error_msg), None, None, None)
            .await?;
        return Err(AppError::FetchError(error_msg));
    }

    // Before `response` is consumed by the body read below.
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
            let _ = feed::update_fetch_result(
                &db,
                feed_id,
                Utc::now(),
                Some(&error_msg),
                None,
                None,
                None,
            )
            .await;
            return Err(AppError::FetchError(error_msg));
        }
    };

    // Custom timestamp parser for Chinese dates. Parser is not Send; drop it
    // before any .await.
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
            let _ = feed::update_fetch_result(
                &db,
                feed_id,
                Utc::now(),
                Some(&error_msg),
                None,
                None,
                None,
            )
            .await;
            return Err(AppError::FeedParseError(error_msg));
        }
    };

    // Before `parsed_feed` is consumed below.
    let icon_url = parsed_feed.icon.as_ref().map(|i| i.uri.clone());
    let logo_url = parsed_feed.logo.as_ref().map(|l| l.uri.clone());

    let needs_icon_refresh = image::needs_refresh(&db, image::ENTITY_FEED, feed_id, 7).await?;

    // Fetch icon if needed (every 7 days).
    if needs_icon_refresh {
        match icon_fetcher::fetch_feed_icon(
            icon_url.as_deref(),
            logo_url.as_deref(),
            feed_data.site_url.as_deref(),
            effective_user_agent,
            fetcher,
        )
        .await
        {
            Ok(Some(fetched)) => {
                let source_url = fetched.source_url.clone();
                let save_result = image::upsert(
                    &db,
                    image::ENTITY_FEED,
                    feed_id,
                    &fetched.data,
                    &fetched.content_type,
                    Some(&fetched.source_url),
                )
                .await;
                match save_result {
                    Ok(()) => {
                        debug!(
                            event = "feed.icon_saved",
                            feed_id,
                            url = source_url,
                            "saved feed icon"
                        );
                    }
                    Err(e) => {
                        warn!(event = "feed.icon_save_failed", feed_id, error = %e, "failed to save feed icon");
                    }
                }
            }
            Ok(None) => {
                debug!(
                    event = "feed.icon_missing",
                    feed_id, "no icon found for feed"
                );
            }
            Err(e) => {
                warn!(event = "feed.icon_fetch_failed", feed_id, error = %e, "failed to fetch feed icon");
            }
        }
    }

    let feed_timestamp = parsed_feed
        .updated
        .or(parsed_feed.published)
        .map(|dt| dt.with_timezone(&Utc));

    // HTTP Last-Modified catches feeds with stale in-feed dates.
    let http_last_modified = new_last_modified.as_deref().and_then(parse_timestamp);

    // One transaction per feed: a single commit, and readers never see a
    // half-applied sync.
    let (new_entries, updated_entries, unchanged_entries, skipped_entries) = {
        let mut new_entries = 0i64;
        let mut updated_entries = 0i64;
        let mut unchanged_entries = 0i64;
        let mut skipped_entries = 0i64;
        let mut latest_entry_date: Option<chrono::DateTime<Utc>> = None;

        let mut tx = db.begin().await?;

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

            // published, then updated, then feed timestamp; None falls back to created_at.
            let published_at = item
                .published
                .or(item.updated)
                .map(|dt| dt.with_timezone(&Utc))
                .or(feed_timestamp);

            if let Some(dt) = published_at {
                latest_entry_date = Some(match latest_entry_date {
                    Some(current) if current > dt => current,
                    _ => dt,
                });
            }

            match entry::upsert_entry_id_tx(
                &mut tx,
                feed_id,
                &guid,
                title.as_deref(),
                link.as_deref(),
                content.as_deref(),
                summary.as_deref(),
                author.as_deref(),
                published_at,
            )
            .await?
            {
                entry::UpsertOutcome::Inserted(_) => new_entries += 1,
                entry::UpsertOutcome::Updated(_) => updated_entries += 1,
                entry::UpsertOutcome::Unchanged(_) => unchanged_entries += 1,
                entry::UpsertOutcome::SkippedTombstoned => skipped_entries += 1,
            }
        }

        let effective_updated_at =
            effective_feed_updated_at(feed_timestamp, latest_entry_date, http_last_modified);

        feed::update_fetch_result_tx(
            &mut tx,
            feed_id,
            Utc::now(),
            None,
            new_etag.as_deref(),
            new_last_modified.as_deref(),
            effective_updated_at,
        )
        .await?;

        tx.commit().await?;

        (
            new_entries,
            updated_entries,
            unchanged_entries,
            skipped_entries,
        )
    };

    info!(
        event = "feed.refreshed",
        feed_id, new_entries, updated_entries, unchanged_entries, skipped_entries, "feed refreshed"
    );

    Ok(SyncResult {
        new_entries,
        updated_entries,
    })
}

pub async fn refresh_bucket(
    db: Db,
    bucket: u8,
    user_agent: &str,
    fetcher: &Fetcher,
) -> Vec<(i64, Result<SyncResult, String>)> {
    let feeds = match feed::list_by_bucket(&db, bucket).await {
        Ok(f) => f,
        Err(e) => {
            error!(event = "sync.bucket_list_failed", bucket, error = %e, "failed to list feeds for bucket");
            return vec![];
        }
    };

    if feeds.is_empty() {
        debug!(event = "sync.bucket_empty", bucket, "no feeds in bucket");
        return vec![];
    }

    info!(
        event = "sync.bucket_started",
        bucket,
        count = feeds.len(),
        "refreshing feeds in bucket"
    );

    let mut results = Vec::new();
    let concurrency_limit = 4;

    for chunk in feeds.chunks(concurrency_limit) {
        let mut set = tokio::task::JoinSet::new();

        for feed_data in chunk {
            let db = db.clone();
            let ua = user_agent.to_string();
            let fetcher = fetcher.clone();
            let feed_id = feed_data.id;
            set.spawn(async move {
                let result = tokio::time::timeout(
                    FEED_SYNC_TIMEOUT,
                    refresh_feed(db, feed_id, &ua, &fetcher),
                )
                .await;
                (feed_id, result)
            });
        }

        while let Some(join_result) = set.join_next().await {
            match join_result {
                Ok((feed_id, Ok(inner))) => {
                    match &inner {
                        Ok(sync) => {
                            debug!(
                                event = "feed.synced",
                                feed_id,
                                new_entries = sync.new_entries,
                                updated_entries = sync.updated_entries,
                                "feed synced"
                            );
                        }
                        Err(e) => {
                            warn!(event = "feed.sync_failed", feed_id, error = %e, "feed sync failed");
                        }
                    }
                    results.push((feed_id, inner.map_err(|e| e.to_string())));
                }
                Ok((feed_id, Err(_))) => {
                    warn!(
                        event = "feed.sync_timeout",
                        feed_id,
                        timeout_s = FEED_SYNC_TIMEOUT.as_secs(),
                        "feed sync timed out"
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
                    error!(event = "sync.task_panicked", error = %e, "feed sync task panicked");
                }
            }
        }
    }

    // Reclaim transient per-feed buffers so RSS does not creep over long uptime.
    crate::reclaim_memory();

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::error::AppError;
    use crate::models::entry;
    use crate::models::user::Role;
    use crate::models::{category, feed};
    use crate::test_support::seed_user;
    use crate::utils::url_validation::FetchPolicy;

    /// wiremock binds loopback, so allow it the same way a deployment opts in a
    /// LAN feed.
    fn loopback_fetcher() -> Fetcher {
        Fetcher::new(FetchPolicy::parse("127.0.0.1").expect("valid allow list"))
            .expect("the guarded client must build")
    }
    use chrono::{Datelike, Timelike};
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Isolated in-memory `Db`; `_name` is unused.
    async fn seeded_pool(_name: &str) -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    /// Seed user → category → feed pointing at `url`; returns the feed id.
    async fn seed_feed(pool: &Db, url: &str) -> i64 {
        let u = seed_user(pool, "syncuser", Role::User).await;
        let cat = category::create_category(pool, u.id, "Tech").await.unwrap();
        feed::create_feed(
            pool,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url,
                title: Some("F"),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_feed_with_ua(pool: &Db, url: &str, custom_user_agent: &str) -> i64 {
        let u = seed_user(pool, "uauser", Role::User).await;
        let cat = category::create_category(pool, u.id, "Tech").await.unwrap();
        feed::create_feed(
            pool,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url,
                title: Some("F"),
                custom_user_agent: Some(custom_user_agent),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    /// Two-item RSS with no `<icon>`/`<logo>`, so the icon fetcher never runs.
    const RSS_TWO: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
  <item><guid>g1</guid><title>One</title><link>https://e/1</link><description>c1</description>
        <pubDate>Tue, 10 Jun 2025 10:00:00 GMT</pubDate></item>
  <item><guid>g2</guid><title>Two</title><link>https://e/2</link><description>c2</description>
        <pubDate>Tue, 10 Jun 2025 11:00:00 GMT</pubDate></item>
</channel></rss>"#;

    /// `RSS_TWO` guids with changed descriptions, for the update path.
    const RSS_TWO_UPDATED: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
  <item><guid>g1</guid><title>One</title><link>https://e/1</link><description>c1-v2</description>
        <pubDate>Tue, 10 Jun 2025 10:00:00 GMT</pubDate></item>
  <item><guid>g2</guid><title>Two</title><link>https://e/2</link><description>c2-v2</description>
        <pubDate>Tue, 10 Jun 2025 11:00:00 GMT</pubDate></item>
</channel></rss>"#;

    /// Stored URLs are checked every refresh, covering legacy and OPML-imported
    /// rows.
    #[tokio::test]
    async fn refresh_feed_refuses_a_private_url_the_policy_does_not_allow() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(RSS_TWO))
            .expect(0)
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_blocked_url").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &Fetcher::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidUrl));

        // Recorded as a fetch failure so the feed does not look healthy.
        let stored = feed::find_by_id(&pool, feed_id).await.unwrap().unwrap();
        assert!(stored.fetch_error.is_some());
    }

    /// The feed URL is fine; only the redirect points inward, so the client must
    /// catch it.
    #[tokio::test]
    async fn refresh_feed_does_not_follow_a_redirect_into_a_blocked_range() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_blocked_redirect").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::FetchError(_)), "got: {err:?}");

        // Without the guard this still fails, but only after connecting and timing
        // out.
        let stored = feed::find_by_id(&pool, feed_id).await.unwrap().unwrap();
        let recorded = stored.fetch_error.unwrap_or_default();
        assert!(recorded.contains("redirect"), "got: {recorded}");

        let imported: i64 = crate::query_scalar!(
            &pool,
            i64,
            "SELECT COUNT(*) FROM entry WHERE feed_id = $1",
            feed_id
        )
        .unwrap();
        assert_eq!(imported, 0, "a refused fetch must not import anything");
    }

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

        let pool = seeded_pool("feed_sync_happy").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(result.new_entries, 2);
        assert_eq!(result.updated_entries, 0);
    }

    #[tokio::test]
    async fn feed_not_found() {
        let pool = seeded_pool("feed_sync_not_found").await;
        let err = refresh_feed(pool, 999_999, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FeedNotFound),
            "expected FeedNotFound, got {err:?}"
        );
    }

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

        let pool = seeded_pool("feed_sync_update").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let first = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(first.new_entries, 2);

        let second = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(second.new_entries, 0);
        assert_eq!(second.updated_entries, 2);
    }

    /// Byte-identical re-served content (no 304 shortcut) must report zero
    /// updates: the guarded UPDATE writes nothing.
    #[tokio::test]
    async fn resync_of_identical_content_updates_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(RSS_TWO),
            )
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_identical").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let first = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(first.new_entries, 2);

        let second = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(second.new_entries, 0);
        assert_eq!(
            second.updated_entries, 0,
            "identical content must not rewrite rows"
        );
    }

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

        let pool = seeded_pool("feed_sync_tombstone").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        // Tombstone g1 before the sync
        entry::insert_tombstone(&pool, feed_id, "g1").await.unwrap();

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(result.new_entries, 1, "g1 should be skipped");

        let found = entry::find_by_guid_and_feed(&pool, "g1", feed_id)
            .await
            .unwrap();
        assert!(found.is_none(), "g1 must not exist (tombstoned)");
    }

    #[tokio::test]
    async fn not_modified_304() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_304").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(result.new_entries, 0);
        assert_eq!(result.updated_entries, 0);
    }

    #[tokio::test]
    async fn http_error_persists_fetch_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_fetch_err").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FetchError(_)),
            "expected FetchError, got {err:?}"
        );

        // Fetch error must have been written to the feed row
        let fetch_error = feed::find_by_id(&pool, feed_id)
            .await
            .unwrap()
            .unwrap()
            .fetch_error;
        assert!(
            fetch_error.is_some(),
            "feed.fetch_error should be populated after HTTP error"
        );
    }

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

        let pool = seeded_pool("feed_sync_parse_err").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        let err = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::FeedParseError(_)),
            "expected FeedParseError, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sends_conditional_get_headers() {
        let server = MockServer::start().await;

        // `.expect(1)` fails the test if If-None-Match is never seen.
        Mock::given(method("GET"))
            .and(header("if-none-match", "\"abc123\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1)
            .mount(&server)
            .await;

        let pool = seeded_pool("feed_sync_cond_get").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        feed::update_fetch_result(
            &pool,
            feed_id,
            chrono::Utc::now(),
            None,
            Some("\"abc123\""),
            Some("Mon, 09 Jun 2025 00:00:00 GMT"),
            None,
        )
        .await
        .unwrap();

        let result = refresh_feed(pool.clone(), feed_id, "RDRS-Test/1.0", &loopback_fetcher())
            .await
            .unwrap();
        assert_eq!(result.new_entries, 0);
    }

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

        let pool = seeded_pool("feed_sync_custom_ua").await;
        let feed_id = seed_feed_with_ua(&pool, &server.uri(), "Custom/9").await;

        let result = refresh_feed(
            pool.clone(),
            feed_id,
            "RDRS-Default/1.0",
            &loopback_fetcher(),
        )
        .await
        .unwrap();
        assert_eq!(result.new_entries, 2);
    }

    #[tokio::test]
    async fn refresh_bucket_empty() {
        let pool = seeded_pool("feed_sync_bucket_empty").await;
        // Bucket 255 is empty in a fresh DB.
        let results = refresh_bucket(pool, 255, "RDRS-Test/1.0", &loopback_fetcher()).await;
        assert!(results.is_empty());
    }

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

        let pool = seeded_pool("feed_sync_bucket_feeds").await;
        let feed_id = seed_feed(&pool, &server.uri()).await;

        #[allow(
            clippy::cast_sign_loss,
            reason = "`bucket` is stored as a URL-hash modulo 60, always in 0..=59"
        )]
        let bucket = feed::find_by_id(&pool, feed_id)
            .await
            .unwrap()
            .unwrap()
            .bucket
            .expect("bucket should be set on create") as u8;

        let results = refresh_bucket(pool, bucket, "RDRS-Test/1.0", &loopback_fetcher()).await;

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
        // Regression: stale lastBuildDate, recent Last-Modified; freshest must win.
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
