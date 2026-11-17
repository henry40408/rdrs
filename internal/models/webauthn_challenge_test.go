package models

import (
	"bytes"
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func TestCreateAndTakeChallengeIsSingleUse(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		bytesIn := []byte{1, 2, 3, 4}
		created, err := CreateChallenge(ctx, database, NewChallenge{
			Challenge: bytesIn, UserID: &user.ID, ChallengeType: ChallengeRegistration, StateData: `{"some":"data"}`,
		})
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(created.Challenge, bytesIn) || created.UserID == nil || *created.UserID != user.ID ||
			created.ChallengeType != ChallengeRegistration || created.StateData != `{"some":"data"}` {
			t.Errorf("created = %+v", created)
		}
		if ttl := created.ExpiresAt.Sub(created.CreatedAt); ttl < 4*time.Minute || ttl > 6*time.Minute {
			t.Errorf("ttl = %v", ttl)
		}
		taken, err := TakeChallenge(ctx, database, &user.ID, ChallengeRegistration)
		if err != nil || !bytes.Equal(taken.Challenge, bytesIn) {
			t.Fatalf("taken = %+v, %v", taken, err)
		}
		if _, err := TakeChallenge(ctx, database, &user.ID, ChallengeRegistration); !errors.Is(err, ErrChallengeNotFound) {
			t.Errorf("second take: %v", err)
		}
	})
}

func TestAnonymousAuthenticationChallenge(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		_, err := CreateChallenge(ctx, database, NewChallenge{
			Challenge: []byte{5, 6, 7, 8}, ChallengeType: ChallengeAuthentication, StateData: "{}",
		})
		if err != nil {
			t.Fatal(err)
		}
		// A user-bound lookup and the wrong type must not see an anonymous one.
		if _, err := TakeChallenge(ctx, database, &user.ID, ChallengeAuthentication); !errors.Is(err, ErrChallengeNotFound) {
			t.Errorf("user-bound take: %v", err)
		}
		if _, err := TakeChallenge(ctx, database, nil, ChallengeRegistration); !errors.Is(err, ErrChallengeNotFound) {
			t.Errorf("wrong type take: %v", err)
		}
		taken, err := TakeChallenge(ctx, database, nil, ChallengeAuthentication)
		if err != nil || taken.UserID != nil || !bytes.Equal(taken.Challenge, []byte{5, 6, 7, 8}) {
			t.Errorf("taken = %+v, %v", taken, err)
		}
	})
}

func TestExpiredChallengeIsNotTaken(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		if _, err := CreateChallenge(ctx, database, NewChallenge{
			Challenge: []byte{1}, UserID: &user.ID, ChallengeType: ChallengeRegistration, StateData: "{}",
		}); err != nil {
			t.Fatal(err)
		}
		mustExec(t, database, `UPDATE webauthn_challenge SET expires_at = $1`, stamp(time.Now().Add(-time.Minute)))
		if _, err := TakeChallenge(ctx, database, &user.ID, ChallengeRegistration); !errors.Is(err, ErrChallengeNotFound) {
			t.Errorf("expired take: %v", err)
		}
	})
}

func TestTakeChallengePrefersTheNewest(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		for _, b := range []byte{1, 2} {
			if _, err := CreateChallenge(ctx, database, NewChallenge{
				Challenge: []byte{b}, UserID: &user.ID, ChallengeType: ChallengeRegistration, StateData: "{}",
			}); err != nil {
				t.Fatal(err)
			}
		}
		taken, err := TakeChallenge(ctx, database, &user.ID, ChallengeRegistration)
		if err != nil || !bytes.Equal(taken.Challenge, []byte{2}) {
			t.Errorf("taken = %+v, %v", taken, err)
		}
	})
}

func TestParseChallengeType(t *testing.T) {
	for _, kind := range []ChallengeType{ChallengeRegistration, ChallengeAuthentication} {
		if got, ok := ParseChallengeType(string(kind)); !ok || got != kind {
			t.Errorf("%s -> %s, %v", kind, got, ok)
		}
	}
	if _, ok := ParseChallengeType("invalid"); ok {
		t.Error("invalid must not parse")
	}
}
