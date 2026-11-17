package server

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/testutil"
)

// The goldens are full pages captured from the Rust server (see
// testdata/html/capture.py). Each handler step adds its pages to goldenCovered.

var goldenNow = time.Date(2026, 11, 17, 7, 15, 0, 0, time.UTC)

var goldenSecret = []byte("golden-secret-not-base64-0123456789")

type goldenPage struct {
	Name        string  `json:"name"`
	Method      string  `json:"method"`
	Path        string  `json:"path"`
	User        *string `json:"user"`
	Status      int     `json:"status"`
	ContentType string  `json:"content_type"`
	Location    *string `json:"location"`
	Anonymous   bool    `json:"anonymous"`
	Body        *string `json:"body"`
	File        *string `json:"file"`
}

// goldenCovered selects the pages whose handlers exist.
func goldenCovered(page goldenPage) bool {
	for _, prefix := range []string{"/setup", "/login", "/invite"} {
		if strings.HasPrefix(page.Path, prefix) {
			return true
		}
	}
	return goldenPaths[page.Path]
}

// goldenPaths are the exact request targets whose handlers exist.
var goldenPaths = map[string]bool{
	"/categories":                      true,
	"/api/sidebar":                     true,
	"/api/sidebar/categories/1/feeds":  true,
	"/api/user":                        true,
	"/feeds":                           true,
	"/feeds/1/edit":                    true,
	"/feeds/2/edit":                    true,
	"/feeds/3/edit":                    true,
	"/feeds/import":                    true,
	"/":                                true,
	"/entries":                         true,
	"/entries?fragment=1":              true,
	"/entries/read":                    true,
	"/entries/starred":                 true,
	"/entries/summarized":              true,
	"/entries/today":                   true,
	"/entries/offline":                 true,
	"/entries/1":                       true,
	"/entries?entry=1":                 true,
	"/feeds/1/entries":                 true,
	"/feeds/1/entries?status=read":     true,
	"/feeds/1/entries?status=starred":  true,
	"/categories/1/entries":            true,
	"/categories/1/entries?fragment=1": true,
	"/entries/1/fragment":              true,
	"/entries/3/fragment":              true,
	"/entries/4/fragment":              true,
	"/entries/6/fragment":              true,
	"/entries/7/fragment":              true,
	"/entries/8/fragment":              true,
	"/entries/1/summary/fragment":      true,
	"/entries/5/summary/fragment":      true,
	"/entries/2/summary/fragment":      true,
	"/offline":                         true,
	"/no/such/page":                    true,
	"/api/offline/manifest":            true,
	"/statistics":                      true,
	"/search":                          true,
	"/search?q=alpha":                  true,
	"/search?q=zzzznothing":            true,
	"/search?q=%22unterminated":        true,
	"/search?q=alpha&fragment=1":       true,
	"/search?q=is%3Astarred+feed%3A1":  true,
}

var (
	hex64Pattern = regexp.MustCompile(`[0-9a-f]{64}`)
	// The file size depends on the page layout the engine writes, not on the app.
	databaseSize = regexp.MustCompile(`(stats-card-value">)[0-9.]+ [KMG]?B(</div><div class="stats-card-label">Database Size)`)
	csrfField    = regexp.MustCompile(`name="_csrf" value="([0-9a-f]+)"`)
)

type goldenFixture struct {
	handler  http.Handler
	database *db.DB
	version  string
	expires  string
}

func newGoldenFixture(t *testing.T) *goldenFixture {
	t.Helper()
	t.Setenv("RDRS_FAST_HASH", "1")
	database := testutil.SQLite(t)
	app := NewApp(Options{
		Config: config.Config{Secret: goldenSecret, MultiUser: true}, DB: database, SetupToken: "CCCCC-CCCCC",
		Audit:   audit.New(slog.New(slog.DiscardHandler), goldenSecret),
		Limiter: auth.NewRateLimiter(0, 60),
		// Inside the window the Rust capture ran in, so relative times match.
		Now: func() time.Time { return goldenNow },
	})
	// The next replayed request must see what a detached write changed.
	app.detach = func(task func()) { task() }
	return &goldenFixture{
		handler: app.Handler(), database: database, version: gitVersion(),
		expires: time.Now().UTC().Add(7 * 24 * time.Hour).Format("2006-01-02T15:04:05.000000+00:00"),
	}
}

// seed applies seed.sql; the first captures ran before it, on an empty instance.
func (f *goldenFixture) seed(t *testing.T) {
	t.Helper()
	raw, err := os.ReadFile("../../testdata/html/seed.sql")
	if err != nil {
		t.Fatal(err)
	}
	script := strings.ReplaceAll(string(raw), "@SESSION_EXPIRES@", f.expires)
	script = strings.ReplaceAll(script, "@INVITE_HASH@", secret.InviteHash(goldenSecret, "golden-invite-token"))
	if _, err := f.database.ExecContext(context.Background(), script); err != nil {
		t.Fatal(err)
	}
	// The Rust server's cleanup sweep ran at its start and removed the seeded
	// summaries, which are older than a day.
	if _, err := models.DeleteExpiredSummaries(context.Background(), f.database, 24); err != nil {
		t.Fatal(err)
	}
}

func (f *goldenFixture) serve(r *http.Request) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	f.handler.ServeHTTP(rec, r)
	return rec
}

// anonymousForm mirrors capture.py: the cookie and CSRF token the form page
// itself issues are the ones the POST carries.
func (f *goldenFixture) anonymousForm(t *testing.T, path, body string) (cookie, withCSRF string) {
	t.Helper()
	rec := f.serve(httptest.NewRequest(http.MethodGet, path, nil))
	for _, c := range rec.Result().Cookies() {
		if c.Name == auth.SessionCookieName {
			cookie = c.Value
		}
	}
	match := csrfField.FindStringSubmatch(rec.Body.String())
	if cookie == "" || match == nil {
		t.Fatalf("GET %s issued no session cookie or CSRF field", path)
	}
	return cookie, "_csrf=" + match[1] + "&" + body
}

func (f *goldenFixture) replay(t *testing.T, page goldenPage) *httptest.ResponseRecorder {
	t.Helper()
	body := ""
	if page.Body != nil {
		body = strings.TrimPrefix(*page.Body, "_csrf=@HEX64@&")
	}
	request := httptest.NewRequest(page.Method, page.Path, strings.NewReader(""))
	switch {
	case page.Method == http.MethodPost && page.Anonymous:
		cookie, withCSRF := f.anonymousForm(t, page.Path, body)
		request = httptest.NewRequest(page.Method, page.Path, strings.NewReader(withCSRF))
		request.AddCookie(&http.Cookie{Name: auth.SessionCookieName, Value: cookie})
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	case page.User != nil:
		request.AddCookie(auth.Cookies{Secret: goldenSecret}.Session(*page.User))
	}
	return f.serve(request)
}

func (f *goldenFixture) normalise(page goldenPage, body string) string {
	body = strings.ReplaceAll(body, f.version, "@VERSION@")
	body = strings.ReplaceAll(body, goldenNow.Format("2006-01-02 15:04:05"), "@NOW@")
	if page.Anonymous {
		body = hex64Pattern.ReplaceAllString(body, "@HEX64@")
	}
	for _, form := range []string{f.expires[:19], f.expires[:10] + " " + f.expires[11:19]} {
		body = strings.ReplaceAll(body, form, "@EXPIRES@")
	}
	return body
}

func TestHTMLGoldens(t *testing.T) {
	raw, err := os.ReadFile("../views/testdata/golden/index.json")
	if err != nil {
		t.Fatal(err)
	}
	var index struct {
		Pages []goldenPage `json:"pages"`
	}
	if err := json.Unmarshal(raw, &index); err != nil {
		t.Fatal(err)
	}
	f := newGoldenFixture(t)
	for _, page := range index.Pages {
		if !goldenCovered(page) {
			continue
		}
		if page.Name == "login" {
			f.seed(t)
		}
		t.Run(page.Name, func(t *testing.T) { f.check(t, page) })
	}
}

func (f *goldenFixture) check(t *testing.T, page goldenPage) {
	rec := f.replay(t, page)
	if rec.Code != page.Status {
		t.Fatalf("status = %d, want %d", rec.Code, page.Status)
	}
	if page.Location != nil && rec.Header().Get("Location") != *page.Location {
		t.Errorf("Location = %q, want %q", rec.Header().Get("Location"), *page.Location)
	}
	if page.File == nil {
		return
	}
	want, err := os.ReadFile("../views/testdata/golden/" + *page.File)
	if err != nil {
		t.Fatal(err)
	}
	got, wanted := databaseSize.ReplaceAllString(f.normalise(page, rec.Body.String()), "${1}@SIZE@${2}"), databaseSize.ReplaceAllString(string(want), "${1}@SIZE@${2}")
	if got != wanted {
		t.Errorf("body differs from %s\n got: %.400s\nwant: %.400s", *page.File, firstDifference(got, wanted), firstDifference(wanted, got))
	}
}

// firstDifference returns the text from a little before the first byte where a
// and b differ.
func firstDifference(a, b string) string {
	i := 0
	for i < len(a) && i < len(b) && a[i] == b[i] {
		i++
	}
	return a[max(0, i-80):]
}
