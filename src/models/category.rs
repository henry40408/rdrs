use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
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

fn row_to_category(row: &rusqlite::Row) -> rusqlite::Result<Category> {
    let created_at: String = row.get(3)?;

    Ok(Category {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        created_at: parse_datetime(&created_at),
    })
}

pub fn create_category(conn: &Connection, user_id: i64, name: &str) -> AppResult<Category> {
    let result = conn.execute(
        "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
        params![user_id, name],
    );

    match result {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            find_by_id(conn, id)?.ok_or(AppError::CategoryNotFound)
        }
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::CategoryExists)
        }
        Err(e) => Err(AppError::Database(e)),
    }
}

pub fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<Category>> {
    conn.query_row(
        "SELECT id, user_id, name, created_at FROM category WHERE id = ?1",
        params![id],
        row_to_category,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_id_and_user(
    conn: &Connection,
    id: i64,
    user_id: i64,
) -> AppResult<Option<Category>> {
    conn.query_row(
        "SELECT id, user_id, name, created_at FROM category WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        row_to_category,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_name_and_user(
    conn: &Connection,
    name: &str,
    user_id: i64,
) -> AppResult<Option<Category>> {
    conn.query_row(
        "SELECT id, user_id, name, created_at FROM category WHERE name = ?1 AND user_id = ?2",
        params![name, user_id],
        row_to_category,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_by_user(conn: &Connection, user_id: i64) -> AppResult<Vec<Category>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, name, created_at FROM category WHERE user_id = ?1 ORDER BY name ASC",
    )?;

    let categories = stmt
        .query_map(params![user_id], row_to_category)?
        .filter_map(Result::ok)
        .collect();

    Ok(categories)
}

pub fn update_name(
    conn: &Connection,
    id: i64,
    user_id: i64,
    new_name: &str,
) -> AppResult<Category> {
    let result = conn.execute(
        "UPDATE category SET name = ?1 WHERE id = ?2 AND user_id = ?3",
        params![new_name, id, user_id],
    );

    match result {
        Ok(rows) if rows == 0 => Err(AppError::CategoryNotFound),
        Ok(_) => find_by_id(conn, id)?.ok_or(AppError::CategoryNotFound),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::CategoryExists)
        }
        Err(e) => Err(AppError::Database(e)),
    }
}

pub fn delete_category(conn: &Connection, id: i64, user_id: i64) -> AppResult<()> {
    let rows = conn.execute(
        "DELETE FROM category WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;

    if rows == 0 {
        return Err(AppError::CategoryNotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
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

    #[test]
    fn test_create_and_find_category() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        let category = create_category(&conn, user_id, "Books").unwrap();
        assert_eq!(category.name, "Books");
        assert_eq!(category.user_id, user_id);

        let found = find_by_id(&conn, category.id).unwrap().unwrap();
        assert_eq!(found.name, "Books");

        let found_by_user = find_by_id_and_user(&conn, category.id, user_id)
            .unwrap()
            .unwrap();
        assert_eq!(found_by_user.name, "Books");
    }

    #[test]
    fn test_duplicate_category_name() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        create_category(&conn, user_id, "Books").unwrap();
        let result = create_category(&conn, user_id, "Books");
        assert!(matches!(result, Err(AppError::CategoryExists)));
    }

    #[test]
    fn test_same_name_different_users() {
        let conn = setup_db();
        let user1 = create_test_user(&conn, "user1");
        let user2 = create_test_user(&conn, "user2");

        create_category(&conn, user1, "Books").unwrap();
        let result = create_category(&conn, user2, "Books");
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_by_user_ordered() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        create_category(&conn, user_id, "Zebra").unwrap();
        create_category(&conn, user_id, "Apple").unwrap();
        create_category(&conn, user_id, "Mango").unwrap();

        let categories = list_by_user(&conn, user_id).unwrap();
        assert_eq!(categories.len(), 3);
        assert_eq!(categories[0].name, "Apple");
        assert_eq!(categories[1].name, "Mango");
        assert_eq!(categories[2].name, "Zebra");
    }

    #[test]
    fn test_update_name() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        let category = create_category(&conn, user_id, "Books").unwrap();
        let updated = update_name(&conn, category.id, user_id, "Novels").unwrap();
        assert_eq!(updated.name, "Novels");
    }

    #[test]
    fn test_update_name_conflict() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        create_category(&conn, user_id, "Books").unwrap();
        let movies = create_category(&conn, user_id, "Movies").unwrap();

        let result = update_name(&conn, movies.id, user_id, "Books");
        assert!(matches!(result, Err(AppError::CategoryExists)));
    }

    #[test]
    fn test_delete_category() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");

        let category = create_category(&conn, user_id, "Books").unwrap();
        delete_category(&conn, category.id, user_id).unwrap();

        assert!(find_by_id(&conn, category.id).unwrap().is_none());
    }

    #[test]
    fn test_ownership_check() {
        let conn = setup_db();
        let user1 = create_test_user(&conn, "user1");
        let user2 = create_test_user(&conn, "user2");

        let category = create_category(&conn, user1, "Books").unwrap();

        // user2 cannot access user1's category
        assert!(find_by_id_and_user(&conn, category.id, user2)
            .unwrap()
            .is_none());

        // user2 cannot update user1's category
        let result = update_name(&conn, category.id, user2, "Novels");
        assert!(matches!(result, Err(AppError::CategoryNotFound)));

        // user2 cannot delete user1's category
        let result = delete_category(&conn, category.id, user2);
        assert!(matches!(result, Err(AppError::CategoryNotFound)));
    }
}
