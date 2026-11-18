package server

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/save"
)

func TestOptionalTextCountsAnEmptyOrMissingFieldAsAbsent(t *testing.T) {
	t.Parallel()
	form := url.Values{"empty": {""}, "filled": {"x"}}
	if got := optionalText(form, "missing"); got != nil {
		t.Errorf("missing = %q", *got)
	}
	if got := optionalText(form, "empty"); got != nil {
		t.Errorf("empty = %q", *got)
	}
	if got := optionalText(form, "filled"); got == nil || *got != "x" {
		t.Errorf("filled = %v", got)
	}
}

func TestNextLinkdingMergesTheSubmittedFields(t *testing.T) {
	t.Parallel()
	stored := &save.LinkdingConfig{APIURL: "https://old.example", APIToken: "old-token"}
	cases := []struct {
		name    string
		current *save.LinkdingConfig
		form    url.Values
		clear   bool
		want    *save.LinkdingConfig
	}{
		{"clear wins over fields", stored, url.Values{"api_url": {"https://new.example"}}, true, nil},
		{"empty submission removes", stored, url.Values{"api_url": {""}, "api_token": {""}}, false, nil},
		{"no fields removes", stored, url.Values{}, false, nil},
		{"both fields on nothing", nil, url.Values{"api_url": {"https://new.example"}, "api_token": {"tok"}}, false,
			&save.LinkdingConfig{APIURL: "https://new.example", APIToken: "tok"}},
		{"url only on nothing", nil, url.Values{"api_url": {"https://new.example"}}, false,
			&save.LinkdingConfig{APIURL: "https://new.example"}},
		{"token only on nothing", nil, url.Values{"api_token": {"tok"}}, false,
			&save.LinkdingConfig{APIToken: "tok"}},
		{"url only keeps the stored token", stored, url.Values{"api_url": {"https://new.example"}, "api_token": {""}}, false,
			&save.LinkdingConfig{APIURL: "https://new.example", APIToken: "old-token"}},
		{"token only keeps the stored url", stored, url.Values{"api_url": {""}, "api_token": {"new-token"}}, false,
			&save.LinkdingConfig{APIURL: "https://old.example", APIToken: "new-token"}},
	}
	for _, tc := range cases {
		before := stored.APIURL + stored.APIToken
		got := nextLinkding(tc.current, tc.form, tc.clear)
		switch {
		case tc.want == nil && got != nil:
			t.Errorf("%s: got %+v, want nil", tc.name, *got)
		case tc.want != nil && (got == nil || *got != *tc.want):
			t.Errorf("%s: got %v, want %+v", tc.name, got, *tc.want)
		}
		if stored.APIURL+stored.APIToken != before {
			t.Errorf("%s: the stored config was modified", tc.name)
		}
	}
}

func TestKagiSessionToken(t *testing.T) {
	t.Parallel()
	if got, err := kagiSessionToken("  https://kagi.com/search?token=abc123 \n"); err != nil || got != "abc123" {
		t.Errorf("valid link = %q, %v", got, err)
	}
	for link, want := range map[string]string{
		"https://kagi.com/search":        "No token found in session link",
		"https://kagi.com/search?token=": "No token found in session link",
		"/search?token=abc":              "Invalid session link URL",
		"://bad":                         "Invalid session link URL",
		"":                               "Invalid session link URL",
	} {
		if got, err := kagiSessionToken(link); err == nil || err.Error() != want || got != "" {
			t.Errorf("%q = %q, %v; want error %q", link, got, err, want)
		}
	}
}

func TestNextKagiAppliesASubmission(t *testing.T) {
	t.Parallel()
	english, german := "EN", "DE"
	stored := &save.KagiConfig{SessionToken: "old", Language: &english}
	token := "new"
	cases := []struct {
		name    string
		current *save.KagiConfig
		edit    kagiEdit
		want    *save.KagiConfig
	}{
		{"empty form removes", stored, kagiEdit{}, nil},
		{"token on nothing", nil, kagiEdit{token: &token}, &save.KagiConfig{SessionToken: "new"}},
		{"language on nothing", nil, kagiEdit{hasLanguage: true, language: &german}, &save.KagiConfig{Language: &german}},
		{"token keeps the language", stored, kagiEdit{token: &token}, &save.KagiConfig{SessionToken: "new", Language: &english}},
		{"language keeps the token", stored, kagiEdit{hasLanguage: true, language: &german}, &save.KagiConfig{SessionToken: "old", Language: &german}},
		{"a blank language field clears it", stored, kagiEdit{hasLanguage: true}, &save.KagiConfig{SessionToken: "old"}},
	}
	for _, tc := range cases {
		got := nextKagi(tc.current, tc.edit)
		switch {
		case tc.want == nil && got != nil:
			t.Errorf("%s: got %+v, want nil", tc.name, *got)
		case tc.want != nil && got == nil:
			t.Errorf("%s: got nil, want %+v", tc.name, *tc.want)
		case tc.want != nil && (got.SessionToken != tc.want.SessionToken || !sameText(got.Language, tc.want.Language)):
			t.Errorf("%s: got %+v, want %+v", tc.name, *got, *tc.want)
		}
	}
	if stored.SessionToken != "old" || *stored.Language != "EN" {
		t.Errorf("the stored config was modified: %+v", *stored)
	}
}

func sameText(a, b *string) bool {
	if a == nil || b == nil {
		return a == b
	}
	return *a == *b
}

func TestReadPreferencesNamesTheMissingField(t *testing.T) {
	t.Parallel()
	full := url.Values{"theme": {"dark"}, "entries_per_page": {"20"}, "retention_read_days": {"30"}}
	for _, missing := range []string{"theme", "entries_per_page", "retention_read_days"} {
		form := url.Values{}
		for name, value := range full {
			if name != missing {
				form[name] = value
			}
		}
		w := httptest.NewRecorder()
		if _, ok := readPreferences(w, form); ok || w.Code != http.StatusUnprocessableEntity ||
			!strings.Contains(w.Body.String(), "missing field `"+missing+"`") {
			t.Errorf("without %s: ok=%v %d %s", missing, ok, w.Code, w.Body)
		}
	}
}

func TestReadPreferencesReportsTheFirstFailureNotAMissingField(t *testing.T) {
	t.Parallel()
	form := url.Values{"theme": {"dark"}, "entries_per_page": {"many"}, "retention_read_days": {"30"}}
	w := httptest.NewRecorder()
	if _, ok := readPreferences(w, form); ok || !strings.Contains(w.Body.String(), "entries_per_page: invalid digit") {
		t.Errorf("ok=%v %d %s", ok, w.Code, w.Body)
	}
	if strings.Contains(w.Body.String(), "missing field") {
		t.Errorf("the parse failure was replaced: %s", w.Body)
	}
}

func TestReadPreferencesReadsAValidForm(t *testing.T) {
	t.Parallel()
	form := url.Values{
		"theme": {"dark"}, "entries_per_page": {"20"}, "retention_read_days": {"30"},
		"sidebar_hide_read": {"on"},
	}
	w := httptest.NewRecorder()
	input, ok := readPreferences(w, form)
	if !ok || input.theme == nil || *input.theme != "dark" || input.entriesPerPage != 20 ||
		input.retentionDays != 30 || !input.sidebarHideRead || input.pixelTracking {
		t.Errorf("ok=%v input=%+v", ok, input)
	}
}

func TestLoadServicesForWriteReadsTheStoredServices(t *testing.T) {
	app := defaultITestApp(t)
	user := app.SeedAccount("saver", fixturePassword, models.RoleUser)
	ctx := context.Background()
	want := save.ServicesConfig{Linkding: &save.LinkdingConfig{APIURL: "https://ld.example", APIToken: "tok"}}
	if err := app.App.storeServices(ctx, user.ID, want); err != nil {
		t.Fatal(err)
	}

	got, err := app.App.loadServicesForWrite(ctx, user.ID)
	if err != nil || got.Linkding == nil || *got.Linkding != *want.Linkding {
		t.Errorf("got %+v, %v", got, err)
	}
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err := app.App.loadServicesForWrite(cancelled, user.ID); err == nil {
		t.Error("a failed read was not reported")
	}
}
