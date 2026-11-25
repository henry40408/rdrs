package server

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/netip"
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

// The goldens are full pages captured from the Rust server. Their files are no
// longer in the tree: goldenPages holds a sha256 digest of each, and git history
// holds the files. Each handler step adds its pages to goldenCovered.

var goldenNow = time.Date(2026, 11, 17, 7, 15, 0, 0, time.UTC)

var goldenSecret = []byte("golden-secret-not-base64-0123456789")

// goldenPage is one replayed request. The zero value of User, Location, Body
// and File means none: no session cookie, no redirect target to check, no form
// body, no golden file.
type goldenPage struct {
	Name      string
	Method    string
	Path      string
	User      string
	Status    int
	Location  string
	Anonymous bool
	// Body is the form without its _csrf field, which anonymousForm adds from the
	// form page itself.
	Body string
	// File is the golden's name under internal/views/testdata/golden (deleted),
	// and Digest the sha256 of its content, see goldenPages.
	File   string
	Digest string
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
	"/summarizer":                      true,
	"/search":                          true,
	"/search?q=alpha":                  true,
	"/search?q=zzzznothing":            true,
	"/search?q=%22unterminated":        true,
	"/search?q=alpha&fragment=1":       true,
	"/search?q=is%3Astarred+feed%3A1":  true,
	"/user-settings":                   true,
	"/settings":                        true,
	"/admin":                           true,
	"/api/passkeys":                    true,
}

var (
	hex64Pattern = regexp.MustCompile(`[0-9a-f]{64}`)
	// The file size depends on the page layout the engine writes, not on the app.
	databaseSize = regexp.MustCompile(`(stats-card-value">)[0-9.]+ [KMG]?B(</div><div class="stats-card-label">Database Size)`)
	// The invite expiry is the seeded time to the minute; the capture left it as is.
	inviteExpiry = regexp.MustCompile(`link expires \d{4}-\d\d-\d\d \d\d:\d\d`)
	lastActive   = regexp.MustCompile(`(<dt>Last active</dt><dd><time datetime=")[^"]*(" data-local-text>)[^<]*`)
	// The capture ran against a scratch directory of the capturing machine.
	capturedDatabase = regexp.MustCompile(`<code>[^<]*golden\.sqlite3</code>`)
	csrfField        = regexp.MustCompile(`name="_csrf" value="([0-9a-f]+)"`)
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
		Config: config.Config{
			Secret: goldenSecret, MultiUser: true, DatabaseURL: "@DB@",
			ServerBind:   netip.MustParseAddrPort("127.0.0.1:18931"),
			WebAuthnRPID: "localhost", WebAuthnRPOrigin: "http://localhost:18931", WebAuthnRPName: "rdrs",
			UserAgent: config.DefaultUserAgent(),
		}, DB: database, SetupToken: "CCCCC-CCCCC",
		Audit:   audit.New(slog.New(slog.DiscardHandler), goldenSecret),
		Limiter: auth.NewRateLimiter(0, 60),
		// Inside the window the Rust capture ran in, so relative times match.
		Now: func() time.Time { return goldenNow },
	})
	// The next replayed request must see what a detached write changed.
	app.detach = func(task func()) { task() }
	return &goldenFixture{
		handler: app.Handler(), database: database, version: gitVersion(),
		expires: time.Now().UTC().Add(7*24*time.Hour).Format("2006-01-02T15:04:05") + ".000000+00:00",
	}
}

// seed applies goldenSeedSQL; the first captures ran before it, on an empty
// instance.
func (f *goldenFixture) seed(t *testing.T) {
	t.Helper()
	script := strings.ReplaceAll(goldenSeedSQL, "@SESSION_EXPIRES@", f.expires)
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

// anonymousForm mirrors the capture: the cookie and CSRF token the form page
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
	request := httptest.NewRequest(page.Method, page.Path, strings.NewReader(""))
	switch {
	case page.Method == http.MethodPost && page.Anonymous:
		cookie, withCSRF := f.anonymousForm(t, page.Path, page.Body)
		request = httptest.NewRequest(page.Method, page.Path, strings.NewReader(withCSRF))
		request.AddCookie(&http.Cookie{Name: auth.SessionCookieName, Value: cookie})
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	case page.User != "":
		request.AddCookie(auth.Cookies{Secret: goldenSecret}.Session(page.User))
	}
	return f.serve(request)
}

func (f *goldenFixture) normalise(page goldenPage, body string) string {
	// Only where the version is shown: the build has none ("unknown"), a word
	// that also appears in page text.
	for _, shown := range []string{"?v=", "<code>", "RDRS/"} {
		body = strings.ReplaceAll(body, shown+f.version, shown+"@VERSION@")
	}
	body = strings.ReplaceAll(body, goldenNow.Format("2006-01-02 15:04:05"), "@NOW@")
	if page.Anonymous {
		body = hex64Pattern.ReplaceAllString(body, "@HEX64@")
	}
	for _, form := range []string{f.expires[:19], f.expires[:10] + " " + f.expires[11:19]} {
		body = strings.ReplaceAll(body, form, "@EXPIRES@")
	}
	return body
}

// scrubVolatile hides what differs between runs on both sides of a comparison.
func scrubVolatile(body string) string {
	body = inviteExpiry.ReplaceAllString(body, "link expires @EXPIRES@")
	body = lastActive.ReplaceAllString(body, "${1}@NOW@${2}@NOW@")
	return capturedDatabase.ReplaceAllString(body, "<code>@DB@</code>")
}

func TestHTMLGoldens(t *testing.T) {
	f := newGoldenFixture(t)
	for _, page := range goldenPages {
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
	if page.Name == "statistics" {
		f.moveRealClockReadsToGoldenDay(t)
	}
	rec := f.replay(t, page)
	if rec.Code != page.Status {
		t.Fatalf("status = %d, want %d", rec.Code, page.Status)
	}
	if page.Location != "" && rec.Header().Get("Location") != page.Location {
		t.Errorf("Location = %q, want %q", rec.Header().Get("Location"), page.Location)
	}
	if page.File == "" {
		return
	}
	got := databaseSize.ReplaceAllString(scrubVolatile(f.normalise(page, rec.Body.String())), "${1}@SIZE@${2}")
	assertPageGolden(t, page, got)
}

// assertPageGolden compares the digest of the replayed page, after the same
// scrubbing the golden's digest had, with the Rust server's. A mismatch writes
// the page to a temporary file; the golden is in git history.
func assertPageGolden(t *testing.T, page goldenPage, got string) {
	t.Helper()
	sum := sha256.Sum256([]byte(got))
	if hex.EncodeToString(sum[:]) == page.Digest {
		return
	}
	file, err := os.CreateTemp("", page.File+".actual.*")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = file.Close() }()
	if _, err := file.WriteString(got); err != nil {
		t.Fatal(err)
	}
	path := "internal/views/testdata/golden/" + page.File
	t.Errorf("%s differs from the Rust golden (%d bytes of scrubbed output written to %s).\n"+
		"The golden is in git history: git show $(git log --diff-filter=D --format=%%h -1 -- %s)^:%s",
		page.Name, len(got), file.Name(), path, path)
}

// moveRealClockReadsToGoldenDay re-dates the reads earlier pages made. The
// database stamps them with its own clock, so they land on the day the test
// runs, while the capture (and goldenNow) sit on goldenNow's day.
func (f *goldenFixture) moveRealClockReadsToGoldenDay(t *testing.T) {
	t.Helper()
	const query = "UPDATE entry SET read_at = $1 || substr(read_at, 11) " +
		"WHERE substr(read_at, 1, 10) = $2 AND substr(read_at, 1, 10) <> $1"
	today := time.Now().UTC().Format("2006-01-02")
	if _, err := f.database.ExecContext(context.Background(), query, goldenNow.Format("2006-01-02"), today); err != nil {
		t.Fatal(err)
	}
}
