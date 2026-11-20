package worker

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

func countInvites(t *testing.T, f fixture) int {
	t.Helper()
	var count int
	row := f.database.QueryRowContext(context.Background(), "SELECT COUNT(*) FROM user_invite")
	must(t, row.Scan(&count))
	return count
}

func TestCleanupSweepDeletesStaleInvitesAndKeepsLiveOnes(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	other, err := models.CreateUser(ctx, f.database, "other", "hash", models.RoleUser)
	must(t, err)
	key := []byte("0123456789abcdef0123456789abcdef")
	for userID, token := range map[int64]string{f.user.ID: "expired", other.ID: "live"} {
		_, err := models.IssueInvite(ctx, f.database, models.IssueInviteParams{SecretKey: key, UserID: userID, Token: token})
		must(t, err)
	}
	f.exec(t, "UPDATE user_invite SET expires_at = $1 WHERE user_id = $2",
		db.FormatRustRFC3339(time.Now().Add(-time.Hour)), f.user.ID)
	logs := captureDebugLogs(t)

	(&Cleanup{DB: f.database}).Sweep(ctx)

	if got := countInvites(t, f); got != 1 {
		t.Fatalf("invites left = %d, want only the live one", got)
	}
	if live, err := models.FindInviteByToken(ctx, f.database, key, "live"); err != nil || live == nil {
		t.Errorf("live invite was removed: %v, %v", live, err)
	}
	if !strings.Contains(logs.String(), "event=cleanup.swept kind=invite count=1") {
		t.Errorf("logs = %s", logs)
	}
}
