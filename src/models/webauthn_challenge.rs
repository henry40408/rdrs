use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};

const CHALLENGE_EXPIRY_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChallengeType {
    Registration,
    Authentication,
}

impl ChallengeType {
    fn as_str(&self) -> &'static str {
        match self {
            ChallengeType::Registration => "registration",
            ChallengeType::Authentication => "authentication",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "registration" => Some(ChallengeType::Registration),
            "authentication" => Some(ChallengeType::Authentication),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebauthnChallenge {
    pub id: i64,
    pub challenge: Vec<u8>,
    pub user_id: Option<i64>,
    pub challenge_type: ChallengeType,
    pub state_data: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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

fn row_to_challenge(row: &rusqlite::Row) -> rusqlite::Result<WebauthnChallenge> {
    let challenge_type_str: String = row.get(3)?;
    let created_at: String = row.get(5)?;
    let expires_at: String = row.get(6)?;

    Ok(WebauthnChallenge {
        id: row.get(0)?,
        challenge: row.get(1)?,
        user_id: row.get(2)?,
        challenge_type: ChallengeType::from_str(&challenge_type_str)
            .unwrap_or(ChallengeType::Registration),
        state_data: row.get(4)?,
        created_at: parse_datetime(&created_at),
        expires_at: parse_datetime(&expires_at),
    })
}

pub fn create_challenge(
    conn: &Connection,
    challenge: &[u8],
    user_id: Option<i64>,
    challenge_type: ChallengeType,
    state_data: &str,
) -> AppResult<WebauthnChallenge> {
    let expires_at = Utc::now() + Duration::minutes(CHALLENGE_EXPIRY_MINUTES);
    let expires_at_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

    conn.execute(
        "INSERT INTO webauthn_challenge (challenge, user_id, challenge_type, state_data, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![challenge, user_id, challenge_type.as_str(), state_data, expires_at_str],
    )?;

    let id = conn.last_insert_rowid();
    find_by_id(conn, id)?.ok_or(AppError::Internal("Failed to create challenge".to_string()))
}

fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<WebauthnChallenge>> {
    conn.query_row(
        "SELECT id, challenge, user_id, challenge_type, state_data, created_at, expires_at FROM webauthn_challenge WHERE id = ?1",
        params![id],
        row_to_challenge,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_and_delete_challenge(
    conn: &Connection,
    user_id: Option<i64>,
    challenge_type: ChallengeType,
) -> AppResult<WebauthnChallenge> {
    let challenge = match user_id {
        Some(uid) => conn.query_row(
            "SELECT id, challenge, user_id, challenge_type, state_data, created_at, expires_at FROM webauthn_challenge WHERE user_id = ?1 AND challenge_type = ?2 AND expires_at > datetime('now') ORDER BY created_at DESC LIMIT 1",
            params![uid, challenge_type.as_str()],
            row_to_challenge,
        ),
        None => conn.query_row(
            "SELECT id, challenge, user_id, challenge_type, state_data, created_at, expires_at FROM webauthn_challenge WHERE user_id IS NULL AND challenge_type = ?1 AND expires_at > datetime('now') ORDER BY created_at DESC LIMIT 1",
            params![challenge_type.as_str()],
            row_to_challenge,
        ),
    }
    .optional()
    .map_err(AppError::Database)?
    .ok_or(AppError::ChallengeNotFound)?;

    // Delete the challenge after retrieval
    conn.execute(
        "DELETE FROM webauthn_challenge WHERE id = ?1",
        params![challenge.id],
    )?;

    Ok(challenge)
}

pub fn cleanup_expired(conn: &Connection) -> AppResult<usize> {
    let deleted = conn.execute(
        "DELETE FROM webauthn_challenge WHERE expires_at < datetime('now')",
        [],
    )?;
    Ok(deleted)
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
    fn test_create_and_find_challenge() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let challenge_bytes = vec![1, 2, 3, 4];
        let state_data = r#"{"some":"data"}"#;

        let challenge = create_challenge(
            &conn,
            &challenge_bytes,
            Some(user.id),
            ChallengeType::Registration,
            state_data,
        )
        .unwrap();

        assert_eq!(challenge.challenge, challenge_bytes);
        assert_eq!(challenge.user_id, Some(user.id));
        assert_eq!(challenge.challenge_type, ChallengeType::Registration);
        assert_eq!(challenge.state_data, state_data);
    }

    #[test]
    fn test_find_and_delete_challenge() {
        let conn = setup_db();
        let user = user::create_user(&conn, "testuser", "hash", Role::User).unwrap();

        let challenge_bytes = vec![1, 2, 3, 4];
        create_challenge(
            &conn,
            &challenge_bytes,
            Some(user.id),
            ChallengeType::Registration,
            "{}",
        )
        .unwrap();

        let found =
            find_and_delete_challenge(&conn, Some(user.id), ChallengeType::Registration).unwrap();
        assert_eq!(found.challenge, challenge_bytes);

        // Should be deleted
        let result = find_and_delete_challenge(&conn, Some(user.id), ChallengeType::Registration);
        assert!(matches!(result, Err(AppError::ChallengeNotFound)));
    }

    #[test]
    fn test_authentication_challenge_no_user() {
        let conn = setup_db();

        let challenge_bytes = vec![5, 6, 7, 8];
        create_challenge(
            &conn,
            &challenge_bytes,
            None,
            ChallengeType::Authentication,
            "{}",
        )
        .unwrap();

        let found = find_and_delete_challenge(&conn, None, ChallengeType::Authentication).unwrap();
        assert_eq!(found.challenge, challenge_bytes);
        assert!(found.user_id.is_none());
    }

    #[test]
    fn test_challenge_type_conversion() {
        assert_eq!(ChallengeType::Registration.as_str(), "registration");
        assert_eq!(ChallengeType::Authentication.as_str(), "authentication");
        assert_eq!(
            ChallengeType::from_str("registration"),
            Some(ChallengeType::Registration)
        );
        assert_eq!(
            ChallengeType::from_str("authentication"),
            Some(ChallengeType::Authentication)
        );
        assert_eq!(ChallengeType::from_str("invalid"), None);
    }

    #[test]
    fn test_cleanup_expired() {
        let conn = setup_db();

        // Create an expired challenge by inserting directly with past expiry
        conn.execute(
            "INSERT INTO webauthn_challenge (challenge, challenge_type, state_data, expires_at) VALUES (?1, ?2, ?3, datetime('now', '-1 hour'))",
            params![vec![1u8, 2, 3], "registration", "{}"],
        )
        .unwrap();

        // Create a valid challenge
        create_challenge(&conn, &[4, 5, 6], None, ChallengeType::Registration, "{}").unwrap();

        // Cleanup should delete 1 expired challenge
        let deleted = cleanup_expired(&conn).unwrap();
        assert_eq!(deleted, 1);

        // Valid challenge should still exist
        let found = find_and_delete_challenge(&conn, None, ChallengeType::Registration);
        assert!(found.is_ok());
    }
}
