package server

import (
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

// Port of tests/client_info_test.rs: PUT /api/user/client-info and the
// settings-page prefill its hint links to.

func authedITestApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	response := app.PostJSON("/api/setup", map[string]string{"username": "testuser", "password": fixturePassword})
	if response.Code != http.StatusCreated {
		t.Fatalf("setup = %d: %s", response.Code, response.Text())
	}
	app.Login("testuser")
	return app
}

// hintsFor reports client info and returns the hints it earned. It also checks
// that no flash cookie was set, since the next request to read flashes would
// consume it.
func hintsFor(t *testing.T, app *itestApp, body map[string]any) []map[string]any {
	t.Helper()
	response := app.PutJSON("/api/user/client-info", body)
	if response.Code != http.StatusOK {
		t.Fatalf("client-info = %d: %s", response.Code, response.Text())
	}
	if response.Cookie("flash") != nil {
		t.Error("a client-info report must not set a flash cookie")
	}
	var decoded struct {
		Hints []map[string]any `json:"hints"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &decoded); err != nil {
		t.Fatal(err)
	}
	return decoded.Hints
}

func assertNoHint(t *testing.T, app *itestApp, body map[string]any) {
	t.Helper()
	if hints := hintsFor(t, app, body); len(hints) != 0 {
		t.Fatalf("hints = %v, want none", hints)
	}
}

func storeTimezone(t *testing.T, app *itestApp, name string) {
	t.Helper()
	response := app.PostForm("/user-settings/preferences", url.Values{
		"theme": {"system"}, "entries_per_page": {"30"}, "retention_read_days": {"0"}, "timezone": {name},
	})
	if response.Code != http.StatusSeeOther {
		t.Fatalf("preferences = %d: %s", response.Code, response.Text())
	}
}

func TestAMatchingTimezoneEarnsNoHint(t *testing.T) {
	assertNoHint(t, authedITestApp(t), map[string]any{"timezone": "UTC"})
}

func TestADifferentTimezoneEarnsAHintLinkingToPreferences(t *testing.T) {
	hints := hintsFor(t, authedITestApp(t), map[string]any{"timezone": "Asia/Taipei"})
	if len(hints) != 1 {
		t.Fatalf("hints = %v", hints)
	}
	message, _ := hints[0]["message"].(string)
	if !strings.Contains(message, "Asia/Taipei") || !strings.Contains(message, "UTC") {
		t.Errorf("the hint names both the reported and the stored zone: %q", message)
	}
	if hints[0]["action_href"] != "/user-settings?timezone=Asia%2FTaipei" {
		t.Errorf("action_href = %v", hints[0]["action_href"])
	}
	if label, _ := hints[0]["action_label"].(string); label == "" {
		t.Error("the hint needs an action label")
	}
}

func TestAHintChangesNoSetting(t *testing.T) {
	app := authedITestApp(t)
	hintsFor(t, app, map[string]any{"timezone": "Asia/Taipei"})
	if !strings.Contains(app.GetOK("/user-settings"), `name="timezone" value="UTC"`) {
		t.Fatal("the stored timezone changed")
	}
}

func TestAnAliasOfTheStoredTimezoneEarnsNoHint(t *testing.T) {
	app := authedITestApp(t)
	// Browsers report either spelling of these.
	assertNoHint(t, app, map[string]any{"timezone": "Etc/UTC"})
	storeTimezone(t, app, "Asia/Kolkata")
	assertNoHint(t, app, map[string]any{"timezone": "Asia/Calcutta"})
}

func TestAnUnknownTimezoneNameIsRefused(t *testing.T) {
	response := authedITestApp(t).PutJSON("/api/user/client-info", map[string]any{"timezone": "Mars/Olympus_Mons"})
	if response.Code != http.StatusBadRequest {
		t.Fatalf("status %d, want 400", response.Code)
	}
}

func TestAReportWithoutFieldsEarnsNoHint(t *testing.T) {
	assertNoHint(t, authedITestApp(t), map[string]any{})
}

func TestAnAnonymousReportIsRefused(t *testing.T) {
	response := defaultITestApp(t).PutJSON("/api/user/client-info", map[string]any{"timezone": "Asia/Taipei"})
	if response.Code < 400 || response.Code >= 500 {
		t.Fatalf("status %d, want a client error", response.Code)
	}
}

func TestTheSettingsPagePrefillsASuggestedTimezoneWithoutSavingIt(t *testing.T) {
	app := authedITestApp(t)

	page := app.GetOK("/user-settings?timezone=Asia%2FTaipei")
	if !strings.Contains(page, `name="timezone" value="Asia/Taipei"`) || !strings.Contains(page, `data-testid="timezone-suggestion"`) {
		t.Error("the suggestion is not prefilled")
	}

	page = app.GetOK("/user-settings")
	if !strings.Contains(page, `name="timezone" value="UTC"`) || strings.Contains(page, `data-testid="timezone-suggestion"`) {
		t.Error("the suggestion was saved")
	}
}

func TestTheSettingsPageIgnoresASuggestionItCannotUse(t *testing.T) {
	app := authedITestApp(t)
	storeTimezone(t, app, "Asia/Tokyo")
	for _, query := range []string{
		"timezone=Mars%2FOlympus_Mons",
		"timezone=",
		"timezone=Asia%2FTokyo",
		"timezone=Asia%2FTokyo&timezone=Asia%2FTaipei",
		"other=1",
	} {
		page := app.GetOK("/user-settings?" + query)
		if !strings.Contains(page, `name="timezone" value="Asia/Tokyo"`) {
			t.Errorf("%s: the stored timezone is not shown", query)
		}
		if strings.Contains(page, `data-testid="timezone-suggestion"`) {
			t.Errorf("%s: a suggestion is shown", query)
		}
	}
}
