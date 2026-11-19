package models

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
)

// A masquerading session carries the target's user_id, so revoking by user_id
// alone would leave a disabled or deleted admin's session alive.
func TestDeleteUserSessionsAlsoRemovesSessionsMasqueradedFrom(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin, target := mustUser(t, database, "admin"), mustUser(t, database, "target")
		adminSession := newSession(t, database, admin.ID)
		actingToken, err := StartMasquerade(ctx, database, adminSession.SessionToken, target.ID)
		if err != nil {
			t.Fatal(err)
		}
		targetSession := newSession(t, database, target.ID)

		if err := DeleteUserSessions(ctx, database, admin.ID); err != nil {
			t.Fatal(err)
		}

		if found, _ := FindSessionByToken(ctx, database, actingToken); found != nil {
			t.Error("the admin's masquerading session survived")
		}
		if found, _ := FindSessionByToken(ctx, database, targetSession.SessionToken); found == nil {
			t.Error("the target's own session was deleted")
		}
	})
}
