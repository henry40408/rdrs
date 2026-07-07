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

/// Return the subset of `entity_ids` that have an image of `entity_type`, as a
/// set, in a single query. Replaces per-entity `exists` calls in list views
/// (e.g. the Feeds page / `GReader` subscription list) that would otherwise issue
/// one query per row. Empty input is a no-op returning an empty set.
pub async fn existing_ids(
    db: &Db,
    entity_type: &str,
    entity_ids: &[i64],
) -> AppResult<std::collections::HashSet<i64>> {
    if entity_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }

    // A dynamic `IN (...)` placeholder list can't be a `&'static str` literal
    // (the dispatch macros require one), so fetch the entity_ids that have an
    // image of this type and intersect with the requested set in Rust.
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
    // Freshness cutoff computed in Rust instead of SQL interval arithmetic:
    // an image is fresh iff `fetched_at > now - max_age_days`.
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

    // No fresh row → needs refresh.
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

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

        // Empty input is a no-op.
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

        // No image exists - needs refresh
        assert!(needs_refresh(&db, ENTITY_FEED, 1, 7).await.unwrap());

        // Insert fresh image - doesn't need refresh
        upsert(&db, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None)
            .await
            .unwrap();
        assert!(!needs_refresh(&db, ENTITY_FEED, 1, 7).await.unwrap());
    }
}
