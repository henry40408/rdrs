//! Structured audit events for the session and credential lifecycle (OWASP
//! "Logging Sessions Life Cycle"), all under [`AUDIT_TARGET`] with shared
//! field names.
//!
//! Identifiers are hashed via [`crate::secret::audit_id`]; a raw token must
//! never reach a log line. Session *usage* is out of scope (no access log).

use chrono::{DateTime, Utc};

use crate::secret::audit_id;

/// Tracing target for every event here (`RUST_LOG=rdrs::audit=info`).
pub const AUDIT_TARGET: &str = "rdrs::audit";

/// A new session was established. `method`: `"password"`, `"passkey"` or
/// `"forward_auth"`.
pub fn session_created(secret: &[u8], token: &str, user_id: i64, method: &str, ip: &str, ua: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "session.created",
        sid = %audit_id(secret, token),
        user_id,
        method,
        ip = %ip,
        user_agent = %ua,
        "session created"
    );
}

/// `GReader` `ClientLogin` minted an `api_token` row (not a `session`, hence
/// distinct from [`session_created`]).
pub fn api_token_created(
    secret: &[u8],
    token: &str,
    user_id: i64,
    method: &str,
    ip: &str,
    ua: &str,
) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "api_token.created",
        sid = %audit_id(secret, token),
        user_id,
        method,
        ip = %ip,
        user_agent = %ua,
        "API token created"
    );
}

/// An existing session's sliding expiry was extended.
pub fn session_renewed(secret: &[u8], token: &str, user_id: i64, new_expires_at: DateTime<Utc>) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "session.renewed",
        sid = %audit_id(secret, token),
        user_id,
        new_expires_at = %new_expires_at,
        "session renewed"
    );
}

/// A session token was rotated (OWASP "Renewal Timeout"); `sid`/`new_sid`
/// keep the session correlatable across the swap.
pub fn session_token_rotated(secret: &[u8], token: &str, new_token: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "session.token_rotated",
        sid = %audit_id(secret, token),
        new_sid = %audit_id(secret, new_token),
        "session token rotated"
    );
}

/// A session was deleted. `reason`: `"logout"` or `"expired"`.
pub fn session_destroyed(secret: &[u8], token: &str, user_id: i64, reason: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "session.destroyed",
        sid = %audit_id(secret, token),
        user_id,
        reason,
        "session destroyed"
    );
}

/// All of a user's sessions were deleted (password change, sign-out-others,
/// account disabled). `count` is `None` when the model does not report it.
pub fn sessions_destroyed_bulk(user_id: i64, reason: &str, count: Option<u64>) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "sessions.destroyed",
        user_id,
        reason,
        count = ?count,
        "sessions destroyed (bulk)"
    );
}

/// One or all `GReader` API tokens were revoked (distinct from
/// [`sessions_destroyed_bulk`]: not `session` rows).
pub fn api_tokens_destroyed(user_id: i64, reason: &str, count: Option<u64>) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "api_tokens.destroyed",
        user_id,
        reason,
        count = ?count,
        "API tokens destroyed"
    );
}

/// An admin started masquerading as another user (a privilege change).
/// `sid`/`new_sid` bracket the token rotation so correlation survives it.
pub fn masquerade_started(
    secret: &[u8],
    token: &str,
    new_token: &str,
    actor_user_id: i64,
    target_user_id: i64,
    ip: &str,
    ua: &str,
) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "masquerade.started",
        sid = %audit_id(secret, token),
        new_sid = %audit_id(secret, new_token),
        actor_user_id,
        target_user_id,
        ip = %ip,
        user_agent = %ua,
        "masquerade started"
    );
}

/// An admin stopped masquerading. Both user ids name the same admin, matching
/// [`masquerade_started`]'s shape; `sid`/`new_sid` bracket the rotation.
pub fn masquerade_stopped(
    secret: &[u8],
    token: &str,
    new_token: &str,
    actor_user_id: i64,
    restored_user_id: i64,
) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "masquerade.stopped",
        sid = %audit_id(secret, token),
        new_sid = %audit_id(secret, new_token),
        actor_user_id,
        restored_user_id,
        "masquerade stopped"
    );
}

/// A passkey was registered: an independent credential that survives a
/// password change. Logged with request `ip`/`user_agent` for tracing.
pub fn passkey_registered(
    secret: &[u8],
    token: &str,
    user_id: i64,
    passkey_id: i64,
    name: &str,
    ip: &str,
    ua: &str,
) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "passkey.registered",
        sid = %audit_id(secret, token),
        user_id,
        passkey_id,
        passkey_name = %name,
        ip = %ip,
        user_agent = %ua,
        "passkey registered"
    );
}

/// A passkey was removed; the counterpart of [`passkey_registered`].
pub fn passkey_removed(secret: &[u8], token: &str, user_id: i64, passkey_id: i64) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "passkey.removed",
        sid = %audit_id(secret, token),
        user_id,
        passkey_id,
        "passkey removed"
    );
}

/// A session re-authenticated, refreshing `middleware::auth::RecentlyAuthenticated`.
/// `method`: `"password"` or `"forward_auth"`.
pub fn session_reauthenticated(secret: &[u8], token: &str, user_id: i64, method: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "session.reauthenticated",
        sid = %audit_id(secret, token),
        user_id,
        method,
        "session reauthenticated"
    );
}

/// A login attempt failed. Takes only `username_len` so a password typed into
/// the username field can never be logged. `reason`: `"unknown_user"`,
/// `"bad_password"` or `"disabled"`.
pub fn login_failed(username_len: usize, reason: &str, ip: &str, ua: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "login.failed",
        username_len,
        reason,
        ip = %ip,
        user_agent = %ua,
        "login failed"
    );
}

/// A credential request was rejected by the per-IP rate limiter; `bucket` is
/// the exhausted `middleware::Bucket`.
pub fn login_rate_limited(endpoint: &str, bucket: &str, ip: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "login.rate_limited",
        endpoint,
        bucket,
        ip = %ip,
        "login rate limited"
    );
}

/// An admin issued a one-time password-setting link. The token is logged only
/// as its salted `audit_id`; `reason` distinguishes new account from reset.
pub fn invite_issued(
    secret: &[u8],
    token: &str,
    user_id: i64,
    actor_user_id: i64,
    reason: &str,
    expires_at: DateTime<Utc>,
) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "invite.issued",
        iid = %audit_id(secret, token),
        user_id,
        actor_user_id,
        reason,
        expires_at = %expires_at.to_rfc3339(),
        "account invite issued"
    );
}

/// A link was redeemed and the account's password set; the counterpart of
/// [`invite_issued`].
pub fn invite_consumed(secret: &[u8], token: &str, user_id: i64, ip: &str, ua: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "invite.consumed",
        iid = %audit_id(secret, token),
        user_id,
        ip = %ip,
        user_agent = %ua,
        "account invite redeemed"
    );
}

/// An outstanding link was cancelled. Identified by account since the token
/// row is already deleted.
pub fn invite_revoked(user_id: i64, actor_user_id: i64) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "invite.revoked",
        user_id,
        actor_user_id,
        "account invite revoked"
    );
}

/// An admin created an account, unusable until its [`invite_issued`] link is
/// redeemed.
pub fn account_created(user_id: i64, actor_user_id: i64, username_len: usize, role: &str) {
    tracing::info!(
        target: AUDIT_TARGET,
        event = "account.created",
        user_id,
        actor_user_id,
        username_len,
        role,
        "account created by admin"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // No `tracing-test` (and must not gain one): these only check that emitters
    // accept their argument shapes and pin the reason/method strings.

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn emitters_accept_their_documented_argument_shapes() {
        session_created(SECRET, "tok", 1, "password", "127.0.0.1", "test-agent");
        session_created(SECRET, "tok", 1, "passkey", "127.0.0.1", "test-agent");
        session_created(SECRET, "tok", 1, "forward_auth", "127.0.0.1", "test-agent");
        api_token_created(SECRET, "tok", 1, "client_login", "127.0.0.1", "test-agent");
        session_renewed(SECRET, "tok", 1, Utc::now());
        session_destroyed(SECRET, "tok", 1, "logout");
        session_destroyed(SECRET, "tok", 1, "expired");
        sessions_destroyed_bulk(1, "password_change", None);
        sessions_destroyed_bulk(1, "revoke_others", Some(3));
        sessions_destroyed_bulk(1, "admin_disable", None);
        api_tokens_destroyed(1, "revoke_token", None);
        api_tokens_destroyed(1, "revoke_all", Some(2));
        masquerade_started(SECRET, "tok", "tok2", 1, 2, "127.0.0.1", "test-agent");
        masquerade_stopped(SECRET, "tok2", "tok3", 1, 1);
        session_token_rotated(SECRET, "tok3", "tok4");
        session_reauthenticated(SECRET, "tok4", 1, "password");
        session_reauthenticated(SECRET, "tok4", 1, "forward_auth");
        passkey_registered(SECRET, "tok4", 1, 7, "MacBook", "127.0.0.1", "test-agent");
        passkey_removed(SECRET, "tok4", 1, 7);
        login_rate_limited("POST /api/session", "login", "127.0.0.1");
        invite_issued(SECRET, "tok5", 2, 1, "account_created", Utc::now());
        invite_consumed(SECRET, "tok5", 2, "127.0.0.1", "test-agent");
        invite_revoked(2, 1);
        account_created(2, 1, 5, "user");
    }

    #[test]
    fn login_failed_reason_strings_match_call_sites() {
        // Pinned so a call-site rename shows up here.
        const REASONS: [&str; 3] = ["unknown_user", "bad_password", "disabled"];
        for reason in REASONS {
            login_failed(0, reason, "127.0.0.1", "test-agent");
        }
    }
}
