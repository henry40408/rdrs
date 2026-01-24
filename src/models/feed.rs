use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::image;

#[derive(Debug, Clone, Serialize)]
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn url_to_bucket(url: &str) -> u8 {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    (hasher.finish() % 60) as u8
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        .or_else(|_| dateparser::parse(s).map(|dt| dt.with_timezone(&Utc)))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_feed(row: &rusqlite::Row) -> rusqlite::Result<Feed> {
    let feed_updated_at: Option<String> = row.get(6)?;
    let fetched_at: Option<String> = row.get(7)?;
    let created_at: String = row.get(11)?;
    let updated_at: String = row.get(12)?;

    Ok(Feed {
        id: row.get(0)?,
        category_id: row.get(1)?,
        url: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        site_url: row.get(5)?,
        feed_updated_at: feed_updated_at.map(|s| parse_datetime(&s)),
        fetched_at: fetched_at.map(|s| parse_datetime(&s)),
        fetch_error: row.get(8)?,
        etag: row.get(9)?,
        last_modified: row.get(10)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

pub fn create_feed(
    conn: &Connection,
    category_id: i64,
    url: &str,
    title: Option<&str>,
    description: Option<&str>,
    site_url: Option<&str>,
) -> AppResult<Feed> {
    let result = conn.execute(
        "INSERT INTO feed (category_id, url, title, description, site_url) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![category_id, url, title, description, site_url],
    );

    match result {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)
        }
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::FeedExists)
        }
        Err(e) => Err(AppError::Database(e)),
    }
}

const SELECT_COLUMNS: &str = "id, category_id, url, title, description, site_url, feed_updated_at, fetched_at, fetch_error, etag, last_modified, created_at, updated_at";

pub fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<Feed>> {
    conn.query_row(
        &format!("SELECT {} FROM feed WHERE id = ?1", SELECT_COLUMNS),
        params![id],
        row_to_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_id_and_category(
    conn: &Connection,
    id: i64,
    category_id: i64,
) -> AppResult<Option<Feed>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM feed WHERE id = ?1 AND category_id = ?2",
            SELECT_COLUMNS
        ),
        params![id, category_id],
        row_to_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_url_and_category(
    conn: &Connection,
    url: &str,
    category_id: i64,
) -> AppResult<Option<Feed>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM feed WHERE url = ?1 AND category_id = ?2",
            SELECT_COLUMNS
        ),
        params![url, category_id],
        row_to_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_by_user(conn: &Connection, user_id: i64) -> AppResult<Vec<Feed>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT f.id, f.category_id, f.url, f.title, f.description, f.site_url,
               f.feed_updated_at, f.fetched_at, f.fetch_error, f.etag, f.last_modified,
               f.created_at, f.updated_at
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
        ORDER BY f.title ASC
        "#,
    )?;

    let feeds = stmt
        .query_map(params![user_id], row_to_feed)?
        .filter_map(Result::ok)
        .collect();

    Ok(feeds)
}

pub fn list_by_category(conn: &Connection, category_id: i64) -> AppResult<Vec<Feed>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM feed WHERE category_id = ?1 ORDER BY title ASC",
        SELECT_COLUMNS
    ))?;

    let feeds = stmt
        .query_map(params![category_id], row_to_feed)?
        .filter_map(Result::ok)
        .collect();

    Ok(feeds)
}

#[allow(clippy::too_many_arguments)]
pub fn update_feed(
    conn: &Connection,
    id: i64,
    category_id: i64,
    new_category_id: i64,
    url: &str,
    title: Option<&str>,
    description: Option<&str>,
    site_url: Option<&str>,
) -> AppResult<Feed> {
    let result = conn.execute(
        r#"
        UPDATE feed
        SET category_id = ?1, url = ?2, title = ?3, description = ?4, site_url = ?5, updated_at = datetime('now')
        WHERE id = ?6 AND category_id = ?7
        "#,
        params![new_category_id, url, title, description, site_url, id, category_id],
    );

    match result {
        Ok(0) => Err(AppError::FeedNotFound),
        Ok(_) => find_by_id(conn, id)?.ok_or(AppError::FeedNotFound),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::FeedExists)
        }
        Err(e) => Err(AppError::Database(e)),
    }
}

pub fn delete_feed(conn: &Connection, id: i64, category_id: i64) -> AppResult<()> {
    let rows = conn.execute(
        "DELETE FROM feed WHERE id = ?1 AND category_id = ?2",
        params![id, category_id],
    )?;

    if rows == 0 {
        return Err(AppError::FeedNotFound);
    }

    // Clean up associated image
    image::delete_by_entity(conn, image::ENTITY_FEED, id)?;

    Ok(())
}

pub fn update_fetch_result(
    conn: &Connection,
    id: i64,
    fetched_at: DateTime<Utc>,
    fetch_error: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<()> {
    let fetched_at_str = fetched_at.format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        r#"
        UPDATE feed
        SET fetched_at = ?1, fetch_error = ?2, etag = ?3, last_modified = ?4, updated_at = datetime('now')
        WHERE id = ?5
        "#,
        params![fetched_at_str, fetch_error, etag, last_modified, id],
    )?;
    Ok(())
}

pub fn list_by_bucket(conn: &Connection, bucket: u8) -> AppResult<Vec<Feed>> {
    let mut stmt = conn.prepare(&format!("SELECT {} FROM feed", SELECT_COLUMNS))?;

    let feeds: Vec<Feed> = stmt
        .query_map([], row_to_feed)?
        .filter_map(Result::ok)
        .filter(|feed| url_to_bucket(&feed.url) == bucket)
        .collect();

    Ok(feeds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::category;
    use crate::models::user::{self, Role};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn create_test_user(conn: &Connection, username: &str) -> i64 {
        user::create_user(conn, username, "hash123", Role::User)
            .unwrap()
            .id
    }

    fn create_test_category(conn: &Connection, user_id: i64, name: &str) -> i64 {
        category::create_category(conn, user_id, name).unwrap().id
    }

    #[test]
    fn test_create_and_find_feed() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        let feed = create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            Some("Example Feed"),
            Some("An example feed"),
            Some("https://example.com"),
        )
        .unwrap();

        assert_eq!(feed.url, "https://example.com/feed.xml");
        assert_eq!(feed.title, Some("Example Feed".to_string()));
        assert_eq!(feed.category_id, category_id);

        let found = find_by_id(&conn, feed.id).unwrap().unwrap();
        assert_eq!(found.url, feed.url);
    }

    #[test]
    fn test_duplicate_feed_url_in_same_category() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();
        let result = create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        );
        assert!(matches!(result, Err(AppError::FeedExists)));
    }

    #[test]
    fn test_same_url_different_categories() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let cat1 = create_test_category(&conn, user_id, "Tech");
        let cat2 = create_test_category(&conn, user_id, "News");

        create_feed(
            &conn,
            cat1,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();
        let result = create_feed(
            &conn,
            cat2,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_by_user() {
        let conn = setup_db();
        let user1 = create_test_user(&conn, "user1");
        let user2 = create_test_user(&conn, "user2");
        let cat1 = create_test_category(&conn, user1, "Tech");
        let cat2 = create_test_category(&conn, user2, "News");

        create_feed(
            &conn,
            cat1,
            "https://example1.com/feed.xml",
            Some("Feed 1"),
            None,
            None,
        )
        .unwrap();
        create_feed(
            &conn,
            cat2,
            "https://example2.com/feed.xml",
            Some("Feed 2"),
            None,
            None,
        )
        .unwrap();

        let user1_feeds = list_by_user(&conn, user1).unwrap();
        assert_eq!(user1_feeds.len(), 1);
        assert_eq!(user1_feeds[0].title, Some("Feed 1".to_string()));

        let user2_feeds = list_by_user(&conn, user2).unwrap();
        assert_eq!(user2_feeds.len(), 1);
        assert_eq!(user2_feeds[0].title, Some("Feed 2".to_string()));
    }

    #[test]
    fn test_list_by_category() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let cat1 = create_test_category(&conn, user_id, "Tech");
        let cat2 = create_test_category(&conn, user_id, "News");

        create_feed(
            &conn,
            cat1,
            "https://example1.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();
        create_feed(
            &conn,
            cat1,
            "https://example2.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();
        create_feed(
            &conn,
            cat2,
            "https://example3.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();

        let cat1_feeds = list_by_category(&conn, cat1).unwrap();
        assert_eq!(cat1_feeds.len(), 2);

        let cat2_feeds = list_by_category(&conn, cat2).unwrap();
        assert_eq!(cat2_feeds.len(), 1);
    }

    #[test]
    fn test_update_feed() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        let feed = create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            Some("Old Title"),
            None,
            None,
        )
        .unwrap();

        let updated = update_feed(
            &conn,
            feed.id,
            category_id,
            category_id,
            "https://example.com/new-feed.xml",
            Some("New Title"),
            Some("New Description"),
            Some("https://example.com"),
        )
        .unwrap();

        assert_eq!(updated.url, "https://example.com/new-feed.xml");
        assert_eq!(updated.title, Some("New Title".to_string()));
        assert_eq!(updated.description, Some("New Description".to_string()));
    }

    #[test]
    fn test_delete_feed() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        let feed = create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();
        delete_feed(&conn, feed.id, category_id).unwrap();

        assert!(find_by_id(&conn, feed.id).unwrap().is_none());
    }

    #[test]
    fn test_cascade_delete_on_category() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        let feed = create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            None,
            None,
            None,
        )
        .unwrap();

        // Delete the category
        category::delete_category(&conn, category_id, user_id).unwrap();

        // Feed should be deleted too
        assert!(find_by_id(&conn, feed.id).unwrap().is_none());
    }
}
