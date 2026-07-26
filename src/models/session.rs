use chrono::{DateTime, Duration, Utc};
use rand::RngExt;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt};

pub const SESSION_EXPIRY_DAYS: i64 = 7;
pub const SESSION_ABSOLUTE_MAX_DAYS: i64 = 90;
const TOKEN_LENGTH: usize = 32;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub session_token: String,
    pub original_user_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_agent: String,
    pub ip_address: String,
    pub last_seen_at: DateTime<Utc>,
}

impl Session {
    pub fn is_masquerading(&self) -> bool {
        self.original_user_id.is_some()
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Compute a new `expires_at` if the session should be slid forward.
    ///
    /// Returns `Some(new_expires_at)` when remaining TTL has fallen below
    /// half of `SESSION_EXPIRY_DAYS` and the session has not yet reached
    /// its absolute cap (`created_at + SESSION_ABSOLUTE_MAX_DAYS`).
    /// Otherwise returns `None`.
    pub fn compute_refreshed_expiry(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let ttl = Duration::days(SESSION_EXPIRY_DAYS);
        let absolute_cap = self.created_at + Duration::days(SESSION_ABSOLUTE_MAX_DAYS);

        if self.expires_at >= absolute_cap {
            return None;
        }
        if self.expires_at - now >= ttl / 2 {
            return None;
        }

        Some((now + ttl).min(absolute_cap))
    }
}

/// A fresh random session token. Public so the anonymous-session middleware can
/// mint a signed cookie for a logged-out visitor without opening a database row
/// — the token only needs to be unguessable and to carry a valid signature; the
/// CSRF token derives from it whether or not a `session` row ever exists.
pub fn generate_token() -> String {
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..TOKEN_LENGTH).map(|_| rng.random()).collect();
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

pub async fn create_session(
    db: &Db,
    user_id: i64,
    user_agent: &str,
    ip_address: &str,
) -> AppResult<Session> {
    let token = generate_token();
    let now = Utc::now();
    let expires_at = now + Duration::days(SESSION_EXPIRY_DAYS);

    query_one!(
        db,
        Session,
        "INSERT INTO session (user_id, session_token, expires_at, user_agent, ip_address, last_seen_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, user_id, session_token, original_user_id, created_at, expires_at, user_agent, ip_address, last_seen_at",
        user_id,
        &token,
        expires_at,
        user_agent,
        ip_address,
        now
    )
    .map_err(AppError::Database)
}

pub async fn find_by_token(db: &Db, token: &str) -> AppResult<Option<Session>> {
    query_opt!(
        db,
        Session,
        "SELECT id, user_id, session_token, original_user_id, created_at, expires_at, user_agent, ip_address, last_seen_at FROM session WHERE session_token = $1",
        token
    )
    .map_err(AppError::Database)
}

/// Bump `last_seen_at` to now, but at most once per minute per session, so an
/// active user's every request doesn't cause a write. Best-effort.
pub async fn touch_last_seen(db: &Db, session: &Session) -> AppResult<()> {
    let now = Utc::now();
    if now - session.last_seen_at < Duration::minutes(1) {
        return Ok(());
    }
    db_execute!(
        db,
        "UPDATE session SET last_seen_at = $1 WHERE id = $2",
        now,
        session.id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Slide the session's `expires_at` forward if it is within the refresh window.
///
/// Returns the new `expires_at` when the session was extended, or `None` when
/// no update was necessary (session still has plenty of TTL, or it has hit the
/// absolute cap of `created_at + SESSION_ABSOLUTE_MAX_DAYS`).
pub async fn refresh_if_needed(db: &Db, session: &Session) -> AppResult<Option<DateTime<Utc>>> {
    let Some(new_expires_at) = session.compute_refreshed_expiry(Utc::now()) else {
        return Ok(None);
    };
    db_execute!(
        db,
        "UPDATE session SET expires_at = $1 WHERE id = $2",
        new_expires_at,
        session.id
    )
    .map_err(AppError::Database)?;
    Ok(Some(new_expires_at))
}

pub async fn delete_session(db: &Db, token: &str) -> AppResult<()> {
    db_execute!(db, "DELETE FROM session WHERE session_token = $1", token)
        .map_err(AppError::Database)?;
    Ok(())
}

pub async fn delete_user_sessions(db: &Db, user_id: i64) -> AppResult<()> {
    db_execute!(db, "DELETE FROM session WHERE user_id = $1", user_id)
        .map_err(AppError::Database)?;
    Ok(())
}

/// All sessions belonging to `user_id`, newest first. Includes expired rows;
/// the caller filters for display if needed.
pub async fn list_user_sessions(db: &Db, user_id: i64) -> AppResult<Vec<Session>> {
    query_all!(
        db,
        Session,
        "SELECT id, user_id, session_token, original_user_id, created_at, expires_at, user_agent, ip_address, last_seen_at \
         FROM session WHERE user_id = $1 ORDER BY created_at DESC",
        user_id
    )
    .map_err(AppError::Database)
}

/// Delete every session of `user_id` except the one whose token is `keep_token`.
/// Used by "sign out other sessions" to preserve the caller's current session.
pub async fn delete_user_sessions_except(db: &Db, user_id: i64, keep_token: &str) -> AppResult<()> {
    db_execute!(
        db,
        "DELETE FROM session WHERE user_id = $1 AND session_token <> $2",
        user_id,
        keep_token
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Delete every expired session row.
///
/// Called periodically by the cleanup worker (`services::summary_cleanup`).
/// This is the backstop for the lazy deletes in `middleware/auth.rs`'s
/// `AuthUser`/`PageAuthUser` and `handlers/greader/auth.rs`'s `GReaderUser`,
/// which only fire when a row is actually touched — a session abandoned on a
/// device the user never returns to is never touched again and would
/// otherwise live forever. Covered by `idx_session_expires_at`. The bound
/// `now` (rather than SQL `datetime('now')`) keeps this a plain `db_execute!`
/// call that behaves identically on both dialects without needing the
/// `pg_rewrite` shim.
pub async fn delete_expired(db: &Db) -> AppResult<u64> {
    let now = Utc::now();
    db_execute!(db, "DELETE FROM session WHERE expires_at <= $1", now).map_err(AppError::Database)
}

pub async fn start_masquerade(db: &Db, token: &str, target_user_id: i64) -> AppResult<()> {
    let session = find_by_token(db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if session.is_masquerading() {
        return Err(AppError::AlreadyMasquerading);
    }

    db_execute!(
        db,
        "UPDATE session SET original_user_id = user_id, user_id = $1 WHERE session_token = $2",
        target_user_id,
        token
    )
    .map_err(AppError::Database)?;

    Ok(())
}

pub async fn stop_masquerade(db: &Db, token: &str) -> AppResult<()> {
    let session = find_by_token(db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    db_execute!(
        db,
        "UPDATE session SET user_id = original_user_id, original_user_id = NULL WHERE session_token = $1",
        token
    )
    .map_err(AppError::Database)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_create_and_find_session() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert_eq!(session.user_id, user.id);
        assert!(!session.is_masquerading());
        assert!(!session.is_expired());
        assert_eq!(session.user_agent, "test-agent");
        assert_eq!(session.ip_address, "127.0.0.1");
        // last_seen_at and expires_at are derived from the same `now` in
        // create_session, so they should be exactly `SESSION_EXPIRY_DAYS` apart.
        assert_eq!(
            session.last_seen_at,
            session.expires_at - Duration::days(SESSION_EXPIRY_DAYS)
        );

        let found = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, session.id);
    }

    #[tokio::test]
    async fn test_delete_session() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        delete_session(&db, &session.session_token).await.unwrap();

        let found = find_by_token(&db, &session.session_token).await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_list_user_sessions() {
        let db = setup_db().await;
        let user_a = user::create_user(&db, "usera", "hash", Role::User)
            .await
            .unwrap();
        let user_b = user::create_user(&db, "userb", "hash", Role::User)
            .await
            .unwrap();

        create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        create_session(&db, user_b.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let sessions = list_user_sessions(&db, user_a.id).await.unwrap();
        assert_eq!(sessions.len(), 2);
        for s in &sessions {
            assert_eq!(s.user_id, user_a.id);
        }
    }

    #[tokio::test]
    async fn test_delete_user_sessions_except() {
        let db = setup_db().await;
        let user_a = user::create_user(&db, "usera", "hash", Role::User)
            .await
            .unwrap();
        let user_b = user::create_user(&db, "userb", "hash", Role::User)
            .await
            .unwrap();

        let keep = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let other = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let b_session = create_session(&db, user_b.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        delete_user_sessions_except(&db, user_a.id, &keep.session_token)
            .await
            .unwrap();

        assert!(
            find_by_token(&db, &other.session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_by_token(&db, &keep.session_token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_token(&db, &b_session.session_token)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_masquerade() {
        let db = setup_db().await;
        let admin = user::create_user(&db, "admin", "hash", Role::Admin)
            .await
            .unwrap();
        let target = user::create_user(&db, "target", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert!(!session.is_masquerading());

        start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        let masq = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert!(masq.is_masquerading());
        assert_eq!(masq.user_id, target.id);
        assert_eq!(masq.original_user_id, Some(admin.id));

        stop_masquerade(&db, &session.session_token).await.unwrap();

        let restored = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert!(!restored.is_masquerading());
        assert_eq!(restored.user_id, admin.id);
    }

    #[tokio::test]
    async fn test_already_masquerading() {
        let db = setup_db().await;
        let admin = user::create_user(&db, "admin", "hash", Role::Admin)
            .await
            .unwrap();
        let target = user::create_user(&db, "target", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        let result = start_masquerade(&db, &session.session_token, target.id).await;
        assert!(matches!(result, Err(AppError::AlreadyMasquerading)));
    }

    #[tokio::test]
    async fn test_not_masquerading() {
        let db = setup_db().await;
        let user = user::create_user(&db, "user", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let result = stop_masquerade(&db, &session.session_token).await;
        assert!(matches!(result, Err(AppError::NotMasquerading)));
    }

    #[test]
    fn test_token_generation() {
        let token1 = generate_token();
        let token2 = generate_token();

        assert_ne!(token1, token2);
        assert!(token1.len() >= 40);
    }

    fn make_session(created_at: DateTime<Utc>, expires_at: DateTime<Utc>) -> Session {
        Session {
            id: 1,
            user_id: 1,
            session_token: "t".to_string(),
            original_user_id: None,
            created_at,
            expires_at,
            user_agent: "t".to_string(),
            ip_address: "127.0.0.1".to_string(),
            last_seen_at: created_at,
        }
    }

    #[test]
    fn compute_refreshed_expiry_skips_fresh_session() {
        let now = Utc::now();
        let session = make_session(now, now + Duration::days(SESSION_EXPIRY_DAYS));
        assert!(session.compute_refreshed_expiry(now).is_none());
    }

    #[test]
    fn compute_refreshed_expiry_extends_when_past_half_ttl() {
        let created = Utc::now() - Duration::days(5);
        let expires = created + Duration::days(SESSION_EXPIRY_DAYS);
        let now = Utc::now();
        let session = make_session(created, expires);

        let new_expires = session
            .compute_refreshed_expiry(now)
            .expect("should extend");
        assert!(new_expires > expires);
        assert!(new_expires <= now + Duration::days(SESSION_EXPIRY_DAYS));
    }

    #[test]
    fn compute_refreshed_expiry_caps_at_absolute_max() {
        let created = Utc::now() - Duration::days(SESSION_ABSOLUTE_MAX_DAYS - 2);
        let expires = Utc::now() + Duration::hours(1);
        let now = Utc::now();
        let session = make_session(created, expires);

        let new_expires = session
            .compute_refreshed_expiry(now)
            .expect("should extend, but capped");
        let cap = created + Duration::days(SESSION_ABSOLUTE_MAX_DAYS);
        assert_eq!(new_expires, cap);
    }

    #[test]
    fn compute_refreshed_expiry_none_at_absolute_max() {
        let created = Utc::now() - Duration::days(SESSION_ABSOLUTE_MAX_DAYS);
        let expires = created + Duration::days(SESSION_ABSOLUTE_MAX_DAYS);
        let now = Utc::now();
        let session = make_session(created, expires);
        assert!(session.compute_refreshed_expiry(now).is_none());
    }

    #[tokio::test]
    async fn refresh_if_needed_persists_new_expiry() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let past_created = Utc::now() - Duration::days(6);
        let near_expiry = Utc::now() + Duration::hours(12);
        db_execute!(
            &db,
            "UPDATE session SET created_at = $1, expires_at = $2 WHERE id = $3",
            past_created,
            near_expiry,
            session.id
        )
        .unwrap();

        let reloaded = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        let new_expires = refresh_if_needed(&db, &reloaded)
            .await
            .unwrap()
            .expect("should refresh");

        let after = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        let drift = (after.expires_at - new_expires).num_seconds().abs();
        assert!(drift <= 1, "persisted expiry diverged: {drift}s");
        assert!(after.expires_at > reloaded.expires_at);
    }

    #[tokio::test]
    async fn refresh_if_needed_noop_for_fresh_session() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        assert!(refresh_if_needed(&db, &session).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn touch_last_seen_updates_when_stale() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let stale = Utc::now() - Duration::minutes(5);
        db_execute!(
            &db,
            "UPDATE session SET last_seen_at = $1 WHERE id = $2",
            stale,
            session.id
        )
        .unwrap();

        let reloaded = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        touch_last_seen(&db, &reloaded).await.unwrap();

        let after = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert!(after.last_seen_at > reloaded.last_seen_at);
        let drift = (Utc::now() - after.last_seen_at).num_seconds().abs();
        assert!(drift <= 1, "last_seen_at not close to now: {drift}s");
    }

    #[tokio::test]
    async fn touch_last_seen_noop_when_fresh() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        touch_last_seen(&db, &session).await.unwrap();

        let after = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.last_seen_at, session.last_seen_at);
    }

    #[tokio::test]
    async fn delete_expired_removes_only_expired() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();

        let expired = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let past = Utc::now() - Duration::days(1);
        db_execute!(
            &db,
            "UPDATE session SET expires_at = $1 WHERE id = $2",
            past,
            expired.id
        )
        .unwrap();

        let fresh_a = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let fresh_b = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let deleted = delete_expired(&db).await.unwrap();
        assert_eq!(deleted, 1);

        assert!(
            find_by_token(&db, &expired.session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_by_token(&db, &fresh_a.session_token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_token(&db, &fresh_b.session_token)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn delete_expired_is_noop_when_nothing_expired() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let deleted = delete_expired(&db).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn delete_expired_boundary_is_inclusive() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // The predicate is `<=`, so a row whose `expires_at` is exactly `now`
        // must be deleted, not just rows strictly in the past.
        let now = Utc::now();
        db_execute!(
            &db,
            "UPDATE session SET expires_at = $1 WHERE id = $2",
            now,
            session.id
        )
        .unwrap();

        let deleted = delete_expired(&db).await.unwrap();
        assert_eq!(deleted, 1);
        assert!(
            find_by_token(&db, &session.session_token)
                .await
                .unwrap()
                .is_none()
        );
    }
}
