package models

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
)

func TestDemotingAnAdminEndsTheirMasquerade(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin, target := mustUser(t, database, "admin"), mustUser(t, database, "target")
		actingToken, err := StartMasquerade(ctx, database, newSession(t, database, admin.ID).SessionToken, target.ID)
		if err != nil {
			t.Fatal(err)
		}

		if err := UpdateRole(ctx, database, admin.ID, RoleUser); err != nil {
			t.Fatal(err)
		}

		if found, _ := FindSessionByToken(ctx, database, actingToken); found != nil {
			t.Error("the demoted admin's masquerade session survived")
		}
	})
}

func TestPromotingKeepsMasqueradeSessions(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin, target := mustUser(t, database, "admin"), mustUser(t, database, "target")
		actingToken, err := StartMasquerade(ctx, database, newSession(t, database, admin.ID).SessionToken, target.ID)
		if err != nil {
			t.Fatal(err)
		}

		if err := UpdateRole(ctx, database, admin.ID, RoleAdmin); err != nil {
			t.Fatal(err)
		}

		if found, _ := FindSessionByToken(ctx, database, actingToken); found == nil {
			t.Error("re-asserting the admin role ended the masquerade")
		}
	})
}
