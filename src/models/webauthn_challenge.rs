use chrono::{DateTime, Duration, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_one, query_opt};

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

/// Row-shaped decode target: `challenge_type` is stored as TEXT, so it is read
/// as `String` here and mapped to the `ChallengeType` enum in `From`. This keeps
/// the storage backend-agnostic (plain TEXT/VARCHAR on both `SQLite` and Postgres)
/// and preserves the original default-to-`Registration` behavior for any value
/// that does not parse.
#[derive(Debug, Clone, sqlx::FromRow)]
struct WebauthnChallengeRow {
    pub id: i64,
    pub challenge: Vec<u8>,
    pub user_id: Option<i64>,
    pub challenge_type: String,
    pub state_data: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl From<WebauthnChallengeRow> for WebauthnChallenge {
    fn from(row: WebauthnChallengeRow) -> Self {
        WebauthnChallenge {
            id: row.id,
            challenge: row.challenge,
            user_id: row.user_id,
            challenge_type: ChallengeType::from_str(&row.challenge_type)
                .unwrap_or(ChallengeType::Registration),
            state_data: row.state_data,
            created_at: row.created_at,
            expires_at: row.expires_at,
        }
    }
}

pub async fn create_challenge(
    db: &Db,
    challenge: &[u8],
    user_id: Option<i64>,
    challenge_type: ChallengeType,
    state_data: &str,
) -> AppResult<WebauthnChallenge> {
    let expires_at = Utc::now() + Duration::minutes(CHALLENGE_EXPIRY_MINUTES);

    let row = query_one!(
        db,
        WebauthnChallengeRow,
        "INSERT INTO webauthn_challenge (challenge, user_id, challenge_type, state_data, expires_at) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, challenge, user_id, challenge_type, state_data, created_at, expires_at",
        challenge,
        user_id,
        challenge_type.as_str(),
        state_data,
        expires_at
    )
    .map_err(AppError::Database)?;

    Ok(row.into())
}

pub async fn find_and_delete_challenge(
    db: &Db,
    user_id: Option<i64>,
    challenge_type: ChallengeType,
) -> AppResult<WebauthnChallenge> {
    let now = Utc::now();

    let row = match user_id {
        Some(uid) => query_opt!(
            db,
            WebauthnChallengeRow,
            "SELECT id, challenge, user_id, challenge_type, state_data, created_at, expires_at \
             FROM webauthn_challenge \
             WHERE user_id = $1 AND challenge_type = $2 AND expires_at > $3 \
             ORDER BY created_at DESC LIMIT 1",
            uid,
            challenge_type.as_str(),
            now
        ),
        None => query_opt!(
            db,
            WebauthnChallengeRow,
            "SELECT id, challenge, user_id, challenge_type, state_data, created_at, expires_at \
             FROM webauthn_challenge \
             WHERE user_id IS NULL AND challenge_type = $1 AND expires_at > $2 \
             ORDER BY created_at DESC LIMIT 1",
            challenge_type.as_str(),
            now
        ),
    }
    .map_err(AppError::Database)?
    .ok_or(AppError::ChallengeNotFound)?;

    // Delete the challenge after retrieval
    db_execute!(db, "DELETE FROM webauthn_challenge WHERE id = $1", row.id)
        .map_err(AppError::Database)?;

    Ok(row.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_find_challenge() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let challenge_bytes = vec![1, 2, 3, 4];
        let state_data = r#"{"some":"data"}"#;

        let challenge = create_challenge(
            &db,
            &challenge_bytes,
            Some(user.id),
            ChallengeType::Registration,
            state_data,
        )
        .await
        .unwrap();

        assert_eq!(challenge.challenge, challenge_bytes);
        assert_eq!(challenge.user_id, Some(user.id));
        assert_eq!(challenge.challenge_type, ChallengeType::Registration);
        assert_eq!(challenge.state_data, state_data);
    }

    #[tokio::test]
    async fn test_find_and_delete_challenge() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let challenge_bytes = vec![1, 2, 3, 4];
        create_challenge(
            &db,
            &challenge_bytes,
            Some(user.id),
            ChallengeType::Registration,
            "{}",
        )
        .await
        .unwrap();

        let found = find_and_delete_challenge(&db, Some(user.id), ChallengeType::Registration)
            .await
            .unwrap();
        assert_eq!(found.challenge, challenge_bytes);

        // Should be deleted
        let result =
            find_and_delete_challenge(&db, Some(user.id), ChallengeType::Registration).await;
        assert!(matches!(result, Err(AppError::ChallengeNotFound)));
    }

    #[tokio::test]
    async fn test_authentication_challenge_no_user() {
        let db = setup_db().await;

        let challenge_bytes = vec![5, 6, 7, 8];
        create_challenge(
            &db,
            &challenge_bytes,
            None,
            ChallengeType::Authentication,
            "{}",
        )
        .await
        .unwrap();

        let found = find_and_delete_challenge(&db, None, ChallengeType::Authentication)
            .await
            .unwrap();
        assert_eq!(found.challenge, challenge_bytes);
        assert!(found.user_id.is_none());
    }

    #[tokio::test]
    async fn test_challenge_type_conversion() {
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
}
