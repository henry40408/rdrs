use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use rusqlite::Connection;
use serde::Serialize;
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};
use crate::models::{entry, feed};

/// Parse Chinese month names to month number
fn parse_chinese_month(s: &str) -> Option<u32> {
    match s {
        "一月" => Some(1),
        "二月" => Some(2),
        "三月" => Some(3),
        "四月" => Some(4),
        "五月" => Some(5),
        "六月" => Some(6),
        "七月" => Some(7),
        "八月" => Some(8),
        "九月" => Some(9),
        "十月" => Some(10),
        "十一月" => Some(11),
        "十二月" => Some(12),
        _ => None,
    }
}

/// Parse timezone offset like "+0000", "+0800", "-0500"
fn parse_timezone_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.len() < 5 {
        return None;
    }

    let sign = match s.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };

    let hours: i32 = s[1..3].parse().ok()?;
    let minutes: i32 = s[3..5].parse().ok()?;

    Some(sign * (hours * 3600 + minutes * 60))
}

/// Parse Chinese date format like "週二, 6 一月 2026 14:28:00 +0000"
fn parse_chinese_datetime(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    // Remove weekday prefix if present (e.g., "週二, " or "星期二, ")
    let s = if let Some(pos) = s.find(", ") {
        &s[pos + 2..]
    } else {
        s
    };

    // Expected format: "6 一月 2026 14:28:00 +0000"
    let parts: Vec<&str> = s.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = parse_chinese_month(parts[1])?;
    let year: i32 = parts[2].parse().ok()?;

    // Parse time and timezone: "14:28:00 +0000"
    let time_tz = parts[3];
    let time_parts: Vec<&str> = time_tz.splitn(2, ' ').collect();
    let time_str = time_parts.first()?;

    let time = NaiveTime::parse_from_str(time_str, "%H:%M:%S").ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive_dt = NaiveDateTime::new(date, time);

    // Parse timezone offset if present
    if let Some(tz_str) = time_parts.get(1) {
        if let Some(offset_secs) = parse_timezone_offset(tz_str) {
            let offset = FixedOffset::east_opt(offset_secs)?;
            let dt = naive_dt.and_local_timezone(offset).single()?;
            return Some(dt.with_timezone(&Utc));
        }
    }

    Some(naive_dt.and_utc())
}

/// Custom timestamp parser that handles standard formats plus Chinese dates
fn parse_timestamp_with_chinese(text: &str) -> Option<DateTime<Utc>> {
    // Try standard parsing first (via dateparser)
    dateparser::parse(text)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        // Then try Chinese date format
        .or_else(|| parse_chinese_datetime(text))
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_entries: i64,
    pub updated_entries: i64,
}

pub async fn refresh_feed(
    db: Arc<Mutex<Connection>>,
    feed_id: i64,
    user_agent: &str,
) -> AppResult<SyncResult> {
    let feed_data = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;
        feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(user_agent)
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

    let response = match client.get(&feed_data.url).headers(headers).send().await {
        Ok(resp) => resp,
        Err(e) => {
            let error_msg = e.to_string();
            if let Ok(conn) = db.lock() {
                let _ = feed::update_fetch_result(&conn, feed_id, Utc::now(), Some(&error_msg), None, None);
            }
            return Err(AppError::FetchError(error_msg));
        }
    };

    let status = response.status();

    // Handle 304 Not Modified
    if status == reqwest::StatusCode::NOT_MODIFIED {
        debug!("Feed {} not modified (304)", feed_id);
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;
        feed::update_fetch_result(
            &conn,
            feed_id,
            Utc::now(),
            None,
            feed_data.etag.as_deref(),
            feed_data.last_modified.as_deref(),
        )?;
        return Ok(SyncResult {
            new_entries: 0,
            updated_entries: 0,
        });
    }

    if !status.is_success() {
        let error_msg = format!("HTTP {}", status);
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;
        feed::update_fetch_result(&conn, feed_id, Utc::now(), Some(&error_msg), None, None)?;
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
            if let Ok(conn) = db.lock() {
                let _ = feed::update_fetch_result(&conn, feed_id, Utc::now(), Some(&error_msg), None, None);
            }
            return Err(AppError::FetchError(error_msg));
        }
    };

    // Parse feed with custom timestamp parser for Chinese date support
    let parser = feed_rs::parser::Builder::new()
        .timestamp_parser(parse_timestamp_with_chinese)
        .build();

    let parsed_feed = match parser.parse(body.as_bytes()) {
        Ok(feed) => feed,
        Err(e) => {
            let error_msg = e.to_string();
            if let Ok(conn) = db.lock() {
                let _ = feed::update_fetch_result(&conn, feed_id, Utc::now(), Some(&error_msg), None, None);
            }
            return Err(AppError::FeedParseError(error_msg));
        }
    };

    let mut new_entries = 0i64;
    let mut updated_entries = 0i64;

    // Extract feed-level timestamp as fallback for entries without dates
    let feed_timestamp = parsed_feed
        .updated
        .or(parsed_feed.published)
        .map(|dt| dt.with_timezone(&Utc));

    {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

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

            // Use published date, fall back to updated date, then feed timestamp, then fetch time
            let published_at = Some(
                item.published
                    .or(item.updated)
                    .map(|dt| dt.with_timezone(&Utc))
                    .or(feed_timestamp)
                    .unwrap_or_else(Utc::now),
            );

            let (_, is_new) = entry::upsert_entry(
                &conn,
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
            &conn,
            feed_id,
            Utc::now(),
            None,
            new_etag.as_deref(),
            new_last_modified.as_deref(),
        )?;
    }

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
    db: Arc<Mutex<Connection>>,
    bucket: u8,
    user_agent: &str,
) -> Vec<(i64, Result<SyncResult, String>)> {
    let feeds = {
        let conn = match db.lock() {
            Ok(c) => c,
            Err(_) => {
                error!("Failed to lock DB for bucket {}", bucket);
                return vec![];
            }
        };
        match feed::list_by_bucket(&conn, bucket) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to list feeds for bucket {}: {}", bucket, e);
                return vec![];
            }
        }
    };

    if feeds.is_empty() {
        debug!("No feeds in bucket {}", bucket);
        return vec![];
    }

    info!("Refreshing {} feeds in bucket {}", feeds.len(), bucket);

    let mut results = Vec::new();

    for feed_data in feeds {
        let result = refresh_feed(db.clone(), feed_data.id, user_agent).await;
        match &result {
            Ok(sync) => {
                debug!(
                    "Feed {} synced: {} new, {} updated",
                    feed_data.id, sync.new_entries, sync.updated_entries
                );
            }
            Err(e) => {
                warn!("Feed {} sync failed: {}", feed_data.id, e);
            }
        }
        results.push((feed_data.id, result.map_err(|e| e.to_string())));
    }

    results
}
