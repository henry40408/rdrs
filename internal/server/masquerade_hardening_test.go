package server

import (
	"context"
	"database/sql"
	"errors"
	"net/http"
	"testing"
)

// masqueradingApp is an admin who has just started masquerading as "target".
func masqueradingApp(t *testing.T) *itestApp {
	t.Helper()
	app := adminApp(t)
	seedTarget(app)
	started := app.request(http.MethodPost, "/admin/users/2/masquerade")
	wantStatus(t, started, http.StatusSeeOther)
	app.ApplyCSRF(started)
	return app
}

// A passkey added while masquerading would still sign in as the target once
// the masquerade ends.
func TestPasskeyRoutesRefusedWhileMasquerading(t *testing.T) {
	app := masqueradingApp(t)

	wantStatus(t, app.PostJSON("/api/passkey/register/start", map[string]any{}), http.StatusForbidden)
	wantStatus(t, app.PostJSON("/api/passkey/register/finish", map[string]any{"name": "backdoor"}), http.StatusForbidden)
	wantStatus(t, app.PutJSON("/api/passkeys/1", map[string]any{"name": "renamed"}), http.StatusForbidden)
	wantStatus(t, app.Do(itestRequest{Method: http.MethodDelete, Path: "/api/passkeys/1"}), http.StatusForbidden)
}

// Pointing api_url at another host and pressing Save would send the stored
// token there.
func TestServiceCredentialFormsRefusedWhileMasquerading(t *testing.T) {
	app := masqueradingApp(t)
	storedServices := func() string {
		var stored sql.NullString
		err := app.DB.QueryRowContext(context.Background(),
			"SELECT save_services FROM user_settings WHERE user_id = 2").Scan(&stored)
		if err != nil && !errors.Is(err, sql.ErrNoRows) {
			t.Fatal(err)
		}
		return stored.String
	}
	before := storedServices()

	wantSeeOther(t, app.postPairs("/user-settings/linkding", "api_url", "https://attacker.example"), "/user-settings")
	wantSeeOther(t, app.postPairs("/user-settings/kagi", "session_link", "https://kagi.com/search?token=abc"), "/user-settings")
	wantSeeOther(t, app.postPairs("/user-settings/kagi", "_clear", "1"), "/user-settings")

	if storedServices() != before {
		t.Error("service credentials changed while masquerading")
	}
}
