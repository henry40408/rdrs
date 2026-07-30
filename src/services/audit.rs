//! Structured audit events for the session lifecycle.
//!
//! OWASP's Session Management Cheat Sheet ("Logging Sessions Life Cycle") asks
//! for creation, renewal, and destruction of session IDs, privilege-level
//! changes within a session, and invalid-session activity to be logged with a
//! timestamp, source IP, and user agent — and for the session ID itself to be
//! stored in logs only as a salted hash, never in the clear. This module is
//! the single place that shape gets applied: every call site goes through
//! [`AUDIT_TARGET`] and the same field names, so an operator's `RUST_LOG`
//! filter or SIEM query written against one event works for all of them.
//!
//! Every event that carries a session or token identifier hashes it through
//! [`crate::secret::audit_id`] first — the raw token must never reach a log
//! line. `tracing` supplies the timestamp, so none of these functions add one
//! of their own.
//!
//! **Deliberately out of scope:** OWASP also mentions logging session
//! *usage*, but that is an access log, which rdrs does not have; recording
//! every authenticated request here would multiply I/O for a stream nobody
//! asked for. This module only fires on a lifecycle transition — created,
//! renewed, destroyed, or a masquerade privilege change — never on ordinary
//! use of an already-valid session.

use chrono::{DateTime, Utc};

use crate::secret::audit_id;

/// Tracing target for every event this module emits. An operator can isolate
/// just this stream with `RUST_LOG=rdrs::audit=info`, or ship it to a SIEM by
/// setting `RDRS_LOG_FORMAT=json`.
pub const AUDIT_TARGET: &str = "rdrs::audit";

/// A new session was established (password, passkey, or forward-auth login).
/// `method` is one of `"password"`, `"passkey"`, `"forward_auth"`.
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

/// A `GReader` `ClientLogin` request minted a new `api_token` row. Kept
/// distinct from [`session_created`]: `ClientLogin` never touches the
/// `session` table (see the module doc on `secret.rs` — an `api_token` is an
/// independently-revocable grant, not a web session), so labelling this a
/// `session.created` event would misreport which table actually changed.
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

/// A single session was deleted — logout, or a lazy expiry-driven cleanup.
/// `reason` is `"logout"` or `"expired"`.
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

/// Every session belonging to a user was deleted in one operation — a
/// password change, "sign out other sessions", or an admin disabling the
/// account. `count` is `None` when the model layer does not report how many
/// rows were affected; the audit line still records that a bulk revocation
/// happened, just not its exact size.
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

/// One or all of a user's `GReader` API tokens were revoked. Kept distinct from
/// [`sessions_destroyed_bulk`] for the same reason [`api_token_created`] is
/// kept distinct from [`session_created`]: an `api_token` row is not a
/// `session` row.
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

/// An admin started masquerading as another user — a privilege-level change
/// within the existing session. OWASP's "privilege level changes within the
/// session" bullet, and the least skippable event in this module: before it,
/// an admin acting as another user left no trace at all.
///
/// Entering a masquerade rotates the session token (see
/// [`crate::models::session::start_masquerade`]), so this event names both
/// sides of the swap: `sid` is the session as it was known up to this point,
/// `new_sid` the identifier every later event on the same session will carry.
/// Without the pair, rotation would silently break the log's session
/// correlation exactly where an auditor most needs it to hold.
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

/// An admin stopped masquerading, restoring their own identity on the
/// session. `actor_user_id` and `restored_user_id` are the same admin — the
/// event still names both explicitly, matching `masquerade_started`'s shape.
/// `sid` / `new_sid` bracket the token rotation, as in [`masquerade_started`].
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

/// A login attempt failed. `username_len` is deliberately the *length* of the
/// attempted username, never the username itself: a very common user error is
/// typing a password into the username field, and accepting only a `usize`
/// here makes writing a password to the log impossible at the type level.
/// `reason` is one of `"unknown_user"`, `"bad_password"`, `"disabled"`.
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

/// A credential-endpoint request was rejected by the per-IP rate limiter
/// before any credential check ran. `bucket` names which budget
/// (`middleware::Bucket`) was exhausted.
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

#[cfg(test)]
mod tests {
    use super::*;

    // This repo has no `tracing-test` (and must not gain one), so log output
    // itself is not asserted here. What is checkable without it: every
    // emitter compiles and accepts the documented argument shapes, and the
    // reason/method strings each call site is expected to pass are pinned as
    // plain values here so a rename at a call site shows up as a diff in this
    // file too. The real guarantee this module offers is the type signature
    // of `login_failed` — see its doc comment.

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
        login_rate_limited("POST /api/session", "login", "127.0.0.1");
    }

    #[test]
    fn login_failed_reason_strings_match_call_sites() {
        // Pinned so a call site renaming one of these three strings shows up
        // as a diff here too, rather than silently drifting apart.
        const REASONS: [&str; 3] = ["unknown_user", "bad_password", "disabled"];
        for reason in REASONS {
            login_failed(0, reason, "127.0.0.1", "test-agent");
        }
    }
}
