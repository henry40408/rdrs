use chrono::{DateTime, Duration, Utc};
use rand::RngExt;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_one, query_opt};

pub const SESSION_EXPIRY_DAYS: i64 = 7;
pub const SESSION_ABSOLUTE_MAX_DAYS: i64 = 90;
const TOKEN_LENGTH: usize = 32;

/// How long the token a rotation replaced keeps authenticating.
///
/// OWASP's "Renewal Timeout" calls for exactly this safety interval: requests
/// already in flight when a rotation lands still carry the old cookie, and
/// rejecting them would sign an active browser out at random. Sixty seconds
/// covers a slow page load plus its subresources while staying far below the
/// rotation interval, so a session never has more than one live predecessor.
pub const ROTATION_GRACE_SECONDS: i64 = 60;

/// How long after proving its credentials a session may perform a sensitive
/// operation without proving them again.
///
/// Five minutes is long enough that the common path — log in, go to settings,
/// add a passkey — is never interrupted, and short enough that a session picked
/// up later cannot quietly mint a new credential.
pub const REAUTH_WINDOW_MINUTES: i64 = 5;

/// The full column list of `session`, expanded at compile time into each
/// statement that needs it (via `concat!`) so the four of them cannot drift
/// apart from the struct below — the failure mode a plain `const` could not
/// prevent, since the query macros splice literals.
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
    /// The token this session answered to before its most recent rotation, or
    /// `None` for a session that has never rotated. Still accepted by
    /// [`find_by_token`] until `previous_token_expires_at`.
    pub previous_token: Option<String>,
    pub previous_token_expires_at: Option<DateTime<Utc>>,
    /// When this session last *proved* its credentials rather than merely
    /// presenting a cookie: set at login, refreshed by a re-authentication.
    /// `None` only for a row predating the column's backfill.
    pub last_authenticated_at: Option<DateTime<Utc>>,
}

impl Session {
    pub fn is_masquerading(&self) -> bool {
        self.original_user_id.is_some()
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Whether the session proved its credentials recently enough for a
    /// sensitive operation — OWASP's reauthentication-for-risk-events rule.
    ///
    /// A missing `last_authenticated_at` counts as stale rather than fresh: the
    /// only rows without one predate the column, and asking such a session to
    /// re-authenticate is the failure direction that cannot do harm.
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
        // A session exists because a login just succeeded, so it starts inside
        // the re-authentication window.
        now
    )
    .map_err(AppError::Database)
}

/// Record that this session has just re-proved its credentials, restarting the
/// window [`Session::authenticated_recently`] measures.
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

/// Look up a session by the token a client presented.
///
/// Matches the current `session_token` first and, failing that, a
/// `previous_token` whose grace interval has not lapsed. Both arms are indexed.
/// The grace arm keeps a rotation from signing out requests already in flight,
/// and it is deliberately part of the *lookup* rather than something each caller
/// has to remember, so every authenticated path inherits it.
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
/// `None` when no update was necessary — plenty of TTL left, or the absolute cap
/// of `created_at + SESSION_ABSOLUTE_MAX_DAYS` reached.
///
/// A `Some` is also the cue to rotate the session token; see [`rotate_token`],
/// which the cookie-writing layer calls once it knows the response can carry the
/// new one.
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

/// Rename the session currently answering to `token`, returning its new token —
/// or `None` when no session matched, meaning another request rotated it first.
///
/// This is OWASP's "Renewal Timeout": a token that would otherwise live for the
/// full 90-day absolute cap is replaced periodically, so a captured value stops
/// working long before the session ends. It is driven off the sliding-refresh
/// trigger, which already fires at most once per half-TTL, so a token lives
/// around 3.5 days with no extra column or timer to pace it.
///
/// The predicate matches `session_token` exactly, never the grace token, so
/// concurrent requests cannot chain rotations: the first renames the row and
/// every other gets `None` and keeps the token it has, which the grace interval
/// keeps valid.
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

/// Delete the session a client presented `token` for.
///
/// Matches `previous_token` as well, because a logout can arrive on the grace
/// token: the browser holds the pre-rotation cookie for up to
/// [`ROTATION_GRACE_SECONDS`], and a `session_token`-only predicate would delete
/// nothing and leave the user apparently signed in.
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

/// All sessions belonging to `user_id`, newest first. Includes expired rows;
/// the caller filters for display if needed.
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

/// Delete every session of `user_id` except the one whose token is `keep_token`,
/// for "sign out other sessions".
///
/// `keep_token` may be a grace token, so the exemption checks `previous_token`
/// too — otherwise the one session the caller meant to keep is the one this
/// deletes. Returns the number deleted, so the caller can report how many
/// devices it signed out and audit the revocation with its true size.
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

/// Delete one session by row id, scoped to `user_id` so a guessed id can never
/// revoke another user's session — the same guarantee as `api_token::delete_token`.
///
/// Returns the number of rows deleted so the caller can tell "revoked" from
/// "there was nothing to revoke". Unlike [`delete_session`] this deliberately
/// does **not** match `previous_token`: the id identifies the row directly, so
/// there is no grace-token ambiguity.
pub async fn delete_user_session_by_id(db: &Db, id: i64, user_id: i64) -> AppResult<u64> {
    db_execute!(
        db,
        "DELETE FROM session WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(AppError::Database)
}

/// Delete every expired session row.
///
/// Called periodically by the cleanup worker. This is the backstop for the lazy
/// deletes in the extractors, which only fire when a row is touched — a session
/// abandoned on a device the user never returns to would otherwise live forever.
/// Covered by `idx_session_expires_at`. The bound `now`, rather than SQL
/// `datetime('now')`, keeps this a plain `db_execute!` that behaves identically
/// on both dialects without the `pg_rewrite` shim.
pub async fn delete_expired(db: &Db) -> AppResult<u64> {
    let now = Utc::now();
    db_execute!(db, "DELETE FROM session WHERE expires_at <= $1", now).map_err(AppError::Database)
}

/// Start masquerading as `target_user_id`, returning the session's **new** token.
///
/// The token is rotated in the same `UPDATE` that changes `user_id`, because
/// entering a masquerade is a privilege-level change and OWASP requires the
/// session ID to be renewed across one. A single statement keeps the identity
/// and credential swaps atomic: there is no window in which the session already
/// acts as the target while still answering to the old token.
///
/// Callers must reissue the session cookie — and the CSRF cookie derived from it
/// — from the returned token, or the client is left holding credentials for a
/// row that no longer exists.
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

/// Stop masquerading, restoring the original user, and return the session's
/// **new** token.
///
/// Rotated for the same reason as [`start_masquerade`], and this is the
/// direction that matters: the token in use while acting as someone else —
/// potentially observed on the impersonated user's screen, in a support
/// recording, or in a debug log — must not survive as the restored admin's.
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
    async fn delete_user_session_by_id_is_user_scoped() {
        let db = setup_db().await;
        let user_a = user::create_user(&db, "usera", "hash", Role::User)
            .await
            .unwrap();
        let user_b = user::create_user(&db, "userb", "hash", Role::User)
            .await
            .unwrap();

        let a_session = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let a_other = create_session(&db, user_a.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // User B aims at user A's session id — must delete nothing. Getting the
        // scoping wrong here hands anyone who can guess an id the ability to
        // sign another user out.
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

        // Revoking the same id twice reports zero rows — the handler turns this
        // into "already gone" rather than a success message for a no-op.
        let deleted = delete_user_session_by_id(&db, a_session.id, user_a.id)
            .await
            .unwrap();
        assert_eq!(deleted, 0);
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

        let masq_token = start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        // The privilege change rotates the token. The old one stays usable for
        // the grace interval — an in-flight request must not be signed out —
        // but it now resolves to the *same* row, already carrying the new
        // identity, so it grants nothing the new token would not.
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
        // The second rotation replaces the grace token, so the token from
        // *before* the masquerade stops working now — only one predecessor is
        // ever live at a time.
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
        // Rotation replaces the credential, not the session: `created_at` and
        // `expires_at` must survive it, or entering a masquerade would silently
        // reset the absolute cap that `compute_refreshed_expiry` enforces.
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
        let admin = user::create_user(&db, "admin", "hash", Role::Admin)
            .await
            .unwrap();
        let target = user::create_user(&db, "target", "hash", Role::User)
            .await
            .unwrap();

        let session = create_session(&db, admin.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        let masq_token = start_masquerade(&db, &session.session_token, target.id)
            .await
            .unwrap();

        // Retried against the rotated token, so the rejection is the
        // already-masquerading guard and not a stale-token lookup miss.
        let result = start_masquerade(&db, &masq_token, target.id).await;
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
        // Rows predating the column's backfill must be asked to
        // re-authenticate, never waved through.
        let now = Utc::now();
        let mut session = make_session(now, now + Duration::days(SESSION_EXPIRY_DAYS));
        session.last_authenticated_at = None;
        assert!(!session.authenticated_recently(now));
    }

    #[tokio::test]
    async fn mark_authenticated_reopens_the_window() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
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
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
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
    async fn rotate_token_replaces_token_and_keeps_the_old_one_in_grace() {
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
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
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let new_token = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();

        // Push the grace deadline into the past rather than sleeping through
        // ROTATION_GRACE_SECONDS.
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
        // The concurrency case: two requests in flight, both told to rotate.
        // The second must find nothing to do rather than chaining a second
        // rotation and evicting the first one's grace token.
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
        let session = create_session(&db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        let first = rotate_token(&db, &session.session_token)
            .await
            .unwrap()
            .unwrap();
        let second = rotate_token(&db, &session.session_token).await.unwrap();
        assert!(second.is_none());

        // The loser keeps using the token it arrived with, which the winner
        // left behind as the grace token.
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
        // A logout can arrive on the pre-rotation cookie; it must still end the
        // session rather than silently deleting nothing.
        let db = setup_db().await;
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
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
        let user = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap();
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

        // The caller still holds the pre-rotation token, so that is what names
        // the session to spare.
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
