use std::sync::{Arc, Mutex};

use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH};
use rusqlite::Connection;
use serde::Serialize;
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};
use crate::models::{entry, feed};

#[derive(Debug, Clone, Serialize)]
pub struct SyncResult {
    pub new_entries: i64,
    pub updated_entries: i64,
}

pub async fn refresh_feed(db: Arc<Mutex<Connection>>, feed_id: i64) -> AppResult<SyncResult> {
    let feed_data = {
        let conn = db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;
        feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("RDRS Feed Reader/1.0")
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

    let response = client
        .get(&feed_data.url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

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

    let body = response
        .text()
        .await
        .map_err(|e| AppError::FetchError(e.to_string()))?;

    // Parse feed
    let parsed_feed = feed_rs::parser::parse(body.as_bytes())
        .map_err(|e| AppError::FeedParseError(e.to_string()))?;

    let mut new_entries = 0i64;
    let mut updated_entries = 0i64;

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

            let published_at = item
                .published
                .or(item.updated)
                .map(|dt| dt.with_timezone(&Utc));

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
        let result = refresh_feed(db.clone(), feed_data.id).await;
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
