use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: Role,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn is_disabled(&self) -> bool {
        self.disabled_at.is_some()
    }

    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    let role_str: String = row.get(3)?;
    let disabled_at: Option<String> = row.get(4)?;
    let created_at: String = row.get(5)?;

    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        role: Role::from_str(&role_str).unwrap_or(Role::User),
        disabled_at: disabled_at.map(|s| parse_datetime(&s)),
        created_at: parse_datetime(&created_at),
    })
}

pub fn create_user(conn: &Connection, username: &str, password_hash: &str, role: Role) -> AppResult<User> {
    let result = conn.execute(
        "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
        params![username, password_hash, role.as_str()],
    );

    match result {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            find_by_id(conn, id)?.ok_or(AppError::UserNotFound)
        }
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Err(AppError::UsernameExists)
        }
        Err(e) => Err(AppError::Database(e)),
    }
}

pub fn find_by_username(conn: &Connection, username: &str) -> AppResult<Option<User>> {
    conn.query_row(
        "SELECT id, username, password_hash, role, disabled_at, created_at FROM user WHERE username = ?1",
        params![username],
        row_to_user,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<User>> {
    conn.query_row(
        "SELECT id, username, password_hash, role, disabled_at, created_at FROM user WHERE id = ?1",
        params![id],
        row_to_user,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_all(conn: &Connection) -> AppResult<Vec<User>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, password_hash, role, disabled_at, created_at FROM user ORDER BY id",
    )?;

    let users = stmt
        .query_map([], row_to_user)?
        .filter_map(Result::ok)
        .collect();

    Ok(users)
}

pub fn update_password(conn: &Connection, user_id: i64, new_password_hash: &str) -> AppResult<()> {
    let rows = conn.execute(
        "UPDATE user SET password_hash = ?1 WHERE id = ?2",
        params![new_password_hash, user_id],
    )?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub fn update_role(conn: &Connection, user_id: i64, role: Role) -> AppResult<()> {
    let rows = conn.execute(
        "UPDATE user SET role = ?1 WHERE id = ?2",
        params![role.as_str(), user_id],
    )?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub fn disable_user(conn: &Connection, user_id: i64) -> AppResult<()> {
    let rows = conn.execute(
        "UPDATE user SET disabled_at = datetime('now') WHERE id = ?1",
        params![user_id],
    )?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub fn enable_user(conn: &Connection, user_id: i64) -> AppResult<()> {
    let rows = conn.execute(
        "UPDATE user SET disabled_at = NULL WHERE id = ?1",
        params![user_id],
    )?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub fn delete_user(conn: &Connection, user_id: i64) -> AppResult<()> {
    let rows = conn.execute("DELETE FROM user WHERE id = ?1", params![user_id])?;

    if rows == 0 {
        return Err(AppError::UserNotFound);
    }
    Ok(())
}

pub fn count(conn: &Connection) -> AppResult<i64> {
    conn.query_row("SELECT COUNT(*) FROM user", [], |row| row.get(0))
        .map_err(AppError::Database)
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
    fn test_create_and_find_user() {
        let conn = setup_db();

        let user = create_user(&conn, "testuser", "hash123", Role::User).unwrap();
        assert_eq!(user.username, "testuser");
        assert_eq!(user.role, Role::User);
        assert!(!user.is_disabled());

        let found = find_by_username(&conn, "testuser").unwrap().unwrap();
        assert_eq!(found.id, user.id);

        let found_by_id = find_by_id(&conn, user.id).unwrap().unwrap();
        assert_eq!(found_by_id.username, "testuser");
    }

    #[test]
    fn test_duplicate_username() {
        let conn = setup_db();

        create_user(&conn, "testuser", "hash123", Role::User).unwrap();
        let result = create_user(&conn, "testuser", "hash456", Role::User);
        assert!(matches!(result, Err(AppError::UsernameExists)));
    }

    #[test]
    fn test_disable_enable_user() {
        let conn = setup_db();

        let user = create_user(&conn, "testuser", "hash123", Role::User).unwrap();
        assert!(!user.is_disabled());

        disable_user(&conn, user.id).unwrap();
        let disabled = find_by_id(&conn, user.id).unwrap().unwrap();
        assert!(disabled.is_disabled());

        enable_user(&conn, user.id).unwrap();
        let enabled = find_by_id(&conn, user.id).unwrap().unwrap();
        assert!(!enabled.is_disabled());
    }

    #[test]
    fn test_update_role() {
        let conn = setup_db();

        let user = create_user(&conn, "testuser", "hash123", Role::User).unwrap();
        assert_eq!(user.role, Role::User);

        update_role(&conn, user.id, Role::Admin).unwrap();
        let admin = find_by_id(&conn, user.id).unwrap().unwrap();
        assert_eq!(admin.role, Role::Admin);
    }

    #[test]
    fn test_delete_user() {
        let conn = setup_db();

        let user = create_user(&conn, "testuser", "hash123", Role::User).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);

        delete_user(&conn, user.id).unwrap();
        assert_eq!(count(&conn).unwrap(), 0);
        assert!(find_by_id(&conn, user.id).unwrap().is_none());
    }

    #[test]
    fn test_list_all() {
        let conn = setup_db();

        create_user(&conn, "user1", "hash1", Role::Admin).unwrap();
        create_user(&conn, "user2", "hash2", Role::User).unwrap();

        let users = list_all(&conn).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "user1");
        assert_eq!(users[1].username, "user2");
    }

    #[test]
    fn test_count() {
        let conn = setup_db();

        assert_eq!(count(&conn).unwrap(), 0);

        create_user(&conn, "user1", "hash1", Role::User).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);

        create_user(&conn, "user2", "hash2", Role::User).unwrap();
        assert_eq!(count(&conn).unwrap(), 2);
    }
}
