use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};

pub const SESSION_EXPIRY_DAYS: i64 = 7;
const TOKEN_LENGTH: usize = 32;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub session_token: String,
    pub original_user_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl Session {
    pub fn is_masquerading(&self) -> bool {
        self.original_user_id.is_some()
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
}

fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..TOKEN_LENGTH).map(|_| rng.gen()).collect();
    base64_encode(&bytes)
}

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();

    for chunk in data.chunks(3) {
        let n = chunk.len();
        let b0 = chunk[0] as usize;
        let b1 = if n > 1 { chunk[1] as usize } else { 0 };
        let b2 = if n > 2 { chunk[2] as usize } else { 0 };

        result.push(ALPHABET[b0 >> 2] as char);
        result.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        if n > 1 {
            result.push(ALPHABET[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        }
        if n > 2 {
            result.push(ALPHABET[b2 & 0x3f] as char);
        }
    }

    result
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

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    let created_at: String = row.get(4)?;
    let expires_at: String = row.get(5)?;

    Ok(Session {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_token: row.get(2)?,
        original_user_id: row.get(3)?,
        created_at: parse_datetime(&created_at),
        expires_at: parse_datetime(&expires_at),
    })
}

pub fn create_session(conn: &Connection, user_id: i64) -> AppResult<Session> {
    let token = generate_token();
    let expires_at = Utc::now() + Duration::days(SESSION_EXPIRY_DAYS);
    let expires_at_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

    conn.execute(
        "INSERT INTO session (user_id, session_token, expires_at) VALUES (?1, ?2, ?3)",
        params![user_id, token, expires_at_str],
    )?;

    let id = conn.last_insert_rowid();
    find_by_id(conn, id)?.ok_or(AppError::Internal("Failed to create session".to_string()))
}

fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<Session>> {
    conn.query_row(
        "SELECT id, user_id, session_token, original_user_id, created_at, expires_at FROM session WHERE id = ?1",
        params![id],
        row_to_session,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_token(conn: &Connection, token: &str) -> AppResult<Option<Session>> {
    conn.query_row(
        "SELECT id, user_id, session_token, original_user_id, created_at, expires_at FROM session WHERE session_token = ?1",
        params![token],
        row_to_session,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn delete_session(conn: &Connection, token: &str) -> AppResult<()> {
    conn.execute(
        "DELETE FROM session WHERE session_token = ?1",
        params![token],
    )?;
    Ok(())
}

pub fn delete_user_sessions(conn: &Connection, user_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM session WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

#[allow(dead_code)]
pub fn cleanup_expired(conn: &Connection) -> AppResult<usize> {
    let deleted = conn.execute("DELETE FROM session WHERE expires_at < datetime('now')", [])?;
    Ok(deleted)
}

pub fn start_masquerade(conn: &Connection, token: &str, target_user_id: i64) -> AppResult<()> {
    let session = find_by_token(conn, token)?.ok_or(AppError::Unauthorized)?;

    if session.is_masquerading() {
        return Err(AppError::AlreadyMasquerading);
    }

    conn.execute(
        "UPDATE session SET original_user_id = user_id, user_id = ?1 WHERE session_token = ?2",
        params![target_user_id, token],
    )?;

    Ok(())
}

pub fn stop_masquerade(conn: &Connection, token: &str) -> AppResult<()> {
    let session = find_by_token(conn, token)?.ok_or(AppError::Unauthorized)?;

    if !session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    conn.execute(
        "UPDATE session SET user_id = original_user_id, original_user_id = NULL WHERE session_token = ?1",
        params![token],
    )?;

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
    fn test_create_and_find_session() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let session = create_session(&conn, user.id).unwrap();
        assert_eq!(session.user_id, user.id);
        assert!(!session.is_masquerading());
        assert!(!session.is_expired());

        let found = find_by_token(&conn, &session.session_token)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, session.id);
    }

    #[test]
    fn test_delete_session() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let session = create_session(&conn, user.id).unwrap();
        delete_session(&conn, &session.session_token).unwrap();

        let found = find_by_token(&conn, &session.session_token).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_masquerade() {
        let conn = setup_db();
        let admin = user::create_user(&conn, "admin", "hash", Role::Admin).unwrap();
        let target = user::create_user(&conn, "target", "hash", Role::User).unwrap();

        let session = create_session(&conn, admin.id).unwrap();
        assert!(!session.is_masquerading());

        start_masquerade(&conn, &session.session_token, target.id).unwrap();

        let masq = find_by_token(&conn, &session.session_token)
            .unwrap()
            .unwrap();
        assert!(masq.is_masquerading());
        assert_eq!(masq.user_id, target.id);
        assert_eq!(masq.original_user_id, Some(admin.id));

        stop_masquerade(&conn, &session.session_token).unwrap();

        let restored = find_by_token(&conn, &session.session_token)
            .unwrap()
            .unwrap();
        assert!(!restored.is_masquerading());
        assert_eq!(restored.user_id, admin.id);
    }

    #[test]
    fn test_already_masquerading() {
        let conn = setup_db();
        let admin = user::create_user(&conn, "admin", "hash", Role::Admin).unwrap();
        let target = user::create_user(&conn, "target", "hash", Role::User).unwrap();

        let session = create_session(&conn, admin.id).unwrap();
        start_masquerade(&conn, &session.session_token, target.id).unwrap();

        let result = start_masquerade(&conn, &session.session_token, target.id);
        assert!(matches!(result, Err(AppError::AlreadyMasquerading)));
    }

    #[test]
    fn test_not_masquerading() {
        let conn = setup_db();
        let user = user::create_user(&conn, "user", "hash", Role::User).unwrap();

        let session = create_session(&conn, user.id).unwrap();

        let result = stop_masquerade(&conn, &session.session_token);
        assert!(matches!(result, Err(AppError::NotMasquerading)));
    }

    #[test]
    fn test_token_generation() {
        let token1 = generate_token();
        let token2 = generate_token();

        assert_ne!(token1, token2);
        assert!(token1.len() >= 40);
    }
}
