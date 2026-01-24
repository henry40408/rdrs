use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};

pub const ENTITY_FEED: &str = "feed";

#[derive(Debug, Clone)]
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

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<Image> {
    let fetched_at: String = row.get(6)?;
    let created_at: String = row.get(7)?;

    Ok(Image {
        id: row.get(0)?,
        entity_type: row.get(1)?,
        entity_id: row.get(2)?,
        data: row.get(3)?,
        content_type: row.get(4)?,
        source_url: row.get(5)?,
        fetched_at: parse_datetime(&fetched_at),
        created_at: parse_datetime(&created_at),
    })
}

pub fn find(conn: &Connection, entity_type: &str, entity_id: i64) -> AppResult<Option<Image>> {
    conn.query_row(
        "SELECT id, entity_type, entity_id, data, content_type, source_url, fetched_at, created_at
         FROM image WHERE entity_type = ?1 AND entity_id = ?2",
        params![entity_type, entity_id],
        row_to_image,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn upsert(
    conn: &Connection,
    entity_type: &str,
    entity_id: i64,
    data: &[u8],
    content_type: &str,
    source_url: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO image (entity_type, entity_id, data, content_type, source_url, fetched_at)
        VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
        ON CONFLICT(entity_type, entity_id) DO UPDATE SET
            data = excluded.data,
            content_type = excluded.content_type,
            source_url = excluded.source_url,
            fetched_at = datetime('now')
        "#,
        params![entity_type, entity_id, data, content_type, source_url],
    )?;
    Ok(())
}

pub fn exists(conn: &Connection, entity_type: &str, entity_id: i64) -> AppResult<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM image WHERE entity_type = ?1 AND entity_id = ?2",
        params![entity_type, entity_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn needs_refresh(
    conn: &Connection,
    entity_type: &str,
    entity_id: i64,
    max_age_days: i64,
) -> AppResult<bool> {
    let result: Option<i64> = conn
        .query_row(
            r#"
            SELECT 1 FROM image
            WHERE entity_type = ?1 AND entity_id = ?2
            AND datetime(fetched_at) > datetime('now', ?3)
            "#,
            params![entity_type, entity_id, format!("-{} days", max_age_days)],
            |row| row.get(0),
        )
        .optional()?;

    Ok(result.is_none())
}

pub fn delete_by_entity(conn: &Connection, entity_type: &str, entity_id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM image WHERE entity_type = ?1 AND entity_id = ?2",
        params![entity_type, entity_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn test_upsert_and_find() {
        let conn = setup_db();

        let data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG header
        upsert(
            &conn,
            ENTITY_FEED,
            1,
            &data,
            "image/png",
            Some("https://example.com/icon.png"),
        )
        .unwrap();

        let img = find(&conn, ENTITY_FEED, 1).unwrap().unwrap();
        assert_eq!(img.entity_type, ENTITY_FEED);
        assert_eq!(img.entity_id, 1);
        assert_eq!(img.data, data);
        assert_eq!(img.content_type, "image/png");
        assert_eq!(
            img.source_url,
            Some("https://example.com/icon.png".to_string())
        );
    }

    #[test]
    fn test_upsert_updates_existing() {
        let conn = setup_db();

        let data1 = vec![0x89, 0x50, 0x4E, 0x47];
        upsert(&conn, ENTITY_FEED, 1, &data1, "image/png", None).unwrap();

        let data2 = vec![0x00, 0x00, 0x01, 0x00]; // ICO header
        upsert(
            &conn,
            ENTITY_FEED,
            1,
            &data2,
            "image/x-icon",
            Some("https://example.com/favicon.ico"),
        )
        .unwrap();

        let img = find(&conn, ENTITY_FEED, 1).unwrap().unwrap();
        assert_eq!(img.data, data2);
        assert_eq!(img.content_type, "image/x-icon");
    }

    #[test]
    fn test_exists() {
        let conn = setup_db();

        assert!(!exists(&conn, ENTITY_FEED, 1).unwrap());

        upsert(&conn, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None).unwrap();

        assert!(exists(&conn, ENTITY_FEED, 1).unwrap());
    }

    #[test]
    fn test_delete_by_entity() {
        let conn = setup_db();

        upsert(&conn, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None).unwrap();
        assert!(exists(&conn, ENTITY_FEED, 1).unwrap());

        delete_by_entity(&conn, ENTITY_FEED, 1).unwrap();
        assert!(!exists(&conn, ENTITY_FEED, 1).unwrap());
    }

    #[test]
    fn test_needs_refresh() {
        let conn = setup_db();

        // No image exists - needs refresh
        assert!(needs_refresh(&conn, ENTITY_FEED, 1, 7).unwrap());

        // Insert fresh image - doesn't need refresh
        upsert(&conn, ENTITY_FEED, 1, &[1, 2, 3], "image/png", None).unwrap();
        assert!(!needs_refresh(&conn, ENTITY_FEED, 1, 7).unwrap());
    }
}
