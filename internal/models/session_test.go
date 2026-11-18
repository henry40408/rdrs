package models

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func newSession(t *testing.T, database *db.DB, userID int64) *Session {
	t.Helper()
	session, err := CreateSession(context.Background(), database, NewSession{UserID: userID, UserAgent: "ua", IPAddress: "1.2.3.4"})
	if err != nil {
		t.Fatal(err)
	}
	return session
}

func TestGenerateTokenIsUnpaddedBase64URL(t *testing.T) {
	token := GenerateToken()
	if len(token) != 43 || strings.ContainsAny(token, "=+/") {
		t.Errorf("token %q is not 43 unpadded base64url characters", token)
	}
	if token == GenerateToken() {
		t.Error("tokens must be random")
	}
	if api := GenerateAPIToken(); !strings.HasPrefix(api, APITokenPrefix) || strings.Contains(api, "/") {
		t.Errorf("api token %q", api)
	}
}

func TestCreateSessionStartsInsideReauthWindow(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		session := newSession(t, database, mustUser(t, database, "alice").ID)
		if !session.AuthenticatedRecently(time.Now()) || session.IsExpired() || session.IsMasquerading() {
			t.Errorf("unexpected session %+v", session)
		}
		wantExpiry := time.Now().Add(7 * 24 * time.Hour)
		if d := session.ExpiresAt.Sub(wantExpiry); d < -5*time.Second || d > 5*time.Second {
			t.Errorf("expires_at = %v", session.ExpiresAt)
		}
		found, err := FindSessionByToken(context.Background(), database, session.SessionToken)
		if err != nil || found == nil || found.ID != session.ID {
			t.Fatalf("find: %+v %v", found, err)
		}
		if miss, err := FindSessionByToken(context.Background(), database, "nope"); err != nil || miss != nil {
			t.Errorf("miss = %+v, %v", miss, err)
		}
	})
}

func TestAuthenticatedRecentlyWindow(t *testing.T) {
	now := time.Now()
	for _, tc := range []struct {
		name string
		at   *time.Time
		want bool
	}{
		{"inside", ptr(now.Add(-4 * time.Minute)), true},
		{"outside", ptr(now.Add(-6 * time.Minute)), false},
		{"exactly at the edge", ptr(now.Add(-ReauthWindow)), false},
		{"missing is stale", nil, false},
	} {
		session := Session{LastAuthenticatedAt: tc.at}
		if got := session.AuthenticatedRecently(now); got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestComputeRefreshedExpiry(t *testing.T) {
	now := time.Now()
	day := 24 * time.Hour
	fresh := Session{CreatedAt: now, ExpiresAt: now.Add(7 * day)}
	if fresh.ComputeRefreshedExpiry(now) != nil {
		t.Error("fresh session must not refresh")
	}
	stale := Session{CreatedAt: now.Add(-10 * day), ExpiresAt: now.Add(3 * day)}
	got := stale.ComputeRefreshedExpiry(now)
	if got == nil || !got.Equal(now.Add(7*day)) {
		t.Errorf("stale refresh = %v", got)
	}
	nearCap := Session{CreatedAt: now.Add(-85 * day), ExpiresAt: now.Add(3 * day)}
	got = nearCap.ComputeRefreshedExpiry(now)
	if want := nearCap.CreatedAt.Add(90 * day); got == nil || !got.Equal(want) {
		t.Errorf("capped refresh = %v, want %v", got, want)
	}
	atCap := Session{CreatedAt: now.Add(-85 * day), ExpiresAt: now.Add(5 * day)}
	if atCap.ComputeRefreshedExpiry(now) != nil {
		t.Error("session at the absolute cap must not refresh")
	}
}

func TestRotateTokenKeepsOldTokenInGrace(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		session := newSession(t, database, mustUser(t, database, "alice").ID)

		newToken, err := RotateToken(ctx, database, session.SessionToken)
		if err != nil || newToken == "" {
			t.Fatalf("rotate: %q %v", newToken, err)
		}
		viaOld, _ := FindSessionByToken(ctx, database, session.SessionToken)
		viaNew, _ := FindSessionByToken(ctx, database, newToken)
		if viaOld == nil || viaNew == nil || viaOld.ID != viaNew.ID {
			t.Fatalf("grace lookup failed: old=%+v new=%+v", viaOld, viaNew)
		}
		again, err := RotateToken(ctx, database, session.SessionToken)
		if err != nil || again != "" {
			t.Errorf("rotating the grace token must be a no-op, got %q %v", again, err)
		}

		mustExec(t, database, "UPDATE session SET previous_token_expires_at = $1 WHERE id = $2",
			stamp(time.Now().Add(-time.Second)), session.ID)
		if viaOld, _ := FindSessionByToken(ctx, database, session.SessionToken); viaOld != nil {
			t.Error("grace token still accepted after expiry")
		}
	})
}

func TestDeleteSessionAcceptsGraceToken(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		session := newSession(t, database, mustUser(t, database, "alice").ID)
		newToken, _ := RotateToken(ctx, database, session.SessionToken)
		if err := DeleteSession(ctx, database, session.SessionToken); err != nil {
			t.Fatal(err)
		}
		if found, _ := FindSessionByToken(ctx, database, newToken); found != nil {
			t.Error("session survived logout on its grace token")
		}
	})
}

func TestDeleteUserSessionsExceptKeepsNamedSession(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		keep := newSession(t, database, user.ID)
		newSession(t, database, user.ID)
		newSession(t, database, user.ID)
		keepToken, _ := RotateToken(ctx, database, keep.SessionToken)

		deleted, err := DeleteUserSessionsExcept(ctx, database, user.ID, keep.SessionToken)
		if err != nil || deleted != 2 {
			t.Fatalf("deleted %d, %v; want 2", deleted, err)
		}
		if found, _ := FindSessionByToken(ctx, database, keepToken); found == nil {
			t.Error("kept session was deleted")
		}
	})
}

func TestDeleteUserSessionByIDIsUserScoped(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		alice, bob := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		session := newSession(t, database, alice.ID)
		if n, err := DeleteUserSessionByID(ctx, database, session.ID, bob.ID); err != nil || n != 0 {
			t.Errorf("cross-user delete = %d, %v", n, err)
		}
		if n, err := DeleteUserSessionByID(ctx, database, session.ID, alice.ID); err != nil || n != 1 {
			t.Errorf("own delete = %d, %v", n, err)
		}
	})
}

func TestListUserSessionsNewestFirst(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		first, second := newSession(t, database, user.ID), newSession(t, database, user.ID)
		mustExec(t, database, "UPDATE session SET created_at = $1 WHERE id = $2", db.FormatSpace(time.Now().Add(-time.Hour)), first.ID)
		list, err := ListUserSessions(ctx, database, user.ID)
		if err != nil || len(list) != 2 || list[0].ID != second.ID {
			t.Errorf("list = %+v, %v", list, err)
		}
	})
}

func TestTouchRefreshAndMarkAuthenticated(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		session := newSession(t, database, mustUser(t, database, "alice").ID)

		if err := TouchLastSeen(ctx, database, session); err != nil {
			t.Fatal(err)
		}
		old := time.Now().Add(-2 * time.Minute)
		mustExec(t, database, "UPDATE session SET last_seen_at = $1 WHERE id = $2", stamp(old), session.ID)
		stale, _ := FindSessionByToken(ctx, database, session.SessionToken)
		if err := TouchLastSeen(ctx, database, stale); err != nil {
			t.Fatal(err)
		}
		touched, _ := FindSessionByToken(ctx, database, session.SessionToken)
		approxNow(t, touched.LastSeenAt, 5*time.Second)

		mustExec(t, database, "UPDATE session SET expires_at = $1 WHERE id = $2", stamp(time.Now().Add(24*time.Hour)), session.ID)
		toRefresh, _ := FindSessionByToken(ctx, database, session.SessionToken)
		refreshed, err := RefreshSessionIfNeeded(ctx, database, toRefresh)
		if err != nil || refreshed == nil {
			t.Fatalf("refresh = %v, %v", refreshed, err)
		}
		persisted, _ := FindSessionByToken(ctx, database, session.SessionToken)
		if !sameInstant(persisted.ExpiresAt, *refreshed) {
			t.Errorf("persisted %v, want %v", persisted.ExpiresAt, *refreshed)
		}

		mustExec(t, database, "UPDATE session SET last_authenticated_at = NULL WHERE id = $1", session.ID)
		if at, err := MarkAuthenticated(ctx, database, session.ID); err != nil || time.Since(at) > time.Second {
			t.Errorf("mark = %v, %v", at, err)
		}
		reopened, _ := FindSessionByToken(ctx, database, session.SessionToken)
		if !reopened.AuthenticatedRecently(time.Now()) {
			t.Error("window not reopened")
		}
	})
}

func TestDeleteExpiredSessions(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		live, dead := newSession(t, database, user.ID), newSession(t, database, user.ID)
		mustExec(t, database, "UPDATE session SET expires_at = $1 WHERE id = $2", stamp(time.Now().Add(-time.Second)), dead.ID)
		if n, err := DeleteExpiredSessions(ctx, database); err != nil || n != 1 {
			t.Fatalf("deleted %d, %v", n, err)
		}
		if found, _ := FindSessionByToken(ctx, database, live.SessionToken); found == nil {
			t.Error("live session deleted")
		}
	})
}

func TestMasqueradeRotatesTokenAndRestores(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin, target := mustUser(t, database, "admin"), mustUser(t, database, "target")
		session := newSession(t, database, admin.ID)

		if _, err := StopMasquerade(ctx, database, session.SessionToken); !errors.Is(err, ErrNotMasquerading) {
			t.Errorf("stop without start: %v", err)
		}
		token, err := StartMasquerade(ctx, database, session.SessionToken, target.ID)
		if err != nil || token == session.SessionToken {
			t.Fatalf("start: %q %v", token, err)
		}
		acting, _ := FindSessionByToken(ctx, database, token)
		if acting.UserID != target.ID || acting.OriginalUserID == nil || *acting.OriginalUserID != admin.ID {
			t.Errorf("acting session = %+v", acting)
		}
		if _, err := StartMasquerade(ctx, database, token, target.ID); !errors.Is(err, ErrAlreadyMasquerading) {
			t.Errorf("double start: %v", err)
		}
		restoredToken, err := StopMasquerade(ctx, database, token)
		if err != nil || restoredToken == token {
			t.Fatalf("stop: %q %v", restoredToken, err)
		}
		back, _ := FindSessionByToken(ctx, database, restoredToken)
		if back.UserID != admin.ID || back.IsMasquerading() {
			t.Errorf("restored session = %+v", back)
		}
		if _, err := StartMasquerade(ctx, database, "unknown", target.ID); !errors.Is(err, ErrUnauthorized) {
			t.Errorf("unknown token: %v", err)
		}
	})
}
