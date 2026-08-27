use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::{Db, DbInner, Tx, is_unique_violation};
use crate::error::{AppError, AppResult};
use crate::models::image;
use crate::{db_execute, db_execute_tx, query_all, query_one, query_opt, query_scalar};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Feed {
    pub id: i64,
    pub category_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub feed_updated_at: Option<DateTime<Utc>>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub fetch_error: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: bool,
    pub custom_referrer: Option<String>,
    pub bucket: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn url_to_bucket(url: &str) -> u8 {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    (hasher.finish() % 60) as u8
}

/// Parameters for creating a new feed.
pub struct CreateFeedParams<'a> {
    pub category_id: i64,
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub site_url: Option<&'a str>,
    pub custom_user_agent: Option<&'a str>,
    pub http2_disabled: Option<bool>,
    pub custom_referrer: Option<&'a str>,
}

/// Parameters for updating an existing feed.
pub struct UpdateFeedParams<'a> {
    pub id: i64,
    pub category_id: i64,
    pub new_category_id: i64,
    pub url: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub site_url: Option<&'a str>,
    pub custom_user_agent: Option<&'a str>,
    pub http2_disabled: bool,
    pub custom_referrer: Option<&'a str>,
}

pub async fn create_feed(db: &Db, params: &CreateFeedParams<'_>) -> AppResult<Feed> {
    let http2_disabled = params.http2_disabled.unwrap_or(false);
    let bucket = url_to_bucket(params.url) as i64;
    query_one!(
        db,
        Feed,
        "INSERT INTO feed (category_id, url, title, description, site_url, \
         custom_user_agent, http2_disabled, custom_referrer, bucket) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at",
        params.category_id,
        params.url,
        params.title,
        params.description,
        params.site_url,
        params.custom_user_agent,
        http2_disabled,
        params.custom_referrer,
        bucket
    )
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::FeedExists
        } else {
            AppError::Database(e)
        }
    })
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<Feed>> {
    query_opt!(
        db,
        Feed,
        "SELECT id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at \
         FROM feed WHERE id = $1",
        id
    )
    .map_err(AppError::Database)
}

pub async fn find_by_url_and_category(
    db: &Db,
    url: &str,
    category_id: i64,
) -> AppResult<Option<Feed>> {
    query_opt!(
        db,
        Feed,
        "SELECT id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at \
         FROM feed WHERE url = $1 AND category_id = $2",
        url,
        category_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_user(db: &Db, user_id: i64) -> AppResult<Vec<Feed>> {
    query_all!(
        db,
        Feed,
        "SELECT f.id, f.category_id, f.url, f.title, f.description, f.site_url, \
         f.feed_updated_at, f.fetched_at, f.fetch_error, f.etag, f.last_modified, \
         f.custom_user_agent, f.http2_disabled, f.custom_referrer, f.bucket, f.created_at, f.updated_at \
         FROM feed f INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1 ORDER BY f.title ASC",
        user_id
    )
    .map_err(AppError::Database)
}

/// Count how many feeds a user has subscribed to (across all categories).
pub async fn count_by_user(db: &Db, user_id: i64) -> AppResult<i64> {
    query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM feed f INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1",
        user_id
    )
    .map_err(AppError::Database)
}

/// Find a feed by URL across all categories for a given user.
pub async fn find_by_url_for_user(db: &Db, url: &str, user_id: i64) -> AppResult<Option<Feed>> {
    query_opt!(
        db,
        Feed,
        "SELECT f.id, f.category_id, f.url, f.title, f.description, f.site_url, \
         f.feed_updated_at, f.fetched_at, f.fetch_error, f.etag, f.last_modified, \
         f.custom_user_agent, f.http2_disabled, f.custom_referrer, f.bucket, f.created_at, f.updated_at \
         FROM feed f INNER JOIN category c ON f.category_id = c.id \
         WHERE f.url = $1 AND c.user_id = $2",
        url,
        user_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_category(db: &Db, category_id: i64) -> AppResult<Vec<Feed>> {
    query_all!(
        db,
        Feed,
        "SELECT id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at \
         FROM feed WHERE category_id = $1 ORDER BY title ASC",
        category_id
    )
    .map_err(AppError::Database)
}

pub async fn update_feed(db: &Db, params: &UpdateFeedParams<'_>) -> AppResult<Feed> {
    let bucket = url_to_bucket(params.url) as i64;
    let now = Utc::now();
    // `RETURNING` + `fetch_optional`: `None` means no row matched (id/category
    // mismatch) → `FeedNotFound`.
    match query_opt!(
        db,
        Feed,
        "UPDATE feed SET category_id = $1, url = $2, title = $3, description = $4, \
         site_url = $5, custom_user_agent = $6, http2_disabled = $7, custom_referrer = $8, \
         bucket = $9, updated_at = $10 WHERE id = $11 AND category_id = $12 \
         RETURNING id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at",
        params.new_category_id,
        params.url,
        params.title,
        params.description,
        params.site_url,
        params.custom_user_agent,
        params.http2_disabled,
        params.custom_referrer,
        bucket,
        now,
        params.id,
        params.category_id
    ) {
        Ok(Some(f)) => Ok(f),
        Ok(None) => Err(AppError::FeedNotFound),
        Err(e) if is_unique_violation(&e) => Err(AppError::FeedExists),
        Err(e) => Err(AppError::Database(e)),
    }
}

pub async fn delete_feed(db: &Db, id: i64, category_id: i64) -> AppResult<()> {
    let rows = db_execute!(
        db,
        "DELETE FROM feed WHERE id = $1 AND category_id = $2",
        id,
        category_id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::FeedNotFound);
    }

    // Clean up associated image
    image::delete_by_entity(db, image::ENTITY_FEED, id).await?;

    Ok(())
}

pub async fn update_fetch_result(
    db: &Db,
    id: i64,
    fetched_at: DateTime<Utc>,
    fetch_error: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    feed_updated_at: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let now = Utc::now();
    db_execute!(
        db,
        "UPDATE feed SET fetched_at = $1, fetch_error = $2, etag = $3, last_modified = $4, \
         feed_updated_at = COALESCE($5, feed_updated_at), updated_at = $6 WHERE id = $7",
        fetched_at,
        fetch_error,
        etag,
        last_modified,
        feed_updated_at,
        now,
        id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Transactional sibling of [`update_fetch_result`] for the feed-sync unit of
/// work, which upserts a whole feed's entries and records the fetch result in
/// one transaction so the read side never observes a half-applied feed.
pub async fn update_fetch_result_tx(
    tx: &mut Tx<'_>,
    id: i64,
    fetched_at: DateTime<Utc>,
    fetch_error: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    feed_updated_at: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let now = Utc::now();
    db_execute_tx!(
        tx,
        "UPDATE feed SET fetched_at = $1, fetch_error = $2, etag = $3, last_modified = $4, \
         feed_updated_at = COALESCE($5, feed_updated_at), updated_at = $6 WHERE id = $7",
        fetched_at,
        fetch_error,
        etag,
        last_modified,
        feed_updated_at,
        now,
        id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Distinct owning user ids for the given feeds (a feed belongs to one
/// category, which belongs to one user). Empty input → empty output.
pub async fn owner_user_ids_for_feeds(db: &Db, feed_ids: &[i64]) -> AppResult<Vec<i64>> {
    if feed_ids.is_empty() {
        return Ok(Vec::new());
    }
    const PREFIX: &str = "SELECT DISTINCT c.user_id \
         FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id IN (";
    // Dynamic IN-list: one bound placeholder per id, built per backend.
    let rows = match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(PREFIX);
            let mut sep = qb.separated(", ");
            for id in feed_ids {
                sep.push_bind(*id);
            }
            qb.push(")");
            qb.build_query_scalar::<i64>().fetch_all(pool).await
        }
        DbInner::Postgres(pool) => {
            let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(PREFIX);
            let mut sep = qb.separated(", ");
            for id in feed_ids {
                sep.push_bind(*id);
            }
            qb.push(")");
            qb.build_query_scalar::<i64>().fetch_all(pool).await
        }
    }
    .map_err(AppError::Database)?;
    Ok(rows)
}

pub async fn list_by_bucket(db: &Db, bucket: u8) -> AppResult<Vec<Feed>> {
    let bucket = bucket as i64;
    query_all!(
        db,
        Feed,
        "SELECT id, category_id, url, title, description, site_url, \
         feed_updated_at, fetched_at, fetch_error, etag, last_modified, \
         custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at \
         FROM feed WHERE bucket = $1",
        bucket
    )
    .map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::category;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    async fn create_test_user(db: &Db, username: &str) -> i64 {
        user::create_user(db, username, "hash123", Role::User)
            .await
            .unwrap()
            .id
    }

    async fn create_test_category(db: &Db, user_id: i64, name: &str) -> i64 {
        category::create_category(db, user_id, name)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn test_create_and_find_feed() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        let feed = create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Example Feed"),
                description: Some("An example feed"),
                site_url: Some("https://example.com"),
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(feed.url, "https://example.com/feed.xml");
        assert_eq!(feed.title, Some("Example Feed".to_string()));
        assert_eq!(feed.category_id, category_id);
        assert_eq!(feed.custom_user_agent, None);
        assert!(!feed.http2_disabled);
        assert_eq!(feed.custom_referrer, None);

        let found = find_by_id(&db, feed.id).await.unwrap().unwrap();
        assert_eq!(found.url, feed.url);
    }

    #[tokio::test]
    async fn test_duplicate_feed_url_in_same_category() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();
        let result = create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await;
        assert!(matches!(result, Err(AppError::FeedExists)));
    }

    #[tokio::test]
    async fn test_same_url_different_categories() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let cat1 = create_test_category(&db, user_id, "Tech").await;
        let cat2 = create_test_category(&db, user_id, "News").await;

        create_feed(
            &db,
            &CreateFeedParams {
                category_id: cat1,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();
        let result = create_feed(
            &db,
            &CreateFeedParams {
                category_id: cat2,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_list_by_user() {
        let db = setup_db().await;
        let user1 = create_test_user(&db, "user1").await;
        let user2 = create_test_user(&db, "user2").await;
        let cat1 = create_test_category(&db, user1, "Tech").await;
        let cat2 = create_test_category(&db, user2, "News").await;

        create_feed(
            &db,
            &CreateFeedParams {
                category_id: cat1,
                url: "https://example1.com/feed.xml",
                title: Some("Feed 1"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();
        create_feed(
            &db,
            &CreateFeedParams {
                category_id: cat2,
                url: "https://example2.com/feed.xml",
                title: Some("Feed 2"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();

        let user1_feeds = list_by_user(&db, user1).await.unwrap();
        assert_eq!(user1_feeds.len(), 1);
        assert_eq!(user1_feeds[0].title, Some("Feed 1".to_string()));

        let user2_feeds = list_by_user(&db, user2).await.unwrap();
        assert_eq!(user2_feeds.len(), 1);
        assert_eq!(user2_feeds[0].title, Some("Feed 2".to_string()));
    }

    #[tokio::test]
    async fn test_list_by_category() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let cat1 = create_test_category(&db, user_id, "Tech").await;
        let cat2 = create_test_category(&db, user_id, "News").await;

        for url in [
            "https://example1.com/feed.xml",
            "https://example2.com/feed.xml",
        ] {
            create_feed(
                &db,
                &CreateFeedParams {
                    category_id: cat1,
                    url,
                    title: None,
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .await
            .unwrap();
        }
        create_feed(
            &db,
            &CreateFeedParams {
                category_id: cat2,
                url: "https://example3.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();

        let cat1_feeds = list_by_category(&db, cat1).await.unwrap();
        assert_eq!(cat1_feeds.len(), 2);

        let cat2_feeds = list_by_category(&db, cat2).await.unwrap();
        assert_eq!(cat2_feeds.len(), 1);
    }

    #[tokio::test]
    async fn test_update_feed() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        let feed = create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Old Title"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();

        let updated = update_feed(
            &db,
            &UpdateFeedParams {
                id: feed.id,
                category_id,
                new_category_id: category_id,
                url: "https://example.com/new-feed.xml",
                title: Some("New Title"),
                description: Some("New Description"),
                site_url: Some("https://example.com"),
                custom_user_agent: Some("Custom UA"),
                http2_disabled: true,
                custom_referrer: Some("https://example.com"),
            },
        )
        .await
        .unwrap();

        assert_eq!(updated.url, "https://example.com/new-feed.xml");
        assert_eq!(updated.title, Some("New Title".to_string()));
        assert_eq!(updated.description, Some("New Description".to_string()));
        assert_eq!(updated.custom_user_agent, Some("Custom UA".to_string()));
        assert!(updated.http2_disabled);
        assert_eq!(
            updated.custom_referrer,
            Some("https://example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_delete_feed() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        let feed = create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();
        delete_feed(&db, feed.id, category_id).await.unwrap();

        assert!(find_by_id(&db, feed.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cascade_delete_on_category() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        let feed = create_feed(
            &db,
            &CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();

        category::delete_category(&db, category_id, user_id)
            .await
            .unwrap();

        // Feed should be deleted too
        assert!(find_by_id(&db, feed.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn owner_user_ids_for_feeds_returns_distinct_owners() {
        let db = setup_db().await;
        let u1 = user::create_user(&db, "u1", "h", Role::User)
            .await
            .unwrap()
            .id;
        let u2 = user::create_user(&db, "u2", "h", Role::User)
            .await
            .unwrap()
            .id;
        let c1 = create_test_category(&db, u1, "A").await;
        let c2 = create_test_category(&db, u2, "B").await;
        let f1 = create_feed(
            &db,
            &CreateFeedParams {
                category_id: c1,
                url: "https://a/f",
                title: Some("a"),
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
        let f2 = create_feed(
            &db,
            &CreateFeedParams {
                category_id: c2,
                url: "https://b/f",
                title: Some("b"),
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

        let mut owners = owner_user_ids_for_feeds(&db, &[f1, f2]).await.unwrap();
        owners.sort_unstable();
        assert_eq!(owners, vec![u1, u2]);
        assert!(owner_user_ids_for_feeds(&db, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_count_by_user() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let other_id = create_test_user(&db, "other").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;

        // No feeds yet.
        assert_eq!(count_by_user(&db, user_id).await.unwrap(), 0);

        for url in [
            "https://a.example.com/feed.xml",
            "https://b.example.com/feed.xml",
        ] {
            create_feed(
                &db,
                &CreateFeedParams {
                    category_id,
                    url,
                    title: None,
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .await
            .unwrap();
        }

        assert_eq!(count_by_user(&db, user_id).await.unwrap(), 2);
        // Another user's count is isolated.
        assert_eq!(count_by_user(&db, other_id).await.unwrap(), 0);
    }
}
