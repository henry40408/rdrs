use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::image;
use crate::utils::datetime::parse_datetime;

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

fn row_to_feed(row: &rusqlite::Row) -> rusqlite::Result<Feed> {
    let feed_updated_at: Option<String> = row.get(6)?;
    let fetched_at: Option<String> = row.get(7)?;
    let http2_disabled: i64 = row.get(12)?;
    let created_at: String = row.get(15)?;
    let updated_at: String = row.get(16)?;

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
        custom_user_agent: row.get(11)?,
        http2_disabled: http2_disabled != 0,
        custom_referrer: row.get(13)?,
        bucket: row.get(14)?,
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

pub fn create_feed(conn: &Connection, params: &CreateFeedParams<'_>) -> AppResult<Feed> {
    let http2_disabled_int = params.http2_disabled.unwrap_or(false) as i64;
    let bucket = url_to_bucket(params.url) as i64;
    let result = conn.execute(
        "INSERT INTO feed (category_id, url, title, description, site_url, custom_user_agent, http2_disabled, custom_referrer, bucket) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![params.category_id, params.url, params.title, params.description, params.site_url, params.custom_user_agent, http2_disabled_int, params.custom_referrer, bucket],
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

const SELECT_COLUMNS: &str = "id, category_id, url, title, description, site_url, feed_updated_at, fetched_at, fetch_error, etag, last_modified, custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at";

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
               f.custom_user_agent, f.http2_disabled, f.custom_referrer, f.bucket, f.created_at, f.updated_at
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
        ORDER BY f.title ASC
        "#,
    )?;

    let feeds = stmt
        .query_map(params![user_id], row_to_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(feeds)
}

/// Find a feed by URL across all categories for a given user.
pub fn find_by_url_for_user(conn: &Connection, url: &str, user_id: i64) -> AppResult<Option<Feed>> {
    conn.query_row(
        &format!(
            r#"
            SELECT {}
            FROM feed f
            INNER JOIN category c ON f.category_id = c.id
            WHERE f.url = ?1 AND c.user_id = ?2
            "#,
            SELECT_COLUMNS
                .split(", ")
                .map(|col| format!("f.{}", col))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        params![url, user_id],
        row_to_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_by_category(conn: &Connection, category_id: i64) -> AppResult<Vec<Feed>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM feed WHERE category_id = ?1 ORDER BY title ASC",
        SELECT_COLUMNS
    ))?;

    let feeds = stmt
        .query_map(params![category_id], row_to_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(feeds)
}

pub fn update_feed(conn: &Connection, params: &UpdateFeedParams<'_>) -> AppResult<Feed> {
    let http2_disabled_int = params.http2_disabled as i64;
    let bucket = url_to_bucket(params.url) as i64;
    let result = conn.execute(
        r#"
        UPDATE feed
        SET category_id = ?1, url = ?2, title = ?3, description = ?4, site_url = ?5, custom_user_agent = ?6, http2_disabled = ?7, custom_referrer = ?8, bucket = ?9, updated_at = datetime('now')
        WHERE id = ?10 AND category_id = ?11
        "#,
        rusqlite::params![params.new_category_id, params.url, params.title, params.description, params.site_url, params.custom_user_agent, http2_disabled_int, params.custom_referrer, bucket, params.id, params.category_id],
    );

    match result {
        Ok(0) => Err(AppError::FeedNotFound),
        Ok(_) => find_by_id(conn, params.id)?.ok_or(AppError::FeedNotFound),
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
    feed_updated_at: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let fetched_at_str = fetched_at.format("%Y-%m-%d %H:%M:%S").to_string();
    let feed_updated_at_str = feed_updated_at.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());
    conn.execute(
        r#"
        UPDATE feed
        SET fetched_at = ?1, fetch_error = ?2, etag = ?3, last_modified = ?4,
            feed_updated_at = COALESCE(?5, feed_updated_at),
            updated_at = datetime('now')
        WHERE id = ?6
        "#,
        params![
            fetched_at_str,
            fetch_error,
            etag,
            last_modified,
            feed_updated_at_str,
            id
        ],
    )?;
    Ok(())
}

pub fn list_by_bucket(conn: &Connection, bucket: u8) -> AppResult<Vec<Feed>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM feed WHERE bucket = ?1",
        SELECT_COLUMNS
    ))?;

    let feeds = stmt
        .query_map(params![bucket as i64], row_to_feed)?
        .collect::<Result<Vec<_>, _>>()?;

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
        .unwrap();

        assert_eq!(feed.url, "https://example.com/feed.xml");
        assert_eq!(feed.title, Some("Example Feed".to_string()));
        assert_eq!(feed.category_id, category_id);
        assert_eq!(feed.custom_user_agent, None);
        assert!(!feed.http2_disabled);
        assert_eq!(feed.custom_referrer, None);

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
        .unwrap();
        let result = create_feed(
            &conn,
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
        .unwrap();
        let result = create_feed(
            &conn,
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
        .unwrap();
        create_feed(
            &conn,
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
            &CreateFeedParams {
                category_id: cat1,
                url: "https://example1.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .unwrap();
        create_feed(
            &conn,
            &CreateFeedParams {
                category_id: cat1,
                url: "https://example2.com/feed.xml",
                title: None,
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .unwrap();
        create_feed(
            &conn,
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
        .unwrap();

        let updated = update_feed(
            &conn,
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

    #[test]
    fn test_delete_feed() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");

        let feed = create_feed(
            &conn,
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
        .unwrap();

        // Delete the category
        category::delete_category(&conn, category_id, user_id).unwrap();

        // Feed should be deleted too
        assert!(find_by_id(&conn, feed.id).unwrap().is_none());
    }
}
