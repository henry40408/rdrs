package auth

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"net/http"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/testutil"
)

type fixture struct {
	t             *testing.T
	database      *db.DB
	authenticator *Authenticator
	logs          *bytes.Buffer
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	database := testutil.SQLite(t)
	logs := &bytes.Buffer{}
	logger := slog.New(slog.NewTextHandler(logs, nil))
	return &fixture{
		t: t, database: database, logs: logs,
		authenticator: &Authenticator{
			DB:     database,
			Config: config.Config{Secret: testSecret},
			Audit:  audit.New(logger, testSecret),
		},
	}
}

func (f *fixture) user(name string, role models.Role) *models.User {
	f.t.Helper()
	user, err := models.CreateUser(context.Background(), f.database, name, "hash", role)
	if err != nil {
		f.t.Fatal(err)
	}
	return user
}

func (f *fixture) session(userID int64) *models.Session {
	f.t.Helper()
	session, err := models.CreateSession(context.Background(), f.database, models.NewSession{UserID: userID})
	if err != nil {
		f.t.Fatal(err)
	}
	return session
}

func (f *fixture) setSessionTimes(sessionID int64, createdAt, expiresAt time.Time) {
	f.t.Helper()
	_, err := f.database.ExecContext(context.Background(),
		"UPDATE session SET created_at = $1, expires_at = $2 WHERE id = $3",
		createdAt.UTC().Format("2006-01-02 15:04:05"), expiresAt.UTC().Format("2006-01-02 15:04:05"), sessionID)
	if err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) request(token string) *http.Request {
	r := requestWithCookies(&http.Cookie{Name: SessionCookieName, Value: secret.SignSession(testSecret, token)})
	ctx, _ := WithRotationSlot(r.Context())
	return r.WithContext(ctx)
}

func TestAuthenticateAcceptsALiveSession(t *testing.T) {
	f := newFixture(t)
	user := f.user("alice", models.RoleUser)
	session := f.session(user.ID)

	principal, err := f.authenticator.Authenticate(f.request(session.SessionToken))
	if err != nil {
		t.Fatal(err)
	}
	if principal.User.ID != user.ID || principal.Session.ID != session.ID || principal.ViaForwardAuth {
		t.Errorf("principal = %+v", principal)
	}
	if principal.CSRFToken != secret.DeriveCSRF(testSecret, session.SessionToken) {
		t.Error("csrf token must derive from the cookie token")
	}
}

func TestAuthenticateRejections(t *testing.T) {
	f := newFixture(t)
	user := f.user("alice", models.RoleUser)
	if _, err := f.authenticator.Authenticate(requestWithCookies()); !errors.Is(err, ErrUnauthorized) {
		t.Errorf("no cookie: %v", err)
	}
	if _, err := f.authenticator.Authenticate(f.request("unknown-token")); !errors.Is(err, ErrUnauthorized) {
		t.Errorf("unknown token: %v", err)
	}
	if err := models.DisableUser(context.Background(), f.database, user.ID); err != nil {
		t.Fatal(err)
	}
	// DisableUser revokes sessions, so the live session is created afterwards.
	session := f.session(user.ID)
	if _, err := f.authenticator.Authenticate(f.request(session.SessionToken)); !errors.Is(err, ErrUserDisabled) {
		t.Errorf("disabled user: %v", err)
	}
	if f.authenticator.AuthenticatePage(f.request(session.SessionToken)) != nil {
		t.Error("page variant must treat a disabled user as anonymous")
	}
}

func TestAuthenticateDeletesAnExpiredSession(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	f.setSessionTimes(session.ID, time.Now().Add(-48*time.Hour), time.Now().Add(-time.Hour))

	if _, err := f.authenticator.Authenticate(f.request(session.SessionToken)); !errors.Is(err, ErrUnauthorized) {
		t.Fatalf("err = %v", err)
	}
	found, err := models.FindSessionByToken(context.Background(), f.database, session.SessionToken)
	if err != nil || found != nil {
		t.Errorf("expired session survived: %+v %v", found, err)
	}
	if !bytes.Contains(f.logs.Bytes(), []byte("reason=expired")) {
		t.Errorf("no session.destroyed audit event: %s", f.logs)
	}
}

func TestAuthenticateSlidesExpiryAndRequestsRotation(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	// Less than half of the 7 day TTL remains.
	f.setSessionTimes(session.ID, time.Now().Add(-4*24*time.Hour), time.Now().Add(2*24*time.Hour))

	r := f.request(session.SessionToken)
	principal, err := f.authenticator.Authenticate(r)
	if err != nil {
		t.Fatal(err)
	}
	slot := r.Context().Value(rotationKey{}).(*RotationSlot)
	if !slot.Requested() {
		t.Error("a renewed session must ask for token rotation")
	}
	if remaining := time.Until(principal.Session.ExpiresAt); remaining < 6*24*time.Hour {
		t.Errorf("expiry not slid: %v remaining", remaining)
	}
	stored, _ := models.FindSessionByToken(context.Background(), f.database, session.SessionToken)
	if stored == nil || time.Until(stored.ExpiresAt) < 6*24*time.Hour {
		t.Error("slid expiry not persisted")
	}
	if !bytes.Contains(f.logs.Bytes(), []byte("session.renewed")) {
		t.Errorf("no renewal audit event: %s", f.logs)
	}
}

func TestAuthenticateLeavesAFreshSessionAlone(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	r := f.request(session.SessionToken)
	if _, err := f.authenticator.Authenticate(r); err != nil {
		t.Fatal(err)
	}
	if r.Context().Value(rotationKey{}).(*RotationSlot).Requested() {
		t.Error("rotation requested for a fresh session")
	}
}

func TestRequestRotationOutsideTheLayerDoesNothing(t *testing.T) {
	RequestRotation(context.Background())
}

func TestAuthenticateAcceptsTheGraceToken(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	rotated, err := models.RotateToken(context.Background(), f.database, session.SessionToken)
	if err != nil || rotated == "" {
		t.Fatalf("rotate: %q %v", rotated, err)
	}
	principal, err := f.authenticator.Authenticate(f.request(session.SessionToken))
	if err != nil {
		t.Fatalf("old token inside its grace interval: %v", err)
	}
	// The form token follows the cookie, not the row, which now holds the new token.
	if principal.CSRFToken != secret.DeriveCSRF(testSecret, session.SessionToken) {
		t.Error("csrf token must follow the cookie token during grace")
	}
}

func TestAdminRequiresAdminRole(t *testing.T) {
	f := newFixture(t)
	admin := f.user("root", models.RoleAdmin)
	member := f.user("alice", models.RoleUser)
	adminSession := f.session(admin.ID)
	memberSession := f.session(member.ID)

	if _, err := f.authenticator.Admin(f.request(adminSession.SessionToken)); err != nil {
		t.Errorf("admin: %v", err)
	}
	if _, err := f.authenticator.Admin(f.request(memberSession.SessionToken)); !errors.Is(err, ErrForbidden) {
		t.Errorf("member: %v", err)
	}
	if f.authenticator.AdminPage(f.request(adminSession.SessionToken)) == nil {
		t.Error("admin page")
	}
	if f.authenticator.AdminPage(f.request(memberSession.SessionToken)) != nil {
		t.Error("a member must not pass as an admin page")
	}
}

func TestAdminWhileMasqueradingFollowsTheOriginalUser(t *testing.T) {
	f := newFixture(t)
	admin := f.user("root", models.RoleAdmin)
	member := f.user("alice", models.RoleUser)
	session := f.session(admin.ID)
	token, err := models.StartMasquerade(context.Background(), f.database, session.SessionToken, member.ID)
	if err != nil {
		t.Fatal(err)
	}
	principal, err := f.authenticator.Admin(f.request(token))
	if err != nil || principal.User.ID != member.ID {
		t.Fatalf("masquerading admin: %+v %v", principal, err)
	}
	if f.authenticator.AdminPage(f.request(token)) == nil {
		t.Error("admin page while masquerading")
	}

	// A demoted original admin loses the masquerade session itself.
	if err := models.UpdateRole(context.Background(), f.database, admin.ID, models.RoleUser); err != nil {
		t.Fatal(err)
	}
	if _, err := f.authenticator.Admin(f.request(token)); !errors.Is(err, ErrUnauthorized) {
		t.Errorf("demoted original: %v", err)
	}
	if f.authenticator.AdminPage(f.request(token)) != nil {
		t.Error("demoted original on a page")
	}
}

func TestRecentlyAuthenticated(t *testing.T) {
	f := newFixture(t)
	user := f.user("alice", models.RoleUser)
	session := f.session(user.ID)
	if _, err := f.authenticator.RecentlyAuthenticated(f.request(session.SessionToken)); err != nil {
		t.Errorf("fresh login: %v", err)
	}
	stale := time.Now().Add(-6 * time.Minute).UTC().Format("2006-01-02 15:04:05")
	if _, err := f.database.ExecContext(context.Background(),
		"UPDATE session SET last_authenticated_at = $1 WHERE id = $2", stale, session.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := f.authenticator.RecentlyAuthenticated(f.request(session.SessionToken)); !errors.Is(err, ErrReauthRequired) {
		t.Errorf("stale login: %v", err)
	}
}
