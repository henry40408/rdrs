use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_opt, query_scalar};

/// Summary processing status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}

impl SummaryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SummaryStatus::Pending => "pending",
            SummaryStatus::Processing => "processing",
            SummaryStatus::Completed => "completed",
            SummaryStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(SummaryStatus::Pending),
            "processing" => Some(SummaryStatus::Processing),
            "completed" => Some(SummaryStatus::Completed),
            "failed" => Some(SummaryStatus::Failed),
            _ => None,
        }
    }
}

/// Decodes the DB `status` TEXT column into `SummaryStatus`. Unknown values map
/// to `Failed`, matching the old `row_to_entry_summary` behaviour. Used by
/// `#[sqlx(try_from = "String")]` on `EntrySummary::status` (the blanket
/// `TryFrom` supplied by this `From` impl is infallible).
impl From<String> for SummaryStatus {
    fn from(s: String) -> Self {
        SummaryStatus::parse(&s).unwrap_or(SummaryStatus::Failed)
    }
}

/// An entry summary stored in the database
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntrySummary {
    pub id: i64,
    pub user_id: i64,
    pub entry_id: i64,
    #[sqlx(try_from = "String")]
    pub status: SummaryStatus,
    pub summary_text: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn find_by_user_and_entry(
    db: &Db,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntrySummary>> {
    query_opt!(
        db,
        EntrySummary,
        "SELECT id, user_id, entry_id, status, summary_text, error_message, created_at, updated_at \
         FROM entry_summary WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id
    )
    .map_err(AppError::Database)
}

/// Resets an existing row to pending, so a retry after a failure reuses the
/// same record instead of accumulating one per attempt.
pub async fn upsert_pending(db: &Db, user_id: i64, entry_id: i64) -> AppResult<EntrySummary> {
    db_execute!(
        db,
        "INSERT INTO entry_summary (user_id, entry_id, status) \
         VALUES ($1, $2, 'pending') \
         ON CONFLICT(user_id, entry_id) DO UPDATE SET \
             status = 'pending', \
             summary_text = NULL, \
             error_message = NULL, \
             updated_at = $3",
        user_id,
        entry_id,
        Utc::now()
    )
    .map_err(AppError::Database)?;

    find_by_user_and_entry(db, user_id, entry_id)
        .await?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

pub async fn set_processing(db: &Db, user_id: i64, entry_id: i64) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "UPDATE entry_summary \
         SET status = 'processing', updated_at = $3 \
         WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id,
        Utc::now()
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    Ok(())
}

pub async fn set_completed(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    summary_text: &str,
) -> AppResult<EntrySummary> {
    let rows = db_execute!(
        db,
        "UPDATE entry_summary \
         SET status = 'completed', summary_text = $3, error_message = NULL, updated_at = $4 \
         WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id,
        summary_text,
        Utc::now()
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    find_by_user_and_entry(db, user_id, entry_id)
        .await?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

pub async fn set_failed(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    error_message: &str,
) -> AppResult<EntrySummary> {
    let rows = db_execute!(
        db,
        "UPDATE entry_summary \
         SET status = 'failed', error_message = $3, updated_at = $4 \
         WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id,
        error_message,
        Utc::now()
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::NotFound("Entry summary not found".to_string()));
    }

    find_by_user_and_entry(db, user_id, entry_id)
        .await?
        .ok_or(AppError::NotFound("Entry summary not found".to_string()))
}

/// `true` when a row was actually removed, `false` when there was none.
pub async fn delete(db: &Db, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let rows = db_execute!(
        db,
        "DELETE FROM entry_summary WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id
    )
    .map_err(AppError::Database)?;

    Ok(rows > 0)
}

/// One query for a whole list page, rather than one per rendered row.
pub async fn get_statuses_for_entries(
    db: &Db,
    user_id: i64,
    entry_ids: &[i64],
) -> AppResult<HashMap<i64, SummaryStatus>> {
    if entry_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // The original used a runtime-built `IN (?, ?, ...)` clause. A variable-length
    // `IN` list can't be a static literal / single portable bind, so it is
    // rewritten as one index-covered point query per entry (identical result).
    let mut map = HashMap::new();
    for &entry_id in entry_ids {
        if let Some((_entry_id, status_str)) = query_opt!(
            db,
            (i64, String),
            "SELECT entry_id, status FROM entry_summary WHERE user_id = $1 AND entry_id = $2",
            user_id,
            entry_id
        )
        .map_err(AppError::Database)?
            && let Some(status) = SummaryStatus::parse(&status_str)
        {
            map.insert(entry_id, status);
        }
    }

    Ok(map)
}

/// Pending or processing rows left behind by a shutdown mid-job, for the
/// worker to re-queue at startup.
pub async fn find_incomplete(db: &Db) -> AppResult<Vec<(i64, i64, String)>> {
    query_all!(
        db,
        (i64, i64, String),
        "SELECT es.user_id, es.entry_id, e.link \
         FROM entry_summary es \
         INNER JOIN entry e ON es.entry_id = e.id \
         WHERE es.status IN ('pending', 'processing') AND e.link IS NOT NULL"
    )
    .map_err(AppError::Database)
}

pub async fn delete_expired(db: &Db, hours: i64) -> AppResult<usize> {
    let cutoff = Utc::now() - chrono::Duration::hours(hours);
    let rows = db_execute!(
        db,
        "DELETE FROM entry_summary WHERE created_at < $1",
        cutoff
    )
    .map_err(AppError::Database)?;

    Ok(rows as usize)
}

/// Count the user's entries that have a COMPLETED summary. Index-covered by
/// `idx_entry_summary_user_status`. Used for the sidebar "Summarized" badge.
pub async fn count_completed(db: &Db, user_id: i64) -> AppResult<i64> {
    query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM entry_summary WHERE user_id = $1 AND status = 'completed'",
        user_id
    )
    .map_err(AppError::Database)
}

/// Check if a summary record exists (any status)
pub async fn exists(db: &Db, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let count = query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM entry_summary WHERE user_id = $1 AND entry_id = $2",
        user_id,
        entry_id
    )
    .map_err(AppError::Database)?;

    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;
    use crate::models::{category, entry, feed, user};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    async fn create_test_user(db: &Db, username: &str) -> i64 {
        user::create_user(db, username, "hash123", Role::User)
            .await
            .unwrap()
            .id
    }

    async fn create_test_entry(db: &Db, user_id: i64) -> i64 {
        let category_id = category::create_category(db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Test Feed"),
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

        let (entry, _) = entry::upsert_entry(
            db,
            feed_id,
            "guid-123",
            Some("Test Entry"),
            Some("https://example.com/entry"),
            Some("Content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        entry.id
    }

    #[tokio::test]
    async fn test_upsert_pending() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        let summary = upsert_pending(&db, user_id, entry_id).await.unwrap();
        assert_eq!(summary.status, SummaryStatus::Pending);
        assert!(summary.summary_text.is_none());
        assert!(summary.error_message.is_none());
    }

    #[tokio::test]
    async fn test_status_transitions() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        upsert_pending(&db, user_id, entry_id).await.unwrap();

        set_processing(&db, user_id, entry_id).await.unwrap();
        let summary = find_by_user_and_entry(&db, user_id, entry_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Processing);

        set_completed(&db, user_id, entry_id, "This is the summary")
            .await
            .unwrap();
        let summary = find_by_user_and_entry(&db, user_id, entry_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Completed);
        assert_eq!(summary.summary_text.as_deref(), Some("This is the summary"));
    }

    #[tokio::test]
    async fn test_set_failed() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        upsert_pending(&db, user_id, entry_id).await.unwrap();
        set_failed(&db, user_id, entry_id, "API error")
            .await
            .unwrap();

        let summary = find_by_user_and_entry(&db, user_id, entry_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.status, SummaryStatus::Failed);
        assert_eq!(summary.error_message.as_deref(), Some("API error"));
    }

    #[tokio::test]
    async fn test_delete() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        upsert_pending(&db, user_id, entry_id).await.unwrap();
        assert!(exists(&db, user_id, entry_id).await.unwrap());

        let deleted = delete(&db, user_id, entry_id).await.unwrap();
        assert!(deleted);
        assert!(!exists(&db, user_id, entry_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_get_statuses_for_entries() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;

        let category_id = category::create_category(&db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Test Feed"),
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

        let mut entry_ids = Vec::new();
        for i in 0..3 {
            let (e, _) = entry::upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some(&format!("Entry {i}")),
                Some("https://example.com/entry"),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
            entry_ids.push(e.id);
        }

        upsert_pending(&db, user_id, entry_ids[0]).await.unwrap();
        upsert_pending(&db, user_id, entry_ids[1]).await.unwrap();
        set_completed(&db, user_id, entry_ids[1], "Summary")
            .await
            .unwrap();
        // entry_ids[2] has no summary

        let statuses = get_statuses_for_entries(&db, user_id, &entry_ids)
            .await
            .unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses.get(&entry_ids[0]), Some(&SummaryStatus::Pending));
        assert_eq!(statuses.get(&entry_ids[1]), Some(&SummaryStatus::Completed));
        assert_eq!(statuses.get(&entry_ids[2]), None);
    }

    #[tokio::test]
    async fn test_find_incomplete() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        upsert_pending(&db, user_id, entry_id).await.unwrap();

        let incomplete = find_incomplete(&db).await.unwrap();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].0, user_id);
        assert_eq!(incomplete[0].1, entry_id);
    }

    #[tokio::test]
    async fn test_upsert_resets_to_pending() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let entry_id = create_test_entry(&db, user_id).await;

        upsert_pending(&db, user_id, entry_id).await.unwrap();
        set_completed(&db, user_id, entry_id, "Summary")
            .await
            .unwrap();

        // Upsert should reset to pending
        let summary = upsert_pending(&db, user_id, entry_id).await.unwrap();
        assert_eq!(summary.status, SummaryStatus::Pending);
        assert!(summary.summary_text.is_none());
    }

    #[tokio::test]
    async fn test_status_string_conversion() {
        assert_eq!(SummaryStatus::Pending.as_str(), "pending");
        assert_eq!(SummaryStatus::Processing.as_str(), "processing");
        assert_eq!(SummaryStatus::Completed.as_str(), "completed");
        assert_eq!(SummaryStatus::Failed.as_str(), "failed");

        assert_eq!(
            SummaryStatus::parse("pending"),
            Some(SummaryStatus::Pending)
        );
        assert_eq!(
            SummaryStatus::parse("processing"),
            Some(SummaryStatus::Processing)
        );
        assert_eq!(
            SummaryStatus::parse("completed"),
            Some(SummaryStatus::Completed)
        );
        assert_eq!(SummaryStatus::parse("failed"), Some(SummaryStatus::Failed));
        assert_eq!(SummaryStatus::parse("invalid"), None);
    }

    #[tokio::test]
    async fn count_completed_counts_only_completed_for_user() {
        let db = setup_db().await;
        let u1 = create_test_user(&db, "u1").await;
        let u2 = create_test_user(&db, "u2").await;

        // create_test_entry reuses the same category name per user, so build
        // entries manually with unique GUIDs for the multi-entry u1 scenario.
        let cat1 = category::create_category(&db, u1, "Tech1")
            .await
            .unwrap()
            .id;
        let feed1 = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cat1,
                url: "https://example.com/feed1.xml",
                title: Some("Feed 1"),
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

        let db_ref = &db;
        let make_entry = |guid: &'static str| async move {
            entry::upsert_entry(
                db_ref,
                feed1,
                guid,
                Some("Test Entry"),
                Some("https://example.com/entry"),
                Some("Content"),
                None,
                None,
                None,
            )
            .await
            .unwrap()
            .0
            .id
        };

        let e1 = make_entry("g1").await;
        let e2 = make_entry("g2").await;
        let e3 = make_entry("g3").await;
        let e4 = make_entry("g4").await;
        // u2 gets its own entry via the helper (first call, no conflict)
        let e5 = create_test_entry(&db, u2).await;

        upsert_pending(&db, u1, e1).await.unwrap();
        set_completed(&db, u1, e1, "s").await.unwrap();
        upsert_pending(&db, u1, e2).await.unwrap();
        set_completed(&db, u1, e2, "s").await.unwrap();
        upsert_pending(&db, u1, e3).await.unwrap();
        upsert_pending(&db, u1, e4).await.unwrap();
        set_failed(&db, u1, e4, "err").await.unwrap();
        upsert_pending(&db, u2, e5).await.unwrap();
        set_completed(&db, u2, e5, "s").await.unwrap();

        assert_eq!(count_completed(&db, u1).await.unwrap(), 2);
        assert_eq!(count_completed(&db, u2).await.unwrap(), 1);
        assert_eq!(count_completed(&db, 99999).await.unwrap(), 0);
    }
}
