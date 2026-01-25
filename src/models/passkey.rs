use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct Passkey {
    pub id: i64,
    pub user_id: i64,
    #[serde(skip_serializing)]
    pub credential_id: Vec<u8>,
    #[serde(skip_serializing)]
    pub public_key: Vec<u8>,
    pub counter: i64,
    pub name: String,
    pub transports: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
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

fn row_to_passkey(row: &rusqlite::Row) -> rusqlite::Result<Passkey> {
    let created_at: String = row.get(7)?;
    let last_used_at: Option<String> = row.get(8)?;

    Ok(Passkey {
        id: row.get(0)?,
        user_id: row.get(1)?,
        credential_id: row.get(2)?,
        public_key: row.get(3)?,
        counter: row.get(4)?,
        name: row.get(5)?,
        transports: row.get(6)?,
        created_at: parse_datetime(&created_at),
        last_used_at: last_used_at.map(|s| parse_datetime(&s)),
    })
}

pub fn create_passkey(
    conn: &Connection,
    user_id: i64,
    credential_id: &[u8],
    public_key: &[u8],
    counter: i64,
    name: &str,
    transports: Option<&str>,
) -> AppResult<Passkey> {
    conn.execute(
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![user_id, credential_id, public_key, counter, name, transports],
    )?;

    let id = conn.last_insert_rowid();
    find_by_id(conn, id)?.ok_or(AppError::Internal("Failed to create passkey".to_string()))
}

pub fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<Passkey>> {
    conn.query_row(
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE id = ?1",
        params![id],
        row_to_passkey,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_credential_id(conn: &Connection, credential_id: &[u8]) -> AppResult<Option<Passkey>> {
    conn.query_row(
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE credential_id = ?1",
        params![credential_id],
        row_to_passkey,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_by_user(conn: &Connection, user_id: i64) -> AppResult<Vec<Passkey>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey WHERE user_id = ?1 ORDER BY created_at DESC",
    )?;
    let passkeys = stmt
        .query_map(params![user_id], row_to_passkey)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(passkeys)
}

pub fn get_all_passkeys(conn: &Connection) -> AppResult<Vec<Passkey>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at FROM passkey ORDER BY user_id, created_at DESC",
    )?;
    let passkeys = stmt
        .query_map([], row_to_passkey)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(passkeys)
}

pub fn update_counter(conn: &Connection, id: i64, counter: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE passkey SET counter = ?1, last_used_at = datetime('now') WHERE id = ?2",
        params![counter, id],
    )?;
    Ok(())
}

pub fn rename_passkey(conn: &Connection, id: i64, user_id: i64, name: &str) -> AppResult<()> {
    let updated = conn.execute(
        "UPDATE passkey SET name = ?1 WHERE id = ?2 AND user_id = ?3",
        params![name, id, user_id],
    )?;
    if updated == 0 {
        return Err(AppError::PasskeyNotFound);
    }
    Ok(())
}

pub fn delete_passkey(conn: &Connection, id: i64, user_id: i64) -> AppResult<()> {
    let deleted = conn.execute(
        "DELETE FROM passkey WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    if deleted == 0 {
        return Err(AppError::PasskeyNotFound);
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

    #[test]
    fn test_create_and_find_passkey() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let credential_id = vec![1, 2, 3, 4];
        let public_key = vec![5, 6, 7, 8];

        let passkey = create_passkey(
            &conn,
            user.id,
            &credential_id,
            &public_key,
            0,
            "My Passkey",
            Some("usb,nfc"),
        )
        .unwrap();

        assert_eq!(passkey.user_id, user.id);
        assert_eq!(passkey.credential_id, credential_id);
        assert_eq!(passkey.name, "My Passkey");

        let found = find_by_id(&conn, passkey.id).unwrap().unwrap();
        assert_eq!(found.id, passkey.id);

        let found = find_by_credential_id(&conn, &credential_id).unwrap().unwrap();
        assert_eq!(found.id, passkey.id);
    }

    #[test]
    fn test_list_passkeys() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        create_passkey(&conn, user.id, &[1], &[1], 0, "Passkey 1", None).unwrap();
        create_passkey(&conn, user.id, &[2], &[2], 0, "Passkey 2", None).unwrap();

        let passkeys = list_by_user(&conn, user.id).unwrap();
        assert_eq!(passkeys.len(), 2);
    }

    #[test]
    fn test_update_counter() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let passkey = create_passkey(&conn, user.id, &[1], &[1], 0, "Passkey", None).unwrap();
        assert_eq!(passkey.counter, 0);
        assert!(passkey.last_used_at.is_none());

        update_counter(&conn, passkey.id, 5).unwrap();

        let updated = find_by_id(&conn, passkey.id).unwrap().unwrap();
        assert_eq!(updated.counter, 5);
        assert!(updated.last_used_at.is_some());
    }

    #[test]
    fn test_rename_passkey() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let passkey = create_passkey(&conn, user.id, &[1], &[1], 0, "Old Name", None).unwrap();
        rename_passkey(&conn, passkey.id, user.id, "New Name").unwrap();

        let updated = find_by_id(&conn, passkey.id).unwrap().unwrap();
        assert_eq!(updated.name, "New Name");
    }

    #[test]
    fn test_delete_passkey() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let passkey = create_passkey(&conn, user.id, &[1], &[1], 0, "Passkey", None).unwrap();
        delete_passkey(&conn, passkey.id, user.id).unwrap();

        let found = find_by_id(&conn, passkey.id).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_delete_passkey_wrong_user() {
        let conn = setup_db();
        let user1 = user::create_user(&conn, "user1", "hash", Role::User).unwrap();
        let user2 = user::create_user(&conn, "user2", "hash", Role::User).unwrap();

        let passkey = create_passkey(&conn, user1.id, &[1], &[1], 0, "Passkey", None).unwrap();

        let result = delete_passkey(&conn, passkey.id, user2.id);
        assert!(matches!(result, Err(AppError::PasskeyNotFound)));
    }
}
