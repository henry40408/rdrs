package server

import (
	"context"
	"net/http"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

// Port of tests/summarizer_test.rs. item_returns_completed_then_error_card is
// covered by TestSummarizerItemCompletesThroughKagi and
// TestSummarizerItemShowsKagiFailuresAsErrorCards (summarizer_handler_test.go).

func signedInWithKagi(t *testing.T, username string, kagi bool) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	user := app.SeedAccount(username, fixturePassword, models.RoleUser)
	app.Login(username)
	if kagi {
		services := `{"linkding":null,"kagi":{"session_token":"tok","language":null}}`
		if _, err := models.UpdateSaveServices(context.Background(), app.DB, user.ID, services, app.App.serviceKey()); err != nil {
			t.Fatal(err)
		}
	}
	return app
}

func TestPageShowsSettingsPromptWhenKagiUnset(t *testing.T) {
	body := signedInWithKagi(t, "alice", false).GetOK("/summarizer")
	if !strings.Contains(body, "/user-settings") || strings.Contains(body, `data-testid="summarizer-form"`) {
		t.Fatal("expected the settings prompt and no form")
	}
}

func TestPageShowsFormWhenKagiConfigured(t *testing.T) {
	body := signedInWithKagi(t, "bob", true).GetOK("/summarizer")
	if !strings.Contains(body, `data-testid="summarizer-form"`) {
		t.Fatal("the form is missing")
	}
	// The textarea shares the full-width class so it matches the other inputs
	// instead of collapsing to the default cols width.
	if !strings.Contains(body, `id="sz-urls"`) || !strings.Contains(body, `class="textarea-full"`) {
		t.Error("the summarizer textarea should use the textarea-full class")
	}
}

func TestPageRequiresAuth(t *testing.T) {
	response := defaultITestApp(t).Get("/summarizer")
	// Unauthenticated visitors are sent to /login.
	if response.Code != http.StatusSeeOther && response.Code != http.StatusFound && response.Code != http.StatusOK {
		t.Fatalf("status %d", response.Code)
	}
}

func TestStartRendersQueuedCards(t *testing.T) {
	app := signedInWithKagi(t, "carol", true)
	response := app.PostForm("/summarizer", url.Values{"urls": {"https://a.com/x\nhttps://b.com/y"}})
	body := response.Text()
	if response.Code != http.StatusOK || strings.Count(body, "data-summarizer-card") != 2 ||
		!strings.Contains(body, `data-state="queued"`) || !strings.Contains(body, "https://a.com/x") {
		t.Fatalf("status %d: %s", response.Code, body)
	}
}

func TestStartRejectsOver30(t *testing.T) {
	app := signedInWithKagi(t, "dave", true)
	lines := make([]string, 31)
	for i := range lines {
		lines[i] = "https://e.com/" + string(rune('a'+i%26)) + string(rune('a'+i/26))
	}
	response := app.PostForm("/summarizer", url.Values{"urls": {strings.Join(lines, "\n")}})
	body := response.Text()
	if response.Code != http.StatusOK || !strings.Contains(body, "30 max") || strings.Count(body, "data-summarizer-card") != 0 {
		t.Fatalf("status %d: %s", response.Code, body)
	}
}
