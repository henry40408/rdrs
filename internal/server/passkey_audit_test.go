package server

import (
	"fmt"
	"log/slog"
	"net/http"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
)

func passkeyAuditApp(t *testing.T) (*itestApp, *capturedLogs) {
	t.Helper()
	logs := &capturedLogs{}
	cfg := itestConfig()
	app := newITestAppWith(t, Options{
		Config: cfg, Audit: audit.New(slog.New(slog.NewTextHandler(logs, nil)), cfg.Secret),
	})
	app.createUserDirectly("pkaudit", fixturePassword)
	return app, logs
}

func TestDeletePasskeyIsAudited(t *testing.T) {
	app, logs := passkeyAuditApp(t)
	passkeyID := insertPasskey(t, app, 1, []byte{1, 2, 3}, "Key", []byte("{}"), 0, nil)
	app.Login("pkaudit")

	wantStatus(t, app.request(http.MethodDelete, fmt.Sprintf("/api/passkeys/%d", passkeyID)), http.StatusNoContent)

	if got := logs.count("event=passkey.removed"); got != 1 {
		t.Errorf("passkey.removed events = %d, want 1", got)
	}
	if got := logs.count(fmt.Sprintf("passkey_id=%d", passkeyID)); got != 1 {
		t.Errorf("events naming the passkey = %d, want 1", got)
	}
}

func TestFailedPasskeyDeleteIsNotAudited(t *testing.T) {
	app, logs := passkeyAuditApp(t)
	passkeyID := insertPasskey(t, app, 1, []byte{1, 2, 3}, "Key", []byte("{}"), 0, nil)
	app.createUserDirectly("pkintruder", fixturePassword)
	app.Login("pkintruder")

	// Unknown id, then another user's passkey: both find no row.
	wantStatus(t, app.request(http.MethodDelete, "/api/passkeys/9999"), http.StatusNotFound)
	wantStatus(t, app.request(http.MethodDelete, fmt.Sprintf("/api/passkeys/%d", passkeyID)), http.StatusNotFound)
	// A database failure.
	execStatistics(t, app, "ALTER TABLE passkey RENAME TO passkey_off")
	wantStatus(t, app.request(http.MethodDelete, fmt.Sprintf("/api/passkeys/%d", passkeyID)), http.StatusInternalServerError)

	if got := logs.count("passkey.removed"); got != 0 {
		t.Errorf("passkey.removed events = %d, want 0", got)
	}
}
