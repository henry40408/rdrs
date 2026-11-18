package models

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

var inviteSecret = []byte("invite-test-secret")

func TestIssuedInviteIsFoundByRawTokenAndNeverStored(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		invite, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "raw-token"})
		if err != nil {
			t.Fatal(err)
		}
		if strings.Contains(invite.TokenHash, "raw-token") || len(invite.TokenHash) != 64 {
			t.Errorf("token_hash = %q", invite.TokenHash)
		}
		found, err := FindInviteByToken(ctx, database, inviteSecret, "raw-token")
		if err != nil || found == nil || found.ID != invite.ID || !found.IsLive(time.Now()) {
			t.Fatalf("find: %+v %v", found, err)
		}
		if miss, _ := FindInviteByToken(ctx, database, inviteSecret, "other"); miss != nil {
			t.Error("unknown token found")
		}
		if miss, _ := FindInviteByToken(ctx, database, []byte("different"), "raw-token"); miss != nil {
			t.Error("token found under a different secret")
		}
	})
}

func TestIssuingAgainRevokesThePreviousLink(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		if _, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "first"}); err != nil {
			t.Fatal(err)
		}
		if _, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "second"}); err != nil {
			t.Fatal(err)
		}
		if old, _ := FindInviteByToken(ctx, database, inviteSecret, "first"); old != nil {
			t.Error("old link survived")
		}
		if cur, _ := FindInviteByToken(ctx, database, inviteSecret, "second"); cur == nil {
			t.Error("new link missing")
		}
	})
}

func TestIssueKeepsOldLinkWhenInsertFails(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		other := mustUser(t, database, "bob")
		if _, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "dup"}); err != nil {
			t.Fatal(err)
		}
		// A token_hash collision with another account's link makes the insert fail.
		if _, err := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: other.ID, Token: "dup"}); err == nil {
			t.Fatal("expected a unique violation")
		}
		if cur, _ := FindInviteByToken(ctx, database, inviteSecret, "dup"); cur == nil || cur.UserID != user.ID {
			t.Errorf("original link lost: %+v", cur)
		}
	})
}

func TestRedeemSetsPasswordAndSpendsOnce(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		seedCredentials(t, database, user.ID)
		invite, _ := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "tok"})

		spent, err := RedeemInvite(ctx, database, RedeemInviteParams{InviteID: invite.ID, UserID: user.ID, PasswordHash: "new-hash"})
		if err != nil || !spent {
			t.Fatalf("first redeem = %v, %v", spent, err)
		}
		found, _ := FindUserByID(ctx, database, user.ID)
		if found.PasswordHash != "new-hash" {
			t.Errorf("password = %q", found.PasswordHash)
		}
		if s, k := credentialCounts(t, database, user.ID); s != 0 || k != 0 {
			t.Errorf("credentials left: %d sessions, %d tokens", s, k)
		}
		spent, err = RedeemInvite(ctx, database, RedeemInviteParams{InviteID: invite.ID, UserID: user.ID, PasswordHash: "again"})
		if err != nil || spent {
			t.Errorf("second redeem = %v, %v", spent, err)
		}
		if again, _ := FindUserByID(ctx, database, user.ID); again.PasswordHash != "new-hash" {
			t.Error("second redeem changed the password")
		}
	})
}

func TestRedeemKeepsLinkLiveWhenPasswordWriteFails(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		invite, _ := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "tok"})
		mustExec(t, database, "DROP TABLE api_token")

		if spent, err := RedeemInvite(ctx, database, RedeemInviteParams{InviteID: invite.ID, UserID: user.ID, PasswordHash: "new"}); err == nil || spent {
			t.Fatalf("redeem = %v, %v; want failure", spent, err)
		}
		if live, _ := FindLiveInviteForUser(ctx, database, user.ID); live == nil {
			t.Error("link was spent despite the failed password write")
		}
	})
}

func TestInviteLiveness(t *testing.T) {
	now := time.Now()
	if (&UserInvite{ExpiresAt: now.Add(-time.Second)}).IsLive(now) {
		t.Error("expired invite is live")
	}
	if (&UserInvite{ExpiresAt: now.Add(time.Hour), ConsumedAt: &now}).IsLive(now) {
		t.Error("consumed invite is live")
	}
	if !(&UserInvite{ExpiresAt: now.Add(time.Hour)}).IsLive(now) {
		t.Error("fresh invite is not live")
	}
}

func TestPendingRevokeAndStaleCleanup(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		alice, bob := mustUser(t, database, "alice"), mustUser(t, database, "bob")
		IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: alice.ID, Token: "a"})
		spentInvite, _ := IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: bob.ID, Token: "b"})

		if live, _ := FindLiveInviteForUser(ctx, database, alice.ID); live == nil {
			t.Error("pending invite not reported")
		}
		if n, err := RevokeInvitesForUser(ctx, database, alice.ID); err != nil || n != 1 {
			t.Errorf("revoke = %d, %v", n, err)
		}
		if live, _ := FindLiveInviteForUser(ctx, database, alice.ID); live != nil {
			t.Error("revoked invite still pending")
		}
		consumeInvite(ctx, database, spentInvite.ID)
		if live, _ := FindLiveInviteForUser(ctx, database, bob.ID); live != nil {
			t.Error("consumed invite reported as pending")
		}
		if n, err := DeleteStaleInvites(ctx, database); err != nil || n != 1 {
			t.Errorf("stale cleanup = %d, %v", n, err)
		}
	})
}

func TestDeletingAccountTakesInviteWithIt(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		IssueInvite(ctx, database, IssueInviteParams{SecretKey: inviteSecret, UserID: user.ID, Token: "a"})
		if err := DeleteUser(ctx, database, user.ID); err != nil {
			t.Fatal(err)
		}
		if found, _ := FindInviteByToken(ctx, database, inviteSecret, "a"); found != nil {
			t.Error("invite outlived its account")
		}
	})
}
