package server

import (
	"context"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/testutil"
)

var stackSecret = []byte("01234567890123456789012345678901")

// stackFixture is the real Handler over a migrated database with three probe
// routes: a page (HTML, so ETag applies), a form target and an API target.
type stackFixture struct {
	handler http.Handler
	reached atomic.Int32
	session *models.Session
	cookies auth.Cookies
}

func newStackFixture(t *testing.T, secure bool) *stackFixture {
	t.Helper()
	database := testutil.SQLite(t)
	cfg := config.Config{Secret: stackSecret, CookieSecure: secure}
	fixture := &stackFixture{cookies: auth.Cookies{Secret: stackSecret, Secure: secure}}
	probe := func(contentType, body string) http.HandlerFunc {
		return func(w http.ResponseWriter, _ *http.Request) {
			fixture.reached.Add(1)
			w.Header().Set("Content-Type", contentType)
			_, _ = io.WriteString(w, body)
		}
	}
	table := []Route{
		{"GET", "/probe", probe("text/html; charset=utf-8", "<p>page</p>")},
		{"POST", "/probe", probe("text/html; charset=utf-8", "<p>posted</p>")},
		{"POST", "/api/probe", probe("application/json", `{"ok":true}`)},
		{"GET", "/health", serveHealth},
	}
	fixture.handler = buildHandler(Options{
		Config: cfg, DB: database, Audit: audit.New(slog.New(slog.DiscardHandler), stackSecret),
	}, table)

	user, err := models.CreateUser(context.Background(), database, "alice", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	fixture.session, err = models.CreateSession(context.Background(), database, models.NewSession{UserID: user.ID})
	if err != nil {
		t.Fatal(err)
	}
	return fixture
}

func (f *stackFixture) serve(r *http.Request) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	f.handler.ServeHTTP(rec, r)
	return rec
}

// signedIn adds the session cookie of the fixture user.
func (f *stackFixture) signedIn(r *http.Request) *http.Request {
	r.AddCookie(f.cookies.Session(f.session.SessionToken))
	return r
}

func (f *stackFixture) csrfToken() string {
	return secret.DeriveCSRF(stackSecret, f.session.SessionToken)
}

func formRequest(path, body string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	return r
}

func TestStackRefusesCrossSiteBeforeAnyLaterLayer(t *testing.T) {
	f := newStackFixture(t, false)
	cases := []struct {
		name    string
		headers map[string]string
	}{
		{"cross-site", map[string]string{"Sec-Fetch-Site": "cross-site"}},
		{"same-site", map[string]string{"Sec-Fetch-Site": "same-site"}},
		{"foreign origin", map[string]string{"Origin": "https://evil.example"}},
		{"null origin", map[string]string{"Origin": "null"}},
		{"other port", map[string]string{"Origin": "http://example.com:9000"}},
	}
	for _, path := range []string{"/probe", "/api/probe"} {
		for _, c := range cases {
			r := f.signedIn(formRequest(path, "_csrf="+url.QueryEscape(f.csrfToken())))
			r.Host = "example.com"
			for name, value := range c.headers {
				r.Header.Set(name, value)
			}
			rec := f.serve(r)
			// A valid session and a valid token cannot rescue a foreign origin:
			// the origin guard runs first, so nothing below it (cookies,
			// session slide, handler) may have acted.
			if rec.Code != http.StatusForbidden || rec.Body.Len() != 0 {
				t.Errorf("%s %s: status %d body %q", c.name, path, rec.Code, rec.Body)
			}
			if len(rec.Header().Values("Set-Cookie")) != 0 {
				t.Errorf("%s %s: a refused request set cookies %v", c.name, path, rec.Header().Values("Set-Cookie"))
			}
			if rec.Header().Get("Content-Security-Policy") == "" {
				t.Errorf("%s %s: refusal lacks the outer security headers", c.name, path)
			}
		}
	}
	if f.reached.Load() != 0 {
		t.Errorf("handler ran %d times for refused requests", f.reached.Load())
	}
}

func TestStackSameOriginMatrixOnAPageWithASession(t *testing.T) {
	f := newStackFixture(t, false)
	good := f.csrfToken()
	cases := []struct {
		name    string
		build   func() *http.Request
		want    int
		reaches bool
	}{
		{"no token", func() *http.Request { return formRequest("/probe", "x=1") }, 403, false},
		{"form token", func() *http.Request { return formRequest("/probe", "_csrf="+url.QueryEscape(good)) }, 200, true},
		{"wrong form token", func() *http.Request { return formRequest("/probe", "_csrf=nope") }, 403, false},
		{"header token", func() *http.Request {
			r := formRequest("/probe", "x=1")
			r.Header.Set("X-CSRF-Token", good)
			return r
		}, 200, true},
		{"wrong header wins over right field", func() *http.Request {
			r := formRequest("/probe", "_csrf="+url.QueryEscape(good))
			r.Header.Set("X-CSRF-Token", "nope")
			return r
		}, 403, false},
		{"api path is guarded too", func() *http.Request { return formRequest("/api/probe", "x=1") }, 403, false},
		{"api path with header", func() *http.Request {
			r := formRequest("/api/probe", "x=1")
			r.Header.Set("X-CSRF-Token", good)
			return r
		}, 200, true},
	}
	for _, c := range cases {
		before := f.reached.Load()
		for _, origin := range []map[string]string{
			nil,
			{"Sec-Fetch-Site": "same-origin"},
			{"Sec-Fetch-Site": "none"},
			{"Origin": "http://example.com"},
		} {
			r := f.signedIn(c.build())
			r.Host = "example.com"
			for name, value := range origin {
				r.Header.Set(name, value)
			}
			if rec := f.serve(r); rec.Code != c.want {
				t.Errorf("%s with %v: status %d, want %d", c.name, origin, rec.Code, c.want)
			}
		}
		if reached := f.reached.Load() != before; reached != c.reaches {
			t.Errorf("%s: handler reached = %v, want %v", c.name, reached, c.reaches)
		}
	}
}

func TestStackPostWithoutASessionIsLeftToTheHandler(t *testing.T) {
	f := newStackFixture(t, false)
	// An API path gets no anonymous session, so the synchronizer guard has no
	// cookie to protect and passes the request on; authentication decides.
	if rec := f.serve(formRequest("/api/probe", "x=1")); rec.Code != 200 {
		t.Errorf("api without session: %d", rec.Code)
	}
	// A page path first mints an anonymous session, which makes the guard
	// require its token: a tokenless form post is refused.
	if rec := f.serve(formRequest("/probe", "x=1")); rec.Code != 403 {
		t.Errorf("page form without token: %d, want 403", rec.Code)
	}
}

func TestStackLayersLeaveTheirMarksOnAnAuthenticatedPage(t *testing.T) {
	f := newStackFixture(t, false)
	r := f.signedIn(httptest.NewRequest(http.MethodGet, "/probe", nil))
	r.Header.Set("Accept-Encoding", "identity")
	rec := f.serve(r)
	header := rec.Header()
	if rec.Code != 200 {
		t.Fatalf("status %d", rec.Code)
	}
	checks := map[string]string{
		"Cache-Control":   "no-store",
		"X-Frame-Options": "DENY",
	}
	for name, want := range checks {
		if got := header.Get(name); got != want {
			t.Errorf("%s = %q, want %q", name, got, want)
		}
	}
	if !strings.Contains(header.Get("Vary"), "Cookie") {
		t.Errorf("Vary = %q, want Cookie", header.Get("Vary"))
	}
	if !strings.HasPrefix(header.Get("ETag"), `W/"`) {
		t.Errorf("ETag = %q, want a weak tag on an HTML 200", header.Get("ETag"))
	}
	if header.Get("Date") == "" || header.Get("Content-Security-Policy") == "" {
		t.Errorf("Date/CSP missing: %v", header)
	}
	var names []string
	for _, cookie := range rec.Result().Cookies() {
		names = append(names, cookie.Name)
	}
	if len(names) != 2 {
		t.Errorf("slid session should reissue the session and CSRF cookies, got %v", names)
	}
}

func TestStackANonMatchingETagIsServedAndAMatchingOneIs304(t *testing.T) {
	f := newStackFixture(t, false)
	first := f.serve(f.signedIn(httptest.NewRequest(http.MethodGet, "/probe", nil)))
	tag := first.Header().Get("ETag")
	if tag == "" {
		t.Fatal("no ETag")
	}
	r := f.signedIn(httptest.NewRequest(http.MethodGet, "/probe", nil))
	r.Header.Set("If-None-Match", tag)
	if rec := f.serve(r); rec.Code != http.StatusNotModified {
		t.Errorf("matching tag: %d", rec.Code)
	}
	r = f.signedIn(httptest.NewRequest(http.MethodGet, "/probe", nil))
	r.Header.Set("If-None-Match", `W/"other"`)
	if rec := f.serve(r); rec.Code != 200 {
		t.Errorf("other tag: %d", rec.Code)
	}
}

func TestStackAnonymousVisitorGetsOnePairOfCookies(t *testing.T) {
	f := newStackFixture(t, false)
	rec := f.serve(httptest.NewRequest(http.MethodGet, "/probe", nil))
	cookies := rec.Result().Cookies()
	counts := map[string]int{}
	for _, cookie := range cookies {
		counts[cookie.Name]++
	}
	if counts[auth.SessionCookieName] != 1 || counts[auth.CSRFCookieName] != 1 || len(cookies) != 2 {
		t.Errorf("cookies = %v", counts)
	}
	// The minted session works on the next request: its token is accepted.
	next := formRequest("/probe", "_csrf=")
	for _, cookie := range cookies {
		next.AddCookie(cookie)
	}
	token := ""
	for _, cookie := range cookies {
		if cookie.Name == auth.CSRFCookieName {
			token = cookie.Value
		}
	}
	next = formRequest("/probe", "_csrf="+url.QueryEscape(token))
	for _, cookie := range cookies {
		next.AddCookie(cookie)
	}
	if rec := f.serve(next); rec.Code != 200 {
		t.Errorf("post with the minted pair: %d", rec.Code)
	}
}

func TestStackSecureDeploymentUsesHostPrefixedCookies(t *testing.T) {
	f := newStackFixture(t, true)
	rec := f.serve(httptest.NewRequest(http.MethodGet, "/probe", nil))
	for _, cookie := range rec.Result().Cookies() {
		if !strings.HasPrefix(cookie.Name, "__Host-") || !cookie.Secure {
			t.Errorf("cookie %s must be __Host- and Secure", cookie.Name)
		}
	}
}

func TestStackPublicAssetsGetNoSessionCookies(t *testing.T) {
	f := newStackFixture(t, false)
	for _, path := range []string{"/health", "/static/app.js", "/favicon.ico"} {
		rec := f.serve(httptest.NewRequest(http.MethodGet, path, nil))
		if len(rec.Header().Values("Set-Cookie")) != 0 {
			t.Errorf("%s set cookies %v", path, rec.Header().Values("Set-Cookie"))
		}
	}
}
