package models

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
)

// Without the explicit delete, the foreign key would null original_user_id and
// leave the admin holding a plain session of the target.
func TestDeleteUserEndsTheirMasquerade(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin, target := mustUser(t, database, "admin"), mustUser(t, database, "target")
		adminSession := newSession(t, database, admin.ID)
		actingToken, err := StartMasquerade(ctx, database, adminSession.SessionToken, target.ID)
		if err != nil {
			t.Fatal(err)
		}

		if err := DeleteUser(ctx, database, admin.ID); err != nil {
			t.Fatal(err)
		}

		if found, _ := FindSessionByToken(ctx, database, actingToken); found != nil {
			t.Errorf("the deleted admin's session survived: %+v", found)
		}
	})
}
