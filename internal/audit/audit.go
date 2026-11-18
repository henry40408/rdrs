// Package audit writes structured events for the session and credential
// lifecycle (OWASP "Logging Sessions Life Cycle"), all with target
// rdrs::audit and shared field names.
//
// Identifiers are hashed with secret.AuditID; a raw token must never reach a
// log line. Session usage is out of scope.
package audit

import (
	"context"
	"log/slog"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

// Target is the value of the "target" attribute on every event.
const Target = "rdrs::audit"

// Logger emits audit events. Its key is the secret that salts token hashes.
type Logger struct {
	log *slog.Logger
	key []byte
}

// New makes a Logger writing through log, hashing tokens with key.
func New(log *slog.Logger, key []byte) *Logger {
	return &Logger{log: log.With("target", Target), key: key}
}

// Request is the connection a credential event came from.
type Request struct {
	IP        string
	UserAgent string
}

// CredentialCreated describes a new session or API token.
type CredentialCreated struct {
	Token  string
	UserID int64
	// Method is "password", "passkey" or "forward_auth" for a session.
	Method string
	Request
}

func (l *Logger) emit(message string, attrs ...any) {
	l.log.Log(context.Background(), slog.LevelInfo, message, attrs...)
}

// SessionCreated records a new session.
func (l *Logger) SessionCreated(e CredentialCreated) {
	l.emit("session created", "event", "session.created",
		"sid", secret.AuditID(l.key, e.Token), "user_id", e.UserID, "method", e.Method,
		"ip", e.IP, "user_agent", e.UserAgent)
}

// APITokenCreated records a GReader ClientLogin token: an api_token row, not a
// session.
func (l *Logger) APITokenCreated(e CredentialCreated) {
	l.emit("API token created", "event", "api_token.created",
		"sid", secret.AuditID(l.key, e.Token), "user_id", e.UserID, "method", e.Method,
		"ip", e.IP, "user_agent", e.UserAgent)
}

// SessionRenewed records an extension of a session's sliding expiry.
func (l *Logger) SessionRenewed(token string, userID int64, newExpiresAt time.Time) {
	l.emit("session renewed", "event", "session.renewed",
		"sid", secret.AuditID(l.key, token), "user_id", userID, "new_expires_at", newExpiresAt.UTC().String())
}

// SessionTokenRotated records a token swap; sid and new_sid keep the session
// correlatable across it.
func (l *Logger) SessionTokenRotated(token, newToken string) {
	l.emit("session token rotated", "event", "session.token_rotated",
		"sid", secret.AuditID(l.key, token), "new_sid", secret.AuditID(l.key, newToken))
}

// SessionDestroyed records a deleted session. reason is "logout" or "expired".
func (l *Logger) SessionDestroyed(token string, userID int64, reason string) {
	l.emit("session destroyed", "event", "session.destroyed",
		"sid", secret.AuditID(l.key, token), "user_id", userID, "reason", reason)
}

// SessionsDestroyedBulk records the deletion of all of a user's sessions. count
// is nil when the model does not report it.
func (l *Logger) SessionsDestroyedBulk(userID int64, reason string, count *uint64) {
	l.emit("sessions destroyed (bulk)", "event", "sessions.destroyed",
		"user_id", userID, "reason", reason, "count", optionalCount(count))
}

// APITokensDestroyed records the revocation of one or all GReader tokens.
func (l *Logger) APITokensDestroyed(userID int64, reason string, count *uint64) {
	l.emit("API tokens destroyed", "event", "api_tokens.destroyed",
		"user_id", userID, "reason", reason, "count", optionalCount(count))
}

// optionalCount renders like Rust's Option Debug output: Some(3) or None.
func optionalCount(count *uint64) string {
	if count == nil {
		return "None"
	}
	return "Some(" + strconv.FormatUint(*count, 10) + ")"
}

// MasqueradeStarted describes an admin starting to act as another user.
type MasqueradeStarted struct {
	Token        string
	NewToken     string
	ActorUserID  int64
	TargetUserID int64
	Request
}

// MasqueradeStarted records a privilege change; sid and new_sid bracket the
// token rotation.
func (l *Logger) MasqueradeStarted(e MasqueradeStarted) {
	l.emit("masquerade started", "event", "masquerade.started",
		"sid", secret.AuditID(l.key, e.Token), "new_sid", secret.AuditID(l.key, e.NewToken),
		"actor_user_id", e.ActorUserID, "target_user_id", e.TargetUserID,
		"ip", e.IP, "user_agent", e.UserAgent)
}

// MasqueradeStopped describes an admin returning to their own account.
type MasqueradeStopped struct {
	Token          string
	NewToken       string
	ActorUserID    int64
	RestoredUserID int64
}

// MasqueradeStopped records the return; both user ids name the same admin.
func (l *Logger) MasqueradeStopped(e MasqueradeStopped) {
	l.emit("masquerade stopped", "event", "masquerade.stopped",
		"sid", secret.AuditID(l.key, e.Token), "new_sid", secret.AuditID(l.key, e.NewToken),
		"actor_user_id", e.ActorUserID, "restored_user_id", e.RestoredUserID)
}

// PasskeyRegistered describes a new passkey.
type PasskeyRegistered struct {
	Token     string
	UserID    int64
	PasskeyID int64
	Name      string
	Request
}

// PasskeyRegistered records an independent credential that survives a password
// change.
func (l *Logger) PasskeyRegistered(e PasskeyRegistered) {
	l.emit("passkey registered", "event", "passkey.registered",
		"sid", secret.AuditID(l.key, e.Token), "user_id", e.UserID, "passkey_id", e.PasskeyID,
		"passkey_name", e.Name, "ip", e.IP, "user_agent", e.UserAgent)
}

// PasskeyRemoved records the removal of a passkey.
func (l *Logger) PasskeyRemoved(token string, userID, passkeyID int64) {
	l.emit("passkey removed", "event", "passkey.removed",
		"sid", secret.AuditID(l.key, token), "user_id", userID, "passkey_id", passkeyID)
}

// SessionReauthenticated records a refresh of the recent-authentication mark.
// method is "password" or "forward_auth".
func (l *Logger) SessionReauthenticated(token string, userID int64, method string) {
	l.emit("session reauthenticated", "event", "session.reauthenticated",
		"sid", secret.AuditID(l.key, token), "user_id", userID, "method", method)
}

// LoginFailed records a failed login. It takes only the username length so a
// password typed into the username field can never be logged. reason is
// "unknown_user", "bad_password" or "disabled".
func (l *Logger) LoginFailed(usernameLen int, reason string, request Request) {
	l.emit("login failed", "event", "login.failed",
		"username_len", usernameLen, "reason", reason,
		"ip", request.IP, "user_agent", request.UserAgent)
}

// LoginRateLimited records a credential request refused by the per-IP limiter;
// bucket is the exhausted one.
func (l *Logger) LoginRateLimited(endpoint, bucket, ip string) {
	l.emit("login rate limited", "event", "login.rate_limited",
		"endpoint", endpoint, "bucket", bucket, "ip", ip)
}

// InviteIssued describes a one-time password-setting link.
type InviteIssued struct {
	Token       string
	UserID      int64
	ActorUserID int64
	// Reason tells a new account from a reset.
	Reason    string
	ExpiresAt time.Time
}

// InviteIssued records the link; the token appears only as its salted hash.
func (l *Logger) InviteIssued(e InviteIssued) {
	l.emit("account invite issued", "event", "invite.issued",
		"iid", secret.AuditID(l.key, e.Token), "user_id", e.UserID, "actor_user_id", e.ActorUserID,
		"reason", e.Reason, "expires_at", e.ExpiresAt.UTC().Format(time.RFC3339Nano))
}

// InviteConsumed describes a redeemed link.
type InviteConsumed struct {
	Token  string
	UserID int64
	Request
}

// InviteConsumed records the redemption that set the account's password.
func (l *Logger) InviteConsumed(e InviteConsumed) {
	l.emit("account invite redeemed", "event", "invite.consumed",
		"iid", secret.AuditID(l.key, e.Token), "user_id", e.UserID,
		"ip", e.IP, "user_agent", e.UserAgent)
}

// InviteRevoked records an admin cancelling a pending link.
func (l *Logger) InviteRevoked(userID, actorUserID int64) {
	l.emit("account invite revoked", "event", "invite.revoked",
		"user_id", userID, "actor_user_id", actorUserID)
}

// AccountCreated records a new account.
func (l *Logger) AccountCreated(userID, actorUserID int64, usernameLen int, role string) {
	l.emit("account created", "event", "account.created",
		"user_id", userID, "actor_user_id", actorUserID, "username_len", usernameLen, "role", role)
}
