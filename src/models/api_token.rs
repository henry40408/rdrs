//! `GReader` `ClientLogin` credentials, deliberately independent of `session`.
//!
//! A token minted here is its own row: its own expiry, its own revocation, and
//! nothing that touches it ever slides a browser session's expiry or shows up
//! in the "Active Sessions" list. See `handlers::greader::auth` for how it is
//! issued and validated.

use chrono::{DateTime, Duration, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::session;
use crate::{db_execute, query_all, query_one, query_opt};

/// `GReader` clients cannot re-authenticate interactively — the user has to go
/// back into the app and retype a password — so this credential uses a longer
/// sliding idle TTL and, deliberately, **no absolute cap**: a client that keeps
/// syncing stays valid, an idle one dies on its own. That is a conscious
/// divergence from the web session's 7-day slide plus 90-day hard cap.
pub const API_TOKEN_IDLE_DAYS: i64 = 90;

/// Token prefix, so a leaked string is recognisable in logs and secret scanners
/// and can never be confused with a session token.
pub const API_TOKEN_PREFIX: &str = "rdrs_gr_";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiToken {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
    pub kind: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub user_agent: String,
    pub ip_address: String,
}

impl ApiToken {
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// New `expires_at` when less than half of `API_TOKEN_IDLE_DAYS` remains.
    /// Shaped like `session::Session::compute_refreshed_expiry` but with no
    /// absolute-cap branch — see the module doc for why.
    pub fn compute_refreshed_expiry(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let ttl = Duration::days(API_TOKEN_IDLE_DAYS);
        if self.expires_at - now >= ttl / 2 {
            return None;
        }
        Some(now + ttl)
    }
}

/// `API_TOKEN_PREFIX` + `session::generate_token()`. The prefix characters are
/// all within the `A-Za-z0-9-_` alphabet, so the token still never contains
/// `/` — which `greader::auth::post_token_parts` relies on for its MAC input to
/// be unambiguous (see the comment at src/handlers/greader/auth.rs:157-159).
pub fn generate_token() -> String {
    format!("{API_TOKEN_PREFIX}{}", session::generate_token())
}

pub async fn create_api_token(
    db: &Db,
    user_id: i64,
    kind: &str,
    label: &str,
    user_agent: &str,
    ip_address: &str,
) -> AppResult<ApiToken> {
    let token = generate_token();
    let now = Utc::now();
    let expires_at = now + Duration::days(API_TOKEN_IDLE_DAYS);

    query_one!(
        db,
        ApiToken,
        "INSERT INTO api_token (user_id, token, kind, label, expires_at, last_seen_at, user_agent, ip_address) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, user_id, token, kind, label, created_at, expires_at, last_seen_at, user_agent, ip_address",
        user_id,
        &token,
        kind,
        label,
        expires_at,
        now,
        user_agent,
        ip_address
    )
    .map_err(AppError::Database)
}

pub async fn find_by_token(db: &Db, token: &str) -> AppResult<Option<ApiToken>> {
    query_opt!(
        db,
        ApiToken,
        "SELECT id, user_id, token, kind, label, created_at, expires_at, last_seen_at, user_agent, ip_address \
         FROM api_token WHERE token = $1",
        token
    )
    .map_err(AppError::Database)
}

/// Writes `last_seen_at` at most once a minute (mirroring
/// `session::touch_last_seen`) and slides `expires_at` when due. Best-effort
/// from the caller's point of view: failure here must not block the request
/// the token is authenticating.
pub async fn touch_and_refresh(db: &Db, t: &ApiToken) -> AppResult<Option<DateTime<Utc>>> {
    let now = Utc::now();
    if now - t.last_seen_at >= Duration::minutes(1) {
        db_execute!(
            db,
            "UPDATE api_token SET last_seen_at = $1 WHERE id = $2",
            now,
            t.id
        )
        .map_err(AppError::Database)?;
    }

    let Some(new_expires_at) = t.compute_refreshed_expiry(now) else {
        return Ok(None);
    };
    db_execute!(
        db,
        "UPDATE api_token SET expires_at = $1 WHERE id = $2",
        new_expires_at,
        t.id
    )
    .map_err(AppError::Database)?;
    Ok(Some(new_expires_at))
}

/// All tokens belonging to `user_id`, newest first.
pub async fn list_user_tokens(db: &Db, user_id: i64) -> AppResult<Vec<ApiToken>> {
    query_all!(
        db,
        ApiToken,
        "SELECT id, user_id, token, kind, label, created_at, expires_at, last_seen_at, user_agent, ip_address \
         FROM api_token WHERE user_id = $1 ORDER BY created_at DESC",
        user_id
    )
    .map_err(AppError::Database)
}

/// `user_id`-scoped so one user can never revoke another's token.
pub async fn delete_token(db: &Db, id: i64, user_id: i64) -> AppResult<()> {
    db_execute!(
        db,
        "DELETE FROM api_token WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

pub async fn delete_user_tokens(db: &Db, user_id: i64) -> AppResult<()> {
    db_execute!(db, "DELETE FROM api_token WHERE user_id = $1", user_id)
        .map_err(AppError::Database)?;
    Ok(())
}

/// Delete every expired token row. Called periodically by the cleanup worker
/// (`services::summary_cleanup`), the backstop for the lazy delete in
/// `handlers/greader/auth.rs`'s `validate_api_token`. The bound `now` (rather
/// than SQL `datetime('now')`) keeps this a plain `db_execute!` call that
/// behaves identically on both dialects without needing the `pg_rewrite` shim.
pub async fn delete_expired(db: &Db) -> AppResult<u64> {
    let now = Utc::now();
    db_execute!(db, "DELETE FROM api_token WHERE expires_at <= $1", now).map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_find_api_token() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let token = create_api_token(
            &db,
            user.id,
            "greader",
            "some-client",
            "test-agent",
            "127.0.0.1",
        )
        .await
        .unwrap();
        assert_eq!(token.user_id, user.id);
        assert_eq!(token.kind, "greader");
        assert_eq!(token.label, "some-client");
        assert!(!token.is_expired());

        let found = find_by_token(&db, &token.token).await.unwrap().unwrap();
        assert_eq!(found.id, token.id);
    }

    #[tokio::test]
    async fn test_token_has_prefix_and_no_slash() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let token = create_api_token(&db, user.id, "greader", "", "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert!(token.token.starts_with(API_TOKEN_PREFIX));
        assert!(
            !token.token.contains('/'),
            "post_token_parts relies on tokens never containing '/'"
        );
    }

    #[tokio::test]
    async fn test_delete_token_is_user_scoped() {
        let db = setup_db().await;
        let user_a = user::create_user(&db, "usera", "hash", Role::User)
            .await
            .unwrap();
        let user_b = user::create_user(&db, "userb", "hash", Role::User)
            .await
            .unwrap();

        let token_a = create_api_token(&db, user_a.id, "greader", "", "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // user B tries to revoke user A's token — must be a no-op.
        delete_token(&db, token_a.id, user_b.id).await.unwrap();

        assert!(
            find_by_token(&db, &token_a.token).await.unwrap().is_some(),
            "user A's token must survive user B's revoke attempt"
        );

        delete_token(&db, token_a.id, user_a.id).await.unwrap();
        assert!(find_by_token(&db, &token_a.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_expired_removes_only_expired() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let expired = create_api_token(&db, user.id, "greader", "", "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let past = Utc::now() - Duration::days(1);
        db_execute!(
            &db,
            "UPDATE api_token SET expires_at = $1 WHERE id = $2",
            past,
            expired.id
        )
        .unwrap();

        let fresh = create_api_token(&db, user.id, "greader", "", "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let deleted = delete_expired(&db).await.unwrap();
        assert_eq!(deleted, 1);

        assert!(find_by_token(&db, &expired.token).await.unwrap().is_none());
        assert!(find_by_token(&db, &fresh.token).await.unwrap().is_some());
    }

    fn make_token(created_at: DateTime<Utc>, expires_at: DateTime<Utc>) -> ApiToken {
        ApiToken {
            id: 1,
            user_id: 1,
            token: "t".to_string(),
            kind: "greader".to_string(),
            label: String::new(),
            created_at,
            expires_at,
            last_seen_at: created_at,
            user_agent: "t".to_string(),
            ip_address: "127.0.0.1".to_string(),
        }
    }

    #[test]
    fn compute_refreshed_expiry_skips_fresh() {
        let now = Utc::now();
        let token = make_token(now, now + Duration::days(API_TOKEN_IDLE_DAYS));
        assert!(token.compute_refreshed_expiry(now).is_none());
    }

    #[test]
    fn compute_refreshed_expiry_extends_past_half_ttl() {
        let created = Utc::now() - Duration::days(60);
        let expires = created + Duration::days(API_TOKEN_IDLE_DAYS);
        let now = Utc::now();
        let token = make_token(created, expires);

        let new_expires = token.compute_refreshed_expiry(now).expect("should extend");
        assert!(new_expires > expires);
    }

    #[test]
    fn compute_refreshed_expiry_has_no_absolute_cap() {
        // A token created well over the idle window ago — far past what would
        // be a session's absolute cap — is still extended, because api_token
        // deliberately has no absolute-cap branch.
        let created = Utc::now() - Duration::days(400);
        let expires = Utc::now() + Duration::hours(1);
        let now = Utc::now();
        let token = make_token(created, expires);

        let new_expires = token
            .compute_refreshed_expiry(now)
            .expect("a 400-day-old token must still be extended");
        assert_eq!(new_expires, now + Duration::days(API_TOKEN_IDLE_DAYS));
    }
}
