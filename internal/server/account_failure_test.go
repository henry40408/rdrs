package server

import (
	"context"
	"net/http"
	"net/url"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

// TestAccountFormsReportRefusedWrites makes one table refuse writes and
// submits each settings form that depends on it. The user must read why the
// form failed instead of a success banner.
func TestAccountFormsReportRefusedWrites(t *testing.T) {
	const next = "violet-harbor-lantern-quartz-92"
	cases := []struct {
		name, table string
		events      []string
		path        string
		form        url.Values
		want        string
	}{
		{"password", "user", []string{"update"}, "/user-settings/password",
			url.Values{"current_password": {testPassword}, "new_password": {next}, "confirm_password": {next}},
			"Failed to update password."},
		{"other sessions", "session", []string{"delete"}, "/user-settings/sessions/revoke-others", nil,
			"Failed to sign out other sessions."},
		{"one api token", "api_token", []string{"delete", "update"}, "/user-settings/api-tokens/1/revoke", nil,
			"Failed to revoke API token."},
		{"all api tokens", "api_token", []string{"delete", "update"}, "/user-settings/api-tokens/revoke-all", nil,
			"Failed to revoke API tokens."},
		{"preferences", "user_settings", []string{"insert", "update"}, "/user-settings/preferences",
			url.Values{"theme": {"dark"}, "entries_per_page": {"20"}, "retention_read_days": {"0"}},
			"Failed to update preferences."},
		{"linkding", "user_settings", []string{"insert", "update"}, "/user-settings/linkding",
			url.Values{"api_url": {"https://links.example"}, "api_token": {"secret"}},
			"Failed to update Linkding configuration."},
		{"kagi", "user_settings", []string{"insert", "update"}, "/user-settings/kagi",
			url.Values{"language": {"EN"}},
			"Failed to update Kagi configuration."},
		{"kagi clear", "user_settings", []string{"insert", "update"}, "/user-settings/kagi",
			url.Values{"_clear": {"1"}},
			"Failed to clear Kagi configuration."},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := newSignedIn(t)
			// A trigger fires per row, so the table needs a row to refuse.
			switch c.table {
			case "session":
				if _, err := models.CreateSession(context.Background(), f.database, models.NewSession{UserID: f.adminID(t)}); err != nil {
					t.Fatal(err)
				}
			case "api_token":
				f.mustExec(t, `INSERT INTO api_token (user_id, token, expires_at, last_seen_at, user_agent, ip_address)
					VALUES (1, 'tok', '2999-01-01 00:00:00', '2026-01-01 00:00:00', '', '')`)
			}
			f.refuseWrites(t, c.table, c.events...)
			f.expectFlash(t, c.path, c.form, "/user-settings", c.want)
		})
	}
}

func TestPasswordChangeRefusedByTheStoreKeepsTheOldPassword(t *testing.T) {
	const next = "violet-harbor-lantern-quartz-92"
	f := newSignedIn(t)
	f.refuseWrites(t, "user", "update")
	f.postForm("/user-settings/password", url.Values{
		"current_password": {testPassword}, "new_password": {next}, "confirm_password": {next},
	})
	login := f.do(jsonRequest("POST", "/api/session", `{"username":"root","password":"`+testPassword+`"}`))
	if login.Code != 200 && login.Code != 201 {
		t.Errorf("old password stopped working after a refused change: %d %s", login.Code, login.Body)
	}
}

// A failed read of the stored services must not render the settings page as if
// no service were configured.
func TestUserSettingsPageFailsWhenTheServicesCannotBeRead(t *testing.T) {
	f := newSignedIn(t)
	f.mustExec(t, `ALTER TABLE user_settings RENAME TO user_settings_gone`)

	if rec := f.get("/user-settings"); rec.Code != http.StatusInternalServerError {
		t.Fatalf("GET /user-settings = %d, want 500", rec.Code)
	}
}

func (f *signedInFixture) mustExec(t *testing.T, query string) {
	t.Helper()
	if _, err := f.database.ExecContext(context.Background(), query); err != nil {
		t.Fatal(err)
	}
}
