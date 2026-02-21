use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use serde::Serialize;
use tracing::{debug, error, info, warn};

use crate::db::DbPool;
use crate::error::{AppError, AppResult};
use crate::models::{entry, feed, image};
use crate::services::http::{
    send_with_retry_on_error, RetryConfig, DEFAULT_TIMEOUT, FEED_SYNC_TIMEOUT,
};
use crate::services::icon_fetcher;
use crate::utils::datetime::parse_timestamp;

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_entries: i64,
    pub updated_entries: i64,
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

    // Build HTTP client with per-feed settings
    let mut client_builder = reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .user_agent(effective_user_agent);

    // Disable HTTP/2 if configured for this feed
    if feed_data.http2_disabled {
        client_builder = client_builder.http1_only();
    }

    let client = client_builder
        .build()
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    let mut headers = HeaderMap::new();

    if let Some(ref etag) = feed_data.etag {
        if let Ok(value) = HeaderValue::from_str(etag) {
            headers.insert(IF_NONE_MATCH, value);
        }
    }

    if let Some(ref last_modified) = feed_data.last_modified {
        if let Ok(value) = HeaderValue::from_str(last_modified) {
            headers.insert(IF_MODIFIED_SINCE, value);
        }
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
            feed::update_fetch_result(conn, feed_id, Utc::now(), Some(&err_clone), None, None)
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

    let (new_entries, updated_entries) = db
        .background(move |conn| {
            let mut new_entries = 0i64;
            let mut updated_entries = 0i64;

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

                let (_, is_new) = entry::upsert_entry(
                    conn,
                    feed_id,
                    &guid,
                    title.as_deref(),
                    link.as_deref(),
                    content.as_deref(),
                    summary.as_deref(),
                    author.as_deref(),
                    published_at,
                )?;

                if is_new {
                    new_entries += 1;
                } else {
                    updated_entries += 1;
                }
            }

            // Update feed fetch result
            feed::update_fetch_result(
                conn,
                feed_id,
                Utc::now(),
                None,
                new_etag.as_deref(),
                new_last_modified.as_deref(),
            )?;

            Ok::<_, AppError>((new_entries, updated_entries))
        })
        .await??;

    info!(
        "Feed {} refreshed: {} new, {} updated",
        feed_id, new_entries, updated_entries
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

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::datetime::normalize_timezone_format;
    use chrono::{Datelike, Timelike};

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
}
