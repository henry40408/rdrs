package models

import (
	"context"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func newToken(t *testing.T, database *db.DB, userID int64) *APIToken {
	t.Helper()
	token, err := CreateAPIToken(context.Background(), database, NewAPIToken{UserID: userID, Kind: "greader", Label: "label", UserAgent: "ua", IPAddress: "1.2.3.4"})
	if err != nil {
		t.Fatal(err)
	}
	return token
}

func TestCreateAndFindAPIToken(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		token := newToken(t, database, mustUser(t, database, "alice").ID)
		found, err := FindAPIToken(ctx, database, token.Token)
		if err != nil || found == nil || found.ID != token.ID || found.Label != "label" || found.IsExpired() {
			t.Fatalf("find: %+v %v", found, err)
		}
		if miss, err := FindAPIToken(ctx, database, "nope"); err != nil || miss != nil {
			t.Errorf("miss = %+v, %v", miss, err)
		}
	})
}

func TestComputeRefreshedAPITokenExpiry(t *testing.T) {
	now := time.Now()
	day := 24 * time.Hour
	if (&APIToken{ExpiresAt: now.Add(89 * day)}).ComputeRefreshedExpiry(now) != nil {
		t.Error("fresh token must not refresh")
	}
	got := (&APIToken{ExpiresAt: now.Add(10 * day)}).ComputeRefreshedExpiry(now)
	if got == nil || !got.Equal(now.Add(90*day)) {
		t.Errorf("refresh = %v", got)
	}
}

func TestDeleteAPITokenIsUserScoped(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		alice, bob := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		token := newToken(t, database, alice.ID)
		if err := DeleteToken(ctx, database, token.ID, bob.ID); err != nil {
			t.Fatal(err)
		}
		if found, _ := FindAPIToken(ctx, database, token.Token); found == nil {
			t.Fatal("another user revoked the token")
		}
		if err := DeleteToken(ctx, database, token.ID, alice.ID); err != nil {
			t.Fatal(err)
		}
		if found, _ := FindAPIToken(ctx, database, token.Token); found != nil {
			t.Error("token survived its owner's delete")
		}
	})
}

func TestDeleteExpiredAPITokens(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		live, dead := newToken(t, database, user.ID), newToken(t, database, user.ID)
		mustExec(t, database, "UPDATE api_token SET expires_at = $1 WHERE id = $2", stamp(time.Now().Add(-time.Second)), dead.ID)
		if n, err := DeleteExpiredTokens(ctx, database); err != nil || n != 1 {
			t.Fatalf("deleted %d, %v", n, err)
		}
		if found, _ := FindAPIToken(ctx, database, live.Token); found == nil {
			t.Error("live token deleted")
		}
	})
}

func TestCreatePrunesPastPerUserCap(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user, other := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		otherToken := newToken(t, database, other.ID)
		var first, last *APIToken
		for i := range MaxTokensPerUser + 3 {
			last = newToken(t, database, user.ID)
			if i == 0 {
				first = last
			}
		}
		tokens, err := ListUserTokens(ctx, database, user.ID)
		if err != nil || len(tokens) != MaxTokensPerUser {
			t.Fatalf("tokens = %d, %v; want %d", len(tokens), err, MaxTokensPerUser)
		}
		if found, _ := FindAPIToken(ctx, database, first.Token); found != nil {
			t.Error("oldest token survived pruning")
		}
		if found, _ := FindAPIToken(ctx, database, last.Token); found == nil {
			t.Error("newest token was pruned")
		}
		if found, _ := FindAPIToken(ctx, database, otherToken.Token); found == nil {
			t.Error("pruning touched another user's token")
		}
	})
}

func TestTouchAndRefreshAPIToken(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		token := newToken(t, database, mustUser(t, database, "alice").ID)
		mustExec(t, database, "UPDATE api_token SET last_seen_at = $1, expires_at = $2 WHERE id = $3",
			stamp(time.Now().Add(-time.Hour)), stamp(time.Now().Add(24*time.Hour)), token.ID)
		stale, _ := FindAPIToken(ctx, database, token.Token)
		refreshed, err := TouchAndRefreshAPIToken(ctx, database, stale)
		if err != nil || refreshed == nil {
			t.Fatalf("refresh = %v, %v", refreshed, err)
		}
		after, _ := FindAPIToken(ctx, database, token.Token)
		approxNow(t, after.LastSeenAt, 5*time.Second)
		if !sameInstant(after.ExpiresAt, *refreshed) {
			t.Errorf("expires_at = %v, want %v", after.ExpiresAt, *refreshed)
		}
	})
}
