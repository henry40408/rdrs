use chrono::{DateTime, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_opt, query_scalar};

pub const ENTITY_FEED: &str = "feed";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Image {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: i64,
    pub data: Vec<u8>,
    pub content_type: String,
    pub source_url: Option<String>,
    pub fetched_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

pub async fn find(db: &Db, entity_type: &str, entity_id: i64) -> AppResult<Option<Image>> {
    query_opt!(
        db,
        Image,
        "SELECT id, entity_type, entity_id, data, content_type, source_url, fetched_at, created_at \
         FROM image WHERE entity_type = $1 AND entity_id = $2",
        entity_type,
        entity_id
    )
    .map_err(AppError::Database)
}

pub async fn upsert(
    db: &Db,
    entity_type: &str,
    entity_id: i64,
    data: &[u8],
    content_type: &str,
    source_url: Option<&str>,
) -> AppResult<()> {
    let now = Utc::now();
    db_execute!(
        db,
        "INSERT INTO image (entity_type, entity_id, data, content_type, source_url, fetched_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT(entity_type, entity_id) DO UPDATE SET \
             data = excluded.data, \
             content_type = excluded.content_type, \
             source_url = excluded.source_url, \
             fetched_at = $6",
        entity_type,
        entity_id,
        data,
        content_type,
        source_url,
        now
    )
    .map_err(AppError::Database)?;
    Ok(())
}

pub async fn exists(db: &Db, entity_type: &str, entity_id: i64) -> AppResult<bool> {
    let count: i64 = query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM image WHERE entity_type = $1 AND entity_id = $2",
        entity_type,
        entity_id
    )
    .map_err(AppError::Database)?;
    Ok(count > 0)
}

/// The subset of `entity_ids` with an image of `entity_type`, in one query
/// (avoids per-row `exists` calls in list views).
pub async fn existing_ids(
    db: &Db,
    entity_type: &str,
    entity_ids: &[i64],
) -> AppResult<std::collections::HashSet<i64>> {
    if entity_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    // The dispatch macros need a static SQL literal, so no dynamic `IN (...)`:
    // fetch all ids of this type and intersect in Rust.
    let rows: Vec<(i64,)> = query_all!(
        db,
        (i64,),
        "SELECT entity_id FROM image WHERE entity_type = $1",
        entity_type
    )
    .map_err(AppError::Database)?;

    let existing: std::collections::HashSet<i64> = rows.into_iter().map(|(id,)| id).collect();
    Ok(entity_ids
        .iter()
        .copied()
        .filter(|id| existing.contains(id))
        .collect())
}

pub async fn needs_refresh(
    db: &Db,
    entity_type: &str,
    entity_id: i64,
    max_age_days: i64,
) -> AppResult<bool> {
    // Cutoff computed in Rust to avoid dialect-specific interval arithmetic.
    let cutoff = Utc::now() - chrono::Duration::days(max_age_days);
    let count: i64 = query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM image \
         WHERE entity_type = $1 AND entity_id = $2 AND fetched_at > $3",
        entity_type,
        entity_id,
        cutoff
    )
    .map_err(AppError::Database)?;

    Ok(count == 0)
}

pub async fn delete_by_entity(db: &Db, entity_type: &str, entity_id: i64) -> AppResult<()> {
    db_execute!(
        db,
        "DELETE FROM image WHERE entity_type = $1 AND entity_id = $2",
        entity_type,
        entity_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Delete up to `batch_size` feed images whose feed no longer exists; returns
/// the count. `image` is polymorphic and has no foreign key, so removing a
/// category or user (which cascades to feeds) and an icon fetch that finishes
/// after its feed was deleted both leave orphans behind.
pub async fn delete_orphaned_feed_images(db: &Db, batch_size: i64) -> AppResult<u64> {
    db_execute!(
        db,
        "DELETE FROM image WHERE id IN ( \
             SELECT id FROM image \
             WHERE entity_type = $1 \
               AND NOT EXISTS (SELECT 1 FROM feed WHERE feed.id = image.entity_id) \
             LIMIT $2)",
        ENTITY_FEED,
        batch_size
    )
    .map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;
    use crate::models::{category, feed};
    use crate::test_support::{seed_user, setup_db};

    #[tokio::test]
    async fn test_upsert_and_find() {
        let db = setup_db().await;

        let data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG header
        upsert(
            &db,
            ENTITY_FEED,
            1,
            &data,
            "image/png",
            Some("https://example.com/icon.png"),
        )
        .await
        .unwrap();

        let img = find(&db, ENTITY_FEED, 1).await.unwrap().unwrap();
        assert_eq!(img.entity_type, ENTITY_FEED);
        assert_eq!(img.entity_id, 1);
        assert_eq!(img.data, data);
        assert_eq!(img.content_type, "image/png");
        assert_eq!(
            img.source_url,
            Some("https://example.com/icon.png".to_string())
        );
    }

    #[tokio::test]
    async fn test_upsert_updates_existing() {
        let db = setup_db().await;

        let data1 = vec![0x89, 0x50, 0x4E, 0x47];
        upsert(&db, ENTITY_FEED, 1, &data1, "image/png", None)
            .await
            .unwrap();

        let data2 = vec![0x00, 0x00, 0x01, 0x00]; // ICO header
        upsert(
            &db,
            ENTITY_FEED,
            1,
            &data2,
            "image/x-icon",
            Some("https://example.com/favicon.ico"),
        )
        .await
        .unwrap();

        let img = find(&db, ENTITY_FEED, 1).await.unwrap().unwrap();
        assert_eq!(img.data, data2);
        assert_eq!(img.content_type, "image/x-icon");
    }

    #[tokio::test]
    async fn test_exists() {
        let db = setup_db().await;

        assert!(!exists(&db, ENTITY_FEED, 1).await.unwrap());

        upsert(&db, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None)
            .await
            .unwrap();

        assert!(exists(&db, ENTITY_FEED, 1).await.unwrap());
    }

    #[tokio::test]
    async fn test_existing_ids() {
        let db = setup_db().await;

        assert!(
            existing_ids(&db, ENTITY_FEED, &[])
                .await
                .unwrap()
                .is_empty()
        );

        upsert(&db, ENTITY_FEED, 1, &[1], "image/png", None)
            .await
            .unwrap();
        upsert(&db, ENTITY_FEED, 3, &[3], "image/png", None)
            .await
            .unwrap();

        let set = existing_ids(&db, ENTITY_FEED, &[1, 2, 3, 4]).await.unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains(&1));
        assert!(set.contains(&3));
        assert!(!set.contains(&2));

        // entity_type is scoped — a different type returns nothing.
        assert!(
            existing_ids(&db, "entry", &[1, 3])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_delete_by_entity() {
        let db = setup_db().await;

        upsert(&db, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None)
            .await
            .unwrap();
        assert!(exists(&db, ENTITY_FEED, 1).await.unwrap());

        delete_by_entity(&db, ENTITY_FEED, 1).await.unwrap();
        assert!(!exists(&db, ENTITY_FEED, 1).await.unwrap());
    }

    #[tokio::test]
    async fn test_needs_refresh() {
        let db = setup_db().await;

        assert!(needs_refresh(&db, ENTITY_FEED, 1, 7).await.unwrap());

        upsert(&db, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None)
            .await
            .unwrap();
        assert!(!needs_refresh(&db, ENTITY_FEED, 1, 7).await.unwrap());
    }

    async fn seed_feed(db: &Db) -> i64 {
        let user_id = seed_user(db, "owner", Role::User).await.id;
        let category_id = category::create_category(db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn test_orphan_sweep_removes_only_images_of_missing_feeds() {
        let db = setup_db().await;
        let live_feed_id = seed_feed(&db).await;
        let missing_feed_id = live_feed_id + 1000;
        upsert(&db, ENTITY_FEED, live_feed_id, &[1], "image/png", None)
            .await
            .unwrap();
        upsert(&db, ENTITY_FEED, missing_feed_id, &[2], "image/png", None)
            .await
            .unwrap();
        // Another entity type with an unrelated id must never be touched.
        upsert(&db, "other", missing_feed_id, &[3], "image/png", None)
            .await
            .unwrap();

        let deleted = delete_orphaned_feed_images(&db, 500).await.unwrap();

        assert_eq!(deleted, 1);
        assert!(exists(&db, ENTITY_FEED, live_feed_id).await.unwrap());
        assert!(!exists(&db, ENTITY_FEED, missing_feed_id).await.unwrap());
        assert!(exists(&db, "other", missing_feed_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_orphan_sweep_respects_the_batch_size() {
        let db = setup_db().await;
        for feed_id in [901, 902, 903] {
            upsert(&db, ENTITY_FEED, feed_id, &[1], "image/png", None)
                .await
                .unwrap();
        }

        assert_eq!(delete_orphaned_feed_images(&db, 2).await.unwrap(), 2);
        assert_eq!(delete_orphaned_feed_images(&db, 2).await.unwrap(), 1);
        assert_eq!(delete_orphaned_feed_images(&db, 2).await.unwrap(), 0);
    }
}
