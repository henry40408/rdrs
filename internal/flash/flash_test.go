package flash

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func cookieFrom(t *testing.T, rec *httptest.ResponseRecorder, name string) *http.Cookie {
	t.Helper()
	for _, cookie := range rec.Result().Cookies() {
		if cookie.Name == name {
			return cookie
		}
	}
	return nil
}

func roundTrip(t *testing.T, messages ...Message) []Message {
	t.Helper()
	rec := httptest.NewRecorder()
	Set(rec, messages...)
	set := cookieFrom(t, rec, CookieName)
	if set == nil {
		t.Fatal("no flash cookie set")
	}
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(set)
	return Take(httptest.NewRecorder(), r)
}

func TestSetThenTakeRoundTrips(t *testing.T) {
	got := roundTrip(t, NewSuccess(`saved "a, b" {ok}`), NewInviteLink("https://x.test/invite/abc"))
	if len(got) != 2 || got[0].Level != Success || got[0].Text != `saved "a, b" {ok}` {
		t.Fatalf("messages = %+v", got)
	}
	if got[1].Kind != InviteLinkKind || got[1].Level != Success || got[1].Text != "https://x.test/invite/abc" {
		t.Errorf("invite = %+v", got[1])
	}
}

func TestSetCookieIsSafeForEveryClient(t *testing.T) {
	rec := httptest.NewRecorder()
	Set(rec, NewError("naïve; text, with \"quotes\" and 中文"))
	raw := rec.Header().Get("Set-Cookie")
	value := strings.TrimPrefix(strings.SplitN(raw, ";", 2)[0], "flash=")
	if strings.ContainsAny(value, ` ",;\{}[]:/`) {
		t.Errorf("value %q carries an unsafe byte", value)
	}
	for _, want := range []string{"Path=/", "HttpOnly", "SameSite=Lax"} {
		if !strings.Contains(raw, want) {
			t.Errorf("%q missing %s", raw, want)
		}
	}
	if strings.Contains(raw, "Secure") {
		t.Error("the flash cookie is not Secure")
	}
	if got := roundTrip(t, NewError("naïve; text, with \"quotes\" and 中文")); got[0].Text != "naïve; text, with \"quotes\" and 中文" {
		t.Errorf("text = %q", got[0].Text)
	}
}

func TestTakeKeepsTheNewestThree(t *testing.T) {
	var many []Message
	for _, text := range []string{"1", "2", "3", "4", "5"} {
		many = append(many, NewInfo(text))
	}
	got := roundTrip(t, many...)
	if len(got) != 3 || got[0].Text != "3" || got[2].Text != "5" {
		t.Errorf("messages = %+v", got)
	}
}

func TestTakeClearsOnlyWhenTheRequestCarriedACookie(t *testing.T) {
	rec := httptest.NewRecorder()
	if got := Take(rec, httptest.NewRequest(http.MethodGet, "/", nil)); got != nil || rec.Header().Get("Set-Cookie") != "" {
		t.Errorf("no cookie: %v / %q", got, rec.Header().Get("Set-Cookie"))
	}
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(&http.Cookie{Name: CookieName, Value: "garbage"})
	rec = httptest.NewRecorder()
	if got := Take(rec, r); got != nil {
		t.Errorf("garbage decoded to %v", got)
	}
	removal := cookieFrom(t, rec, CookieName)
	// Path=/ is required, or the removal fails on multi-segment routes.
	if removal == nil || removal.MaxAge >= 0 || removal.Path != "/" || removal.Value != "" {
		t.Errorf("removal = %+v", removal)
	}
}

func TestTakeRejectsUnknownLevelsAndKinds(t *testing.T) {
	for _, raw := range []string{
		`[{"level":"fatal","message":"x"}]`,
		`[{"level":"info","message":"x","kind":"weird"}]`,
		`[{"level":"info","message":"x","timestamp":"yesterday"}]`,
		`{"level":"info"}`,
	} {
		r := httptest.NewRequest(http.MethodGet, "/", nil)
		r.AddCookie(&http.Cookie{Name: CookieName, Value: escapeValue(raw)})
		if got := Take(httptest.NewRecorder(), r); got != nil {
			t.Errorf("%s decoded to %v", raw, got)
		}
	}
}

func TestMissingTimestampDefaultsToNow(t *testing.T) {
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(&http.Cookie{Name: CookieName, Value: escapeValue(`[{"level":"info","message":"x"}]`)})
	got := Take(httptest.NewRecorder(), r)
	if len(got) != 1 || time.Since(got[0].Timestamp) > time.Minute {
		t.Errorf("messages = %+v", got)
	}
}

func TestPlainKindIsOmittedFromTheCookie(t *testing.T) {
	encoded, err := encodeMessages([]Message{{Level: Info, Text: "x", Timestamp: time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)}})
	if err != nil {
		t.Fatal(err)
	}
	if want := `[{"level":"info","message":"x","timestamp":"2026-01-02T03:04:05Z"}]`; encoded != want {
		t.Errorf("encoded = %s", encoded)
	}
}

func TestTimestampFormatsLikeChrono(t *testing.T) {
	base := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)
	for _, tc := range []struct {
		nanos int
		want  string
	}{
		{0, "2026-01-02T03:04:05"},
		{120_000_000, "2026-01-02T03:04:05.120"},
		{123_456_000, "2026-01-02T03:04:05.123456"},
		{123_456_789, "2026-01-02T03:04:05.123456789"},
	} {
		m := Message{Timestamp: base.Add(time.Duration(tc.nanos))}
		if got := m.TimestampISO(); got != tc.want+"+00:00" {
			t.Errorf("ISO = %s, want %s+00:00", got, tc.want)
		}
		if got := formatTimestamp(m.Timestamp, "Z"); got != tc.want+"Z" {
			t.Errorf("wire = %s", got)
		}
	}
	if got := (Message{Timestamp: base}).FormattedTime(); got != "03:04:05" {
		t.Errorf("FormattedTime = %s", got)
	}
}

func TestPresentationHelpers(t *testing.T) {
	for _, tc := range []struct {
		message           Message
		class, role, name string
	}{
		{NewSuccess(""), "banner--success", "status", "Success"},
		{NewInfo(""), "banner--info", "status", "Info"},
		{NewWarning(""), "banner--warning", "alert", "Warning"},
		{NewError(""), "banner--error", "alert", "Error"},
	} {
		if tc.message.LevelClass() != tc.class || tc.message.AriaRole() != tc.role || tc.message.LevelName() != tc.name {
			t.Errorf("%s: %s %s %s", tc.name, tc.message.LevelClass(), tc.message.AriaRole(), tc.message.LevelName())
		}
	}
}

func TestRedirectIsASeeOtherWithAnEmptyBody(t *testing.T) {
	rec := httptest.NewRecorder()
	Redirect(rec, "/login", NewWarning("Please log in to continue."))
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" || rec.Body.Len() != 0 {
		t.Errorf("redirect = %d %q %q", rec.Code, rec.Header().Get("Location"), rec.Body.String())
	}
	if cookieFrom(t, rec, CookieName) == nil {
		t.Error("no flash cookie")
	}
}
