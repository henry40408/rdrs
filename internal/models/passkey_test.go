package models

import (
	"bytes"
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func mustPasskey(t *testing.T, database *db.DB, userID int64, credential byte, name string) *Passkey {
	t.Helper()
	passkey, err := CreatePasskey(context.Background(), database, NewPasskey{
		UserID: userID, CredentialID: []byte{credential}, PublicKey: []byte{credential, 9}, Name: name,
	})
	if err != nil {
		t.Fatal(err)
	}
	return passkey
}

func TestCreateAndFindPasskey(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		credential, key := []byte{1, 2, 3, 4}, []byte{5, 6, 7, 8}
		created, err := CreatePasskey(ctx, database, NewPasskey{
			UserID: user.ID, CredentialID: credential, PublicKey: key, Name: "My Passkey", Transports: ptr("usb,nfc"),
		})
		if err != nil {
			t.Fatal(err)
		}
		if created.UserID != user.ID || !bytes.Equal(created.CredentialID, credential) || !bytes.Equal(created.PublicKey, key) ||
			created.Name != "My Passkey" || created.Transports == nil || *created.Transports != "usb,nfc" || created.LastUsedAt != nil {
			t.Errorf("created = %+v", created)
		}
		byID, err := FindPasskeyByID(ctx, database, created.ID)
		if err != nil || byID == nil || byID.ID != created.ID {
			t.Errorf("by id = %+v, %v", byID, err)
		}
		byCredential, err := FindPasskeyByCredentialID(ctx, database, credential)
		if err != nil || byCredential == nil || byCredential.ID != created.ID {
			t.Errorf("by credential = %+v, %v", byCredential, err)
		}
		if missing, err := FindPasskeyByCredentialID(ctx, database, []byte{99, 99, 99}); err != nil || missing != nil {
			t.Errorf("missing = %+v, %v", missing, err)
		}
	})
}

func TestPasskeysAreOnlyEverListedPerUser(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		first, second := mustUser(t, database, "user1"), mustUser(t, database, "user2")
		mustPasskey(t, database, first.ID, 1, "User1 Passkey A")
		mustPasskey(t, database, first.ID, 3, "User1 Passkey B")
		mustPasskey(t, database, second.ID, 2, "User2 Passkey")
		mine, err := ListUserPasskeys(ctx, database, first.ID)
		if err != nil || len(mine) != 2 {
			t.Fatalf("mine = %+v, %v", mine, err)
		}
		for _, p := range mine {
			if p.UserID != first.ID {
				t.Errorf("listed another user's passkey: %+v", p)
			}
		}
	})
}

func TestUpdatePasskeyCounterMarksItUsed(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		passkey := mustPasskey(t, database, mustUser(t, database, "alice").ID, 1, "Passkey")
		if passkey.Counter != 0 || passkey.LastUsedAt != nil {
			t.Fatalf("fresh passkey = %+v", passkey)
		}
		if err := UpdatePasskeyCounter(ctx, database, passkey.ID, 5); err != nil {
			t.Fatal(err)
		}
		updated, _ := FindPasskeyByID(ctx, database, passkey.ID)
		if updated.Counter != 5 || updated.LastUsedAt == nil || time.Since(*updated.LastUsedAt) > time.Minute {
			t.Errorf("updated = %+v", updated)
		}
	})
}

func TestRenameAndDeletePasskeyAreOwnerScoped(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		owner, intruder := mustUser(t, database, "user1"), mustUser(t, database, "user2")
		passkey := mustPasskey(t, database, owner.ID, 1, "Old Name")

		if err := RenamePasskey(ctx, database, passkey.ID, intruder.ID, "Stolen"); !errors.Is(err, ErrPasskeyNotFound) {
			t.Errorf("rename by another user: %v", err)
		}
		if err := DeletePasskey(ctx, database, passkey.ID, intruder.ID); !errors.Is(err, ErrPasskeyNotFound) {
			t.Errorf("delete by another user: %v", err)
		}
		if err := RenamePasskey(ctx, database, passkey.ID, owner.ID, "New Name"); err != nil {
			t.Fatal(err)
		}
		if got, _ := FindPasskeyByID(ctx, database, passkey.ID); got == nil || got.Name != "New Name" {
			t.Errorf("after rename = %+v", got)
		}
		if err := DeletePasskey(ctx, database, passkey.ID, owner.ID); err != nil {
			t.Fatal(err)
		}
		if got, _ := FindPasskeyByID(ctx, database, passkey.ID); got != nil {
			t.Errorf("after delete = %+v", got)
		}
		if err := DeletePasskey(ctx, database, passkey.ID, owner.ID); !errors.Is(err, ErrPasskeyNotFound) {
			t.Errorf("second delete: %v", err)
		}
	})
}
