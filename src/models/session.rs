use chrono::{DateTime, Duration, Utc};
use rand::RngExt;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt};

pub const SESSION_EXPIRY_DAYS: i64 = 7;
pub const SESSION_ABSOLUTE_MAX_DAYS: i64 = 90;
const TOKEN_LENGTH: usize = 32;

/// How long a rotated-out token keeps authenticating, so in-flight requests
/// aren't signed out; far below the rotation interval, so at most one predecessor is live.
pub const ROTATION_GRACE_SECONDS: i64 = 60;

/// How long after proving its credentials a session may perform a sensitive
/// operation without re-authenticating.
pub const REAUTH_WINDOW_MINUTES: i64 = 5;

/// `session` column list, a macro so `concat!` can splice it into the query
/// literals and keep them in sync with [`Session`].
macro_rules! session_columns {
    () => {
        "id, user_id, session_token, original_user_id, created_at, expires_at, \
         user_agent, ip_address, last_seen_at, previous_token, previous_token_expires_at, \
         last_authenticated_at"
    };
}

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
    /// Pre-rotation token, accepted by [`find_by_token`] until `previous_token_expires_at`.
    pub previous_token: Option<String>,
    pub previous_token_expires_at: Option<DateTime<Utc>>,
    /// When credentials were last proved (login or re-auth); `None` only for pre-backfill rows.
    pub last_authenticated_at: Option<DateTime<Utc>>,
}

impl Session {
    pub fn is_masquerading(&self) -> bool {
        self.original_user_id.is_some()
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Whether credentials were proved within [`REAUTH_WINDOW_MINUTES`]. A
    /// missing timestamp counts as stale (the safe direction).
    pub fn authenticated_recently(&self, now: DateTime<Utc>) -> bool {
        self.last_authenticated_at
            .is_some_and(|at| now - at < Duration::minutes(REAUTH_WINDOW_MINUTES))
    }

    /// `Some(new_expires_at)` when remaining TTL has fallen below half of
    /// `SESSION_EXPIRY_DAYS` and the session has not reached its absolute cap
    /// (`created_at + SESSION_ABSOLUTE_MAX_DAYS`).
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
/// mint a cookie (and derive a CSRF token) without a database row.
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
        concat!(
            "INSERT INTO session \
                 (user_id, session_token, expires_at, user_agent, ip_address, last_seen_at, \
                  last_authenticated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING ",
            session_columns!()
        ),
        user_id,
        &token,
        expires_at,
        user_agent,
        ip_address,
        now,
        // A login just succeeded, so start inside the re-auth window.
        now
    )
    .map_err(AppError::Database)
}

/// Restart the [`Session::authenticated_recently`] window.
pub async fn mark_authenticated(db: &Db, session_id: i64) -> AppResult<DateTime<Utc>> {
    let now = Utc::now();
    db_execute!(
        db,
        "UPDATE session SET last_authenticated_at = $1 WHERE id = $2",
        now,
        session_id
    )
    .map_err(AppError::Database)?;
    Ok(now)
}

/// Look up a session by `session_token`, or by a `previous_token` still in its
/// grace interval (both indexed). The grace arm lives here so every caller gets it.
pub async fn find_by_token(db: &Db, token: &str) -> AppResult<Option<Session>> {
    query_opt!(
        db,
        Session,
        concat!(
            "SELECT ",
            session_columns!(),
            " FROM session \
             WHERE session_token = $1 \
                OR (previous_token = $1 AND previous_token_expires_at > $2)"
        ),
        token,
        Utc::now()
    )
    .map_err(AppError::Database)
}

/// Bump `last_seen_at`, at most once per minute per session. Best-effort.
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

/// Slide `expires_at` forward if within the refresh window; `None` if no update
/// was needed. A `Some` is also the cue to call [`rotate_token`].
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

/// Rotate the token of the session answering to `token` (OWASP renewal
/// timeout); `None` if another request rotated it first.
///
/// Matches `session_token` only, never the grace token, so concurrent requests
/// cannot chain rotations.
pub async fn rotate_token(db: &Db, token: &str) -> AppResult<Option<String>> {
    let new_token = generate_token();
    let grace_until = Utc::now() + Duration::seconds(ROTATION_GRACE_SECONDS);
    let affected = db_execute!(
        db,
        "UPDATE session \
         SET session_token = $1, previous_token = session_token, \
             previous_token_expires_at = $2 \
         WHERE session_token = $3",
        &new_token,
        grace_until,
        token
    )
    .map_err(AppError::Database)?;

    Ok((affected > 0).then_some(new_token))
}

/// Delete the session for `token`. Also matches `previous_token`, since a
/// logout can arrive on the grace token.
pub async fn delete_session(db: &Db, token: &str) -> AppResult<()> {
    db_execute!(
        db,
        "DELETE FROM session WHERE session_token = $1 OR previous_token = $1",
        token
    )
    .map_err(AppError::Database)?;
    Ok(())
}

pub async fn delete_user_sessions(db: &Db, user_id: i64) -> AppResult<()> {
    db_execute!(db, "DELETE FROM session WHERE user_id = $1", user_id)
        .map_err(AppError::Database)?;
    Ok(())
}

/// All sessions of `user_id`, newest first, including expired rows.
pub async fn list_user_sessions(db: &Db, user_id: i64) -> AppResult<Vec<Session>> {
    query_all!(
        db,
        Session,
        concat!(
            "SELECT ",
            session_columns!(),
            " FROM session WHERE user_id = $1 ORDER BY created_at DESC"
        ),
        user_id
    )
    .map_err(AppError::Database)
}

/// Delete every session of `user_id` except `keep_token`'s, returning the count.
/// `keep_token` may be a grace token, so `previous_token` is exempted too.
pub async fn delete_user_sessions_except(
    db: &Db,
    user_id: i64,
    keep_token: &str,
) -> AppResult<u64> {
    db_execute!(
        db,
        "DELETE FROM session \
         WHERE user_id = $1 \
           AND session_token <> $2 \
           AND (previous_token IS NULL OR previous_token <> $2)",
        user_id,
        keep_token
    )
    .map_err(AppError::Database)
}

/// Delete one session by id, scoped to `user_id` so a guessed id can't revoke
/// another user's session. Returns rows deleted.
pub async fn delete_user_session_by_id(db: &Db, id: i64, user_id: i64) -> AppResult<u64> {
    db_execute!(
        db,
        "DELETE FROM session WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)
}

/// Delete expired sessions; the cleanup worker's backstop for abandoned rows the
/// lazy deletes never touch. Uses `idx_session_expires_at`; bound `now` keeps it
/// dialect-neutral.
pub async fn delete_expired(db: &Db) -> AppResult<u64> {
    let now = Utc::now();
    db_execute!(db, "DELETE FROM session WHERE expires_at <= $1", now).map_err(AppError::Database)
}

/// Start masquerading as `target_user_id`, returning the session's **new** token.
///
/// The token rotates in the same `UPDATE` as `user_id` (privilege change, done
/// atomically). Callers must reissue the session and CSRF cookies from it.
pub async fn start_masquerade(db: &Db, token: &str, target_user_id: i64) -> AppResult<String> {
    let session = find_by_token(db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if session.is_masquerading() {
        return Err(AppError::AlreadyMasquerading);
    }

    let new_token = generate_token();
    db_execute!(
        db,
        "UPDATE session \
         SET original_user_id = user_id, user_id = $1, session_token = $2, \
             previous_token = session_token, previous_token_expires_at = $3 \
         WHERE id = $4",
        target_user_id,
        &new_token,
        Utc::now() + Duration::seconds(ROTATION_GRACE_SECONDS),
        session.id
    )
    .map_err(AppError::Database)?;

    Ok(new_token)
}

/// Stop masquerading and return the session's **new** token; the token used
/// while impersonating must not survive as the admin's.
pub async fn stop_masquerade(db: &Db, token: &str) -> AppResult<String> {
    let session = find_by_token(db, token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    let new_token = generate_token();
    db_execute!(
        db,
        "UPDATE session \
         SET user_id = original_user_id, original_user_id = NULL, session_token = $1, \
             previous_token = session_token, previous_token_expires_at = $2 \
         WHERE id = $3",
        &new_token,
        Utc::now() + Duration::seconds(ROTATION_GRACE_SECONDS),
        session.id
    )
    .map_err(AppError::Database)?;

    Ok(new_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;
    use crate::test_support::{seed_user, setup_db};

    #[tokio::test]
    async fn test_create_and_find_session() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;

        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert_eq!(session.user_id, user.id);
        assert!(!session.is_masquerading());
        assert!(!session.is_expired());
        assert_eq!(session.user_agent, "test-agent");
        assert_eq!(session.ip_address, "127.0.0.1");
        // Both derive from the same `now`.
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
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user_a = seed_user(&db, "usera", Role::User).await;
        let user_b = seed_user(&db, "userb", Role::User).await;

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
        let user_a = seed_user(&db, "usera", Role::User).await;
        let user_b = seed_user(&db, "userb", Role::User).await;

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
    async fn delete_user_session_by_id_is_user_scoped() {
        let db = setup_db().await;
        let user_a = seed_user(&db, "usera", Role::User).await;
        let user_b = seed_user(&db, "userb", Role::User).await;

        let a_session = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let a_other = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // User B aims at user A's session id — must delete nothing.
        let deleted = delete_user_session_by_id(&db, a_session.id, user_b.id)
            .await
            .unwrap();
        assert_eq!(deleted, 0, "cross-user revoke must be a no-op");
        assert!(
            find_by_token(&db, &a_session.session_token)
                .await
                .unwrap()
                .is_some(),
            "user A's session must survive user B's revoke attempt"
        );

        let deleted = delete_user_session_by_id(&db, a_session.id, user_a.id)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        assert!(
            find_by_token(&db, &a_session.session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_by_token(&db, &a_other.session_token)
                .await
                .unwrap()
                .is_some(),
            "revoking one session must leave the user's other sessions alone"
        );

        // A second revoke reports zero rows.
        let deleted = delete_user_session_by_id(&db, a_session.id, user_a.id)
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_masquerade() {
        let db = setup_db().await;
        let admin = seed_user(&db, "admin", Role::Admin).await;
        let target = seed_user(&db, "target", Role::User).await;

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert!(!session.is_masquerading());

        let masq_token = start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        // The token rotates; the old one stays valid for the grace interval but
        // resolves to the same row with the new identity.
        assert_ne!(masq_token, session.session_token);
        let via_grace = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .expect("pre-rotation token stays valid for the grace interval");
        assert_eq!(via_grace.id, session.id);
        assert_eq!(via_grace.user_id, target.id);

        let masq = find_by_token(&db, &masq_token).await.unwrap().unwrap();
        assert_eq!(masq.id, session.id);
        assert!(masq.is_masquerading());
        assert_eq!(masq.user_id, target.id);
        assert_eq!(masq.original_user_id, Some(admin.id));
        assert_eq!(
            masq.previous_token.as_deref(),
            Some(&*session.session_token)
        );

        let restored_token = stop_masquerade(&db, &masq_token).await.unwrap();
        assert_ne!(restored_token, masq_token);
        assert_ne!(restored_token, session.session_token);
        // The second rotation evicts the pre-masquerade grace token.
        assert!(
            find_by_token(&db, &session.session_token)
                .await
                .unwrap()
                .is_none()
        );

        let restored = find_by_token(&db, &restored_token).await.unwrap().unwrap();
        assert_eq!(restored.id, session.id);
        assert!(!restored.is_masquerading());
        assert_eq!(restored.user_id, admin.id);
    }

    #[tokio::test]
    async fn test_masquerade_rotation_preserves_session_lifetime() {
        // Rotation must preserve `created_at`/`expires_at`, or the absolute cap resets.
        let db = setup_db().await;
        let admin = seed_user(&db, "admin", Role::Admin).await;
        let target = seed_user(&db, "target", Role::User).await;

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let masq_token = start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        let masq = find_by_token(&db, &masq_token).await.unwrap().unwrap();
        assert_eq!(masq.created_at, session.created_at);
        assert_eq!(masq.expires_at, session.expires_at);
    }

    #[tokio::test]
    async fn test_already_masquerading() {
        let db = setup_db().await;
        let admin = seed_user(&db, "admin", Role::Admin).await;
        let target = seed_user(&db, "target", Role::User).await;

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let masq_token = start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        // Use the rotated token so the guard, not a lookup miss, rejects it.
        let result = start_masquerade(&db, &masq_token, target.id).await;
        assert!(matches!(result, Err(AppError::AlreadyMasquerading)));
    }

    #[tokio::test]
    async fn test_not_masquerading() {
        let db = setup_db().await;
        let user = seed_user(&db, "user", Role::User).await;

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
            previous_token: None,
            previous_token_expires_at: None,
            last_authenticated_at: Some(created_at),
        }
    }

    #[test]
    fn authenticated_recently_inside_and_outside_the_window() {
        let now = Utc::now();
        let mut session = make_session(now, now + Duration::days(SESSION_EXPIRY_DAYS));

        session.last_authenticated_at = Some(now);
        assert!(session.authenticated_recently(now));

        session.last_authenticated_at =
            Some(now - Duration::minutes(REAUTH_WINDOW_MINUTES) + Duration::seconds(1));
        assert!(session.authenticated_recently(now));

        session.last_authenticated_at = Some(now - Duration::minutes(REAUTH_WINDOW_MINUTES));
        assert!(!session.authenticated_recently(now));
    }

    #[test]
    fn authenticated_recently_treats_a_missing_timestamp_as_stale() {
        let now = Utc::now();
        let mut session = make_session(now, now + Duration::days(SESSION_EXPIRY_DAYS));
        session.last_authenticated_at = None;
        assert!(!session.authenticated_recently(now));
    }

    #[tokio::test]
    async fn mark_authenticated_reopens_the_window() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        db_execute!(
            &db,
            "UPDATE session SET last_authenticated_at = $1 WHERE id = $2",
            Utc::now() - Duration::hours(1),
            session.id
        )
        .unwrap();
        let stale = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert!(!stale.authenticated_recently(Utc::now()));

        mark_authenticated(&db, session.id).await.unwrap();

        let fresh = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        assert!(fresh.authenticated_recently(Utc::now()));
    }

    #[tokio::test]
    async fn create_session_starts_inside_the_reauth_window() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert!(session.authenticated_recently(Utc::now()));
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
        let user = seed_user(&db, "testuser", Role::User).await;
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
    async fn rotate_token_replaces_token_and_keeps_the_old_one_in_grace() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let new_token = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .expect("the session matched, so it rotated");
        assert_ne!(new_token, session.session_token);

        let by_new = find_by_token(&db, &new_token).await.unwrap().unwrap();
        assert_eq!(by_new.id, session.id);
        assert_eq!(
            by_new.previous_token.as_deref(),
            Some(&*session.session_token)
        );

        // Both names reach the same row while the grace interval holds.
        let by_old = find_by_token(&db, &session.session_token)
            .await
            .unwrap()
            .expect("the replaced token still authenticates");
        assert_eq!(by_old.id, session.id);
    }

    #[tokio::test]
    async fn rotate_token_grace_lapses() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let new_token = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();

        // Expire the grace window instead of sleeping.
        db_execute!(
            &db,
            "UPDATE session SET previous_token_expires_at = $1 WHERE id = $2",
            Utc::now() - Duration::seconds(1),
            session.id
        )
        .unwrap();

        assert!(
            find_by_token(&db, &session.session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(find_by_token(&db, &new_token).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn rotate_token_is_none_when_another_request_already_rotated() {
        // Two concurrent rotations: the second must not chain and evict the grace token.
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let first = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        let second = rotate_token(&db, &session.session_token).await.unwrap();
        assert!(second.is_none());

        // The loser's token survives as the grace token.
        assert!(
            find_by_token(&db, &session.session_token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(find_by_token(&db, &first).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_session_accepts_the_grace_token() {
        // A logout on the pre-rotation cookie must still end the session.
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let new_token = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();

        delete_session(&db, &session.session_token).await.unwrap();

        assert!(find_by_token(&db, &new_token).await.unwrap().is_none());
        assert!(
            find_by_token(&db, &session.session_token)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_user_sessions_except_keeps_a_session_named_by_its_grace_token() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let keep = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let other = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let rotated = rotate_token(&db, &keep.session_token)
            .await
            .unwrap()
            .unwrap();

        // The caller still holds the pre-rotation token.
        delete_user_sessions_except(&db, user.id, &keep.session_token)
            .await
            .unwrap();

        assert!(find_by_token(&db, &rotated).await.unwrap().is_some());
        assert!(
            find_by_token(&db, &other.session_token)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn refresh_if_needed_noop_for_fresh_session() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        assert!(refresh_if_needed(&db, &session).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn touch_last_seen_updates_when_stale() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
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
        let user = seed_user(&db, "testuser", Role::User).await;
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
        let user = seed_user(&db, "testuser", Role::User).await;

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
        let user = seed_user(&db, "testuser", Role::User).await;
        create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let deleted = delete_expired(&db).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn delete_expired_boundary_is_inclusive() {
        let db = setup_db().await;
        let user = seed_user(&db, "testuser", Role::User).await;
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // The predicate is `<=`, so `expires_at == now` is deleted.
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
