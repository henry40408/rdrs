package server

import (
	"context"
	"log/slog"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/models"
)

func TestReissueInviteAuditsWhyTheLinkWasIssued(t *testing.T) {
	logs := &capturedLogs{}
	cfg := itestConfig()
	app := newITestAppWith(t, Options{
		Config: cfg, Audit: audit.New(slog.New(slog.NewTextHandler(logs, nil)), cfg.Secret),
	})
	ctx := context.Background()
	withPassword := app.SeedAccount("settled", fixturePassword, models.RoleUser)
	neverSet, err := models.CreateUser(ctx, app.DB, "newcomer", noPasswordHash, models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}

	if _, err := app.App.reissueInvite(ctx, withPassword.ID, withPassword.ID); err != nil {
		t.Fatal(err)
	}
	if got := logs.count("reason=password_reset"); got != 1 || logs.count("reason=account_created") != 0 {
		t.Fatalf("an account with a password: %s", logs.buffer.String())
	}
	if _, err := app.App.reissueInvite(ctx, neverSet.ID, withPassword.ID); err != nil {
		t.Fatal(err)
	}
	if got := logs.count("reason=account_created"); got != 1 || logs.count("reason=password_reset") != 1 {
		t.Fatalf("an account without a password: %s", logs.buffer.String())
	}
	if _, err := app.App.reissueInvite(ctx, neverSet.ID+100, withPassword.ID); err != ErrUserNotFound {
		t.Errorf("unknown user error = %v", err)
	}
	if strings.Count(logs.buffer.String(), "invite.issued") != 2 {
		t.Errorf("a failed reissue was audited: %s", logs.buffer.String())
	}
}
