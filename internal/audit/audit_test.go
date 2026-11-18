package audit

import (
	"bytes"
	"encoding/json"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

var testKey = []byte("0123456789abcdef0123456789abcdef")

func capture(t *testing.T, emit func(*Logger)) map[string]any {
	t.Helper()
	var buf bytes.Buffer
	emit(New(slog.New(slog.NewJSONHandler(&buf, nil)), testKey))
	var record map[string]any
	if err := json.Unmarshal(buf.Bytes(), &record); err != nil {
		t.Fatalf("log line %q: %v", buf.String(), err)
	}
	return record
}

func check(t *testing.T, record map[string]any, want map[string]any) {
	t.Helper()
	for key, value := range want {
		if got := record[key]; got != value {
			t.Errorf("%s = %v (%T), want %v", key, got, got, value)
		}
	}
	if record["level"] != "INFO" || record["target"] != Target {
		t.Errorf("level/target = %v/%v", record["level"], record["target"])
	}
}

func TestSessionCreatedHashesTheToken(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.SessionCreated(CredentialCreated{Token: "tok", UserID: 1, Method: "password",
			Request: Request{IP: "127.0.0.1", UserAgent: "ua"}})
	})
	check(t, record, map[string]any{
		"msg": "session created", "event": "session.created", "sid": secret.AuditID(testKey, "tok"),
		"user_id": 1.0, "method": "password", "ip": "127.0.0.1", "user_agent": "ua",
	})
}

func TestNoEventLeaksTheRawToken(t *testing.T) {
	var buf bytes.Buffer
	l := New(slog.New(slog.NewTextHandler(&buf, nil)), testKey)
	r := Request{IP: "1.2.3.4", UserAgent: "ua"}
	l.SessionCreated(CredentialCreated{Token: "RAWTOKEN", Request: r})
	l.APITokenCreated(CredentialCreated{Token: "RAWTOKEN", Request: r})
	l.SessionRenewed("RAWTOKEN", 1, time.Now())
	l.SessionTokenRotated("RAWTOKEN", "RAWTOKEN2")
	l.SessionDestroyed("RAWTOKEN", 1, "logout")
	l.MasqueradeStarted(MasqueradeStarted{Token: "RAWTOKEN", NewToken: "RAWTOKEN2", Request: r})
	l.MasqueradeStopped(MasqueradeStopped{Token: "RAWTOKEN", NewToken: "RAWTOKEN2"})
	l.PasskeyRegistered(PasskeyRegistered{Token: "RAWTOKEN", Request: r})
	l.PasskeyRemoved("RAWTOKEN", 1, 2)
	l.SessionReauthenticated("RAWTOKEN", 1, "password")
	l.InviteIssued(InviteIssued{Token: "RAWTOKEN"})
	l.InviteConsumed(InviteConsumed{Token: "RAWTOKEN", Request: r})
	if strings.Contains(buf.String(), "RAWTOKEN") {
		t.Fatalf("raw token in log:\n%s", buf.String())
	}
	if got := strings.Count(buf.String(), "target=rdrs::audit"); got != 12 {
		t.Fatalf("%d events with target, want 12", got)
	}
}

func TestAPITokenCreated(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.APITokenCreated(CredentialCreated{Token: "tok", UserID: 2, Method: "client_login"})
	})
	check(t, record, map[string]any{"event": "api_token.created", "method": "client_login", "msg": "API token created"})
}

func TestSessionRenewedAndRotated(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.SessionRenewed("tok", 1, time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC))
	})
	check(t, record, map[string]any{"event": "session.renewed", "new_expires_at": "2026-01-02 03:04:05 +0000 UTC"})
	record = capture(t, func(l *Logger) { l.SessionTokenRotated("a", "b") })
	check(t, record, map[string]any{"event": "session.token_rotated",
		"sid": secret.AuditID(testKey, "a"), "new_sid": secret.AuditID(testKey, "b")})
}

func TestBulkCountsRenderLikeOptions(t *testing.T) {
	three := uint64(3)
	record := capture(t, func(l *Logger) { l.SessionsDestroyedBulk(1, "revoke_others", &three) })
	check(t, record, map[string]any{"event": "sessions.destroyed", "reason": "revoke_others", "count": "Some(3)"})
	record = capture(t, func(l *Logger) { l.SessionsDestroyedBulk(1, "password_change", nil) })
	check(t, record, map[string]any{"count": "None"})
	record = capture(t, func(l *Logger) { l.APITokensDestroyed(1, "revoke_all", &three) })
	check(t, record, map[string]any{"event": "api_tokens.destroyed", "count": "Some(3)"})
}

func TestSessionDestroyed(t *testing.T) {
	record := capture(t, func(l *Logger) { l.SessionDestroyed("tok", 4, "expired") })
	check(t, record, map[string]any{"event": "session.destroyed", "user_id": 4.0, "reason": "expired"})
}

func TestMasquerade(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.MasqueradeStarted(MasqueradeStarted{Token: "a", NewToken: "b", ActorUserID: 1, TargetUserID: 2,
			Request: Request{IP: "ip", UserAgent: "ua"}})
	})
	check(t, record, map[string]any{"event": "masquerade.started", "actor_user_id": 1.0,
		"target_user_id": 2.0, "ip": "ip", "user_agent": "ua"})
	record = capture(t, func(l *Logger) {
		l.MasqueradeStopped(MasqueradeStopped{Token: "b", NewToken: "c", ActorUserID: 1, RestoredUserID: 1})
	})
	check(t, record, map[string]any{"event": "masquerade.stopped", "restored_user_id": 1.0})
}

func TestPasskey(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.PasskeyRegistered(PasskeyRegistered{Token: "t", UserID: 1, PasskeyID: 7, Name: "MacBook",
			Request: Request{IP: "ip", UserAgent: "ua"}})
	})
	check(t, record, map[string]any{"event": "passkey.registered", "passkey_id": 7.0, "passkey_name": "MacBook"})
	record = capture(t, func(l *Logger) { l.PasskeyRemoved("t", 1, 7) })
	check(t, record, map[string]any{"event": "passkey.removed", "passkey_id": 7.0})
}

func TestLoginEvents(t *testing.T) {
	record := capture(t, func(l *Logger) { l.SessionReauthenticated("t", 1, "forward_auth") })
	check(t, record, map[string]any{"event": "session.reauthenticated", "method": "forward_auth"})
	record = capture(t, func(l *Logger) {
		l.LoginFailed(5, "bad_password", Request{IP: "ip", UserAgent: "ua"})
	})
	check(t, record, map[string]any{"event": "login.failed", "username_len": 5.0, "reason": "bad_password", "ip": "ip"})
	record = capture(t, func(l *Logger) { l.LoginRateLimited("POST /api/session", "login", "ip") })
	check(t, record, map[string]any{"event": "login.rate_limited", "endpoint": "POST /api/session", "bucket": "login"})
}

func TestInviteAndAccountEvents(t *testing.T) {
	record := capture(t, func(l *Logger) {
		l.InviteIssued(InviteIssued{Token: "t", UserID: 2, ActorUserID: 1, Reason: "account_created",
			ExpiresAt: time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)})
	})
	check(t, record, map[string]any{"event": "invite.issued", "iid": secret.AuditID(testKey, "t"),
		"reason": "account_created", "expires_at": "2026-01-02T03:04:05Z"})
	record = capture(t, func(l *Logger) {
		l.InviteConsumed(InviteConsumed{Token: "t", UserID: 2, Request: Request{IP: "ip", UserAgent: "ua"}})
	})
	check(t, record, map[string]any{"event": "invite.consumed", "msg": "account invite redeemed"})
	record = capture(t, func(l *Logger) { l.InviteRevoked(2, 1) })
	check(t, record, map[string]any{"event": "invite.revoked", "actor_user_id": 1.0})
	record = capture(t, func(l *Logger) { l.AccountCreated(2, 1, 5, "user") })
	check(t, record, map[string]any{"event": "account.created", "username_len": 5.0, "role": "user"})
}
