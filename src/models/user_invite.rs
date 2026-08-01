//! One-time links that let a new account set its own password.
//!
//! rdrs has no self-service registration and no email. An admin creates the
//! account (username and role), and the person it belongs to receives a link
//! that is the only way to give it a password. That shape is what closes the
//! account-enumeration hole a public sign-up form always opens: there is no
//! anonymous endpoint that takes a username, so there is nothing to ask.
//!
//! The same table backs an admin-issued password *reset* for an account that
//! already has one. The two flows are identical except that the target's
//! current password keeps working until the link is redeemed — issuing a reset
//! must not lock someone out on its own.

use chrono::{DateTime, Duration, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secret::{DOMAIN_INVITE, tag};
use crate::{db_execute, query_one, query_opt};

/// How long a freshly issued link stays usable.
///
/// Long enough to survive a weekend and a time zone, short enough that a link
/// forgotten in a chat log or a proxy access log stops being a credential
/// fairly soon. Redemption is single-use as well, so this bounds only the
/// window in which an *unused* link is worth stealing.
pub const INVITE_TTL_DAYS: i64 = 7;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserInvite {
    pub id: i64,
    pub user_id: i64,
    /// HMAC of the token under [`DOMAIN_INVITE`] — never the token itself.
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// When the link was spent. `None` while it is still live.
    pub consumed_at: Option<DateTime<Utc>>,
}

impl UserInvite {
    /// Whether this row can still be redeemed *right now*.
    ///
    /// Both halves matter and both are re-checked at redemption time rather
    /// than trusted from the lookup: a link can expire between the page load
    /// that rendered the form and the submission that spends it.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.consumed_at.is_none() && now < self.expires_at
    }
}

/// Derive the stored form of a token.
///
/// Kept here rather than in the handler so there is exactly one definition of
/// "the same token": issuing hashes it to store it, redemption hashes the URL
/// segment to find it, and neither can drift from the other.
#[must_use]
pub fn hash_token(secret: &[u8], token: &str) -> String {
    hex::encode(tag(secret, DOMAIN_INVITE, &[token.as_bytes()]))
}

/// Issue a link for `user_id`, replacing any outstanding one.
///
/// Returns the row; the caller keeps the raw token, which is the only copy
/// that will ever exist — nothing here can reproduce it. Re-issuing revokes
/// the previous link deliberately: two live links for one account would mean
/// revoking the one you know about still leaves a way in.
pub async fn issue(db: &Db, secret: &[u8], user_id: i64, token: &str) -> AppResult<UserInvite> {
    revoke_for_user(db, user_id).await?;

    let now = Utc::now();
    let expires_at = now + Duration::days(INVITE_TTL_DAYS);
    let token_hash = hash_token(secret, token);

    query_one!(
        db,
        UserInvite,
        "INSERT INTO user_invite (user_id, token_hash, created_at, expires_at) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, user_id, token_hash, created_at, expires_at, consumed_at",
        user_id,
        token_hash,
        now,
        expires_at
    )
    .map_err(AppError::Database)
}

/// Look a token up by its hashed form.
///
/// Returns the row whether or not it is still live — the caller decides, and
/// must answer identically in every failing case (unknown, expired, already
/// spent) so the endpoint cannot be used to tell them apart.
pub async fn find_by_token(db: &Db, secret: &[u8], token: &str) -> AppResult<Option<UserInvite>> {
    let token_hash = hash_token(secret, token);

    query_opt!(
        db,
        UserInvite,
        "SELECT id, user_id, token_hash, created_at, expires_at, consumed_at \
         FROM user_invite WHERE token_hash = $1",
        token_hash
    )
    .map_err(AppError::Database)
}

/// Spend the invite, returning whether this caller is the one that spent it.
///
/// The `consumed_at IS NULL` predicate is the whole point: two submissions
/// racing on the same link both pass an earlier `is_live` check, and without
/// it both would go on to set a password — the later one silently overwriting
/// the password the first person just chose. Exactly one caller sees a row
/// count of 1; every other caller must treat `false` as "this link is no
/// longer valid" and change nothing.
pub async fn consume(db: &Db, invite_id: i64) -> AppResult<bool> {
    let now = Utc::now();
    let updated = db_execute!(
        db,
        "UPDATE user_invite SET consumed_at = $1 WHERE id = $2 AND consumed_at IS NULL",
        now,
        invite_id
    )
    .map_err(AppError::Database)?;

    Ok(updated == 1)
}

/// Drop any outstanding link for `user_id`.
///
/// Deletes rather than marking consumed: a revoked link never happened, and
/// keeping spent rows around would only grow the table and blur "was this
/// used?" into "was this used or cancelled?". The audit log records both.
pub async fn revoke_for_user(db: &Db, user_id: i64) -> AppResult<u64> {
    db_execute!(db, "DELETE FROM user_invite WHERE user_id = $1", user_id)
        .map_err(AppError::Database)
}

/// The live invite for `user_id`, if any — what `/admin` shows as "pending".
pub async fn find_live_for_user(db: &Db, user_id: i64) -> AppResult<Option<UserInvite>> {
    let invite = query_opt!(
        db,
        UserInvite,
        "SELECT id, user_id, token_hash, created_at, expires_at, consumed_at \
         FROM user_invite WHERE user_id = $1",
        user_id
    )
    .map_err(AppError::Database)?;

    Ok(invite.filter(|i| i.is_live(Utc::now())))
}

/// Delete invites that are spent or long past their expiry.
///
/// Housekeeping only — an expired row is already refused by [`UserInvite::is_live`],
/// so this exists to keep the table from accumulating dead links, not to
/// enforce anything.
pub async fn delete_stale(db: &Db) -> AppResult<u64> {
    let cutoff = Utc::now();
    db_execute!(
        db,
        "DELETE FROM user_invite WHERE consumed_at IS NOT NULL OR expires_at <= $1",
        cutoff
    )
    .map_err(AppError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::{self, Role};

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    async fn setup() -> (Db, i64) {
        let db = Db::connect_in_memory().await.unwrap();
        let user = user::create_user(&db, "invitee", "!", Role::User)
            .await
            .unwrap();
        (db, user.id)
    }

    #[tokio::test]
    async fn an_issued_invite_is_found_by_its_raw_token() {
        let (db, user_id) = setup().await;

        let issued = issue(&db, SECRET, user_id, "raw-token-value")
            .await
            .unwrap();
        assert!(issued.is_live(Utc::now()));

        let found = find_by_token(&db, SECRET, "raw-token-value")
            .await
            .unwrap()
            .expect("the token just issued must resolve");
        assert_eq!(found.id, issued.id);
    }

    #[tokio::test]
    async fn the_raw_token_is_never_stored() {
        // A database copy must not be enough to mint a working link.
        let (db, user_id) = setup().await;
        let issued = issue(&db, SECRET, user_id, "raw-token-value")
            .await
            .unwrap();

        assert_ne!(issued.token_hash, "raw-token-value");
        assert!(!issued.token_hash.contains("raw-token-value"));

        // ...and the hash is keyed, so another deployment's secret does not
        // resolve the same token.
        assert!(
            find_by_token(&db, b"a-different-secret-value-32bytes", "raw-token-value")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn issuing_again_revokes_the_previous_link() {
        // Two live links for one account would mean revoking the one you know
        // about still leaves a way in.
        let (db, user_id) = setup().await;

        issue(&db, SECRET, user_id, "first").await.unwrap();
        issue(&db, SECRET, user_id, "second").await.unwrap();

        assert!(find_by_token(&db, SECRET, "first").await.unwrap().is_none());
        assert!(
            find_by_token(&db, SECRET, "second")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_invite_can_only_be_consumed_once() {
        let (db, user_id) = setup().await;
        let issued = issue(&db, SECRET, user_id, "single-use").await.unwrap();

        assert!(consume(&db, issued.id).await.unwrap());
        assert!(
            !consume(&db, issued.id).await.unwrap(),
            "a second redemption must not be allowed to change anything"
        );

        let after = find_by_token(&db, SECRET, "single-use")
            .await
            .unwrap()
            .unwrap();
        assert!(!after.is_live(Utc::now()));
    }

    #[tokio::test]
    async fn an_expired_invite_is_not_live() {
        let (db, user_id) = setup().await;
        let issued = issue(&db, SECRET, user_id, "aging").await.unwrap();

        assert!(issued.is_live(issued.expires_at - Duration::seconds(1)));
        assert!(!issued.is_live(issued.expires_at));
        assert!(!issued.is_live(issued.expires_at + Duration::days(1)));
    }

    #[tokio::test]
    async fn only_a_live_invite_is_reported_as_pending() {
        let (db, user_id) = setup().await;
        issue(&db, SECRET, user_id, "pending-link").await.unwrap();
        assert!(find_live_for_user(&db, user_id).await.unwrap().is_some());

        let issued = find_by_token(&db, SECRET, "pending-link")
            .await
            .unwrap()
            .unwrap();
        consume(&db, issued.id).await.unwrap();
        assert!(find_live_for_user(&db, user_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoking_removes_the_link() {
        let (db, user_id) = setup().await;
        issue(&db, SECRET, user_id, "doomed").await.unwrap();

        assert_eq!(revoke_for_user(&db, user_id).await.unwrap(), 1);
        assert!(
            find_by_token(&db, SECRET, "doomed")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deleting_the_account_takes_its_invite_with_it() {
        // ON DELETE CASCADE: a deleted account must not leave a live link
        // behind that would resurrect nothing but still resolve.
        let (db, user_id) = setup().await;
        issue(&db, SECRET, user_id, "orphan").await.unwrap();

        user::delete_user(&db, user_id).await.unwrap();

        assert!(
            find_by_token(&db, SECRET, "orphan")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn stale_rows_are_cleaned_up() {
        let (db, user_id) = setup().await;
        let issued = issue(&db, SECRET, user_id, "spent").await.unwrap();
        consume(&db, issued.id).await.unwrap();

        assert_eq!(delete_stale(&db).await.unwrap(), 1);
        assert!(find_by_token(&db, SECRET, "spent").await.unwrap().is_none());
    }
}
