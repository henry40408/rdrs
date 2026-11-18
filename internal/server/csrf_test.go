package server

import (
	"bytes"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"testing"
)

// Port of tests/csrf_test.rs: the first-line CSRF guard through the real
// router, the synchronizer-token guard and the anonymous-session cookies.

func postSetup(app *itestApp, headers map[string]string, host string) itestResponse {
	app.t.Helper()
	body := `{"username":"u","password":"` + fixturePassword + `"}`
	if headers == nil {
		headers = map[string]string{}
	}
	headers["Content-Type"] = "application/json"
	return app.Do(itestRequest{Method: http.MethodPost, Path: "/api/setup", Body: body, Headers: headers, Host: host})
}

func TestCrossSitePostIsRejectedBeforeTheHandler(t *testing.T) {
	response := postSetup(defaultITestApp(t), map[string]string{"Sec-Fetch-Site": "cross-site"}, "")
	if response.Code != http.StatusForbidden {
		t.Errorf("status = %d", response.Code)
	}
}

func TestCrossOriginViaOriginHeaderIsRejected(t *testing.T) {
	response := postSetup(defaultITestApp(t), map[string]string{"Origin": "https://evil.example.com"}, "app.example.com")
	if response.Code != http.StatusForbidden {
		t.Errorf("status = %d", response.Code)
	}
}

func TestSameOriginPostReachesTheHandler(t *testing.T) {
	response := postSetup(defaultITestApp(t), map[string]string{"Sec-Fetch-Site": "same-origin"}, "")
	if response.Code != http.StatusCreated {
		t.Errorf("status = %d", response.Code)
	}
}

func TestSafeGetIsNeverBlockedCrossSite(t *testing.T) {
	response := defaultITestApp(t).Do(itestRequest{Path: "/login", Headers: map[string]string{"Sec-Fetch-Site": "cross-site"}})
	if response.Code != http.StatusOK {
		t.Errorf("status = %d", response.Code)
	}
}

func TestOriginFallbackRejectsAnAuthorityThatDiffersFromHost(t *testing.T) {
	cases := []struct{ origin, host, why string }{
		{"http://app.example.com:9000", "app.example.com:8080", "another port on the same host is another origin"},
		{"https://app.example.com:8443", "app.example.com", "a proxy forwarding Host without the port looks the same"},
		{"https://other.example.com", "app.example.com", "a sibling subdomain is another origin"},
		{"null", "app.example.com", "an opaque origin is never legitimate for a mutation"},
	}
	for _, c := range cases {
		response := postSetup(defaultITestApp(t), map[string]string{"Origin": c.origin}, c.host)
		if response.Code != http.StatusForbidden {
			t.Errorf("Origin %s / Host %s = %d, must be rejected: %s", c.origin, c.host, response.Code, c.why)
		}
	}
}

func TestOriginFallbackPassesAnAuthorityThatMatchesHost(t *testing.T) {
	cases := []struct{ origin, host string }{
		{"https://app.example.com:8443", "app.example.com:8443"},
		{"https://app.example.com", "app.example.com"},
	}
	for _, c := range cases {
		response := postSetup(defaultITestApp(t), map[string]string{"Origin": c.origin}, c.host)
		if response.Code != http.StatusCreated {
			t.Errorf("Origin %s / Host %s = %d, must reach the handler", c.origin, c.host, response.Code)
		}
	}
}

func TestNonBrowserClientWithoutHeadersReachesTheHandler(t *testing.T) {
	response := postSetup(defaultITestApp(t), nil, "")
	if response.Code != http.StatusCreated {
		t.Errorf("status = %d", response.Code)
	}
}

func setCookieHeaders(response itestResponse) []string {
	return response.Result().Header.Values("Set-Cookie")
}

func TestLoggedOutPageRequestEmitsExactlyOneSetCookiePerName(t *testing.T) {
	response := defaultITestApp(t).Get("/login")
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d", response.Code)
	}
	all := setCookieHeaders(response)
	for _, name := range []string{"session_token", "csrf_token"} {
		count := 0
		for _, header := range all {
			if strings.HasPrefix(header, name+"=") {
				count++
			}
		}
		if count != 1 {
			t.Errorf("Set-Cookie for %s appears %d times: %v", name, count, all)
		}
	}
}

func secureITestApp(t *testing.T) *itestApp {
	config := itestConfig()
	config.CookieSecure = true
	return newITestApp(t, config)
}

func TestSecureAnonymousSessionRoundTripsAndDoesNotRemintCsrfCookie(t *testing.T) {
	app := secureITestApp(t)
	first := app.Get("/login")
	session, csrf := first.Cookie("__Host-session_token"), first.Cookie("__Host-csrf_token")
	if session == nil || csrf == nil {
		t.Fatalf("the __Host- prefixed pair must be minted: %v", setCookieHeaders(first))
	}
	if first.Cookie("session_token") != nil || first.Cookie("csrf_token") != nil {
		t.Error("unprefixed cookies must not be minted when cookie_secure is true")
	}

	register := postSetup(app, map[string]string{"Sec-Fetch-Site": "same-origin", "X-CSRF-Token": csrf.Value}, "")
	if register.Code != http.StatusCreated {
		t.Fatalf("register = %d", register.Code)
	}
	second := app.Get("/login")
	if second.Code != http.StatusOK {
		t.Fatalf("second = %d", second.Code)
	}
	if reissued := second.Cookie("__Host-session_token"); reissued != nil && reissued.Value != session.Value {
		t.Error("the session token changed across requests")
	}
	if reissued := second.Cookie("__Host-csrf_token"); reissued != nil && reissued.Value != csrf.Value {
		t.Error("the CSRF token changed across requests")
	}
}

func TestAStaleCsrfCookieIsRemintedAndUnblocksTheNextPost(t *testing.T) {
	app := defaultITestApp(t)
	first := app.Get("/login")
	session, expected := first.Cookie("session_token"), first.Cookie("csrf_token")
	if session == nil || expected == nil {
		t.Fatal("anonymous session must mint both cookies")
	}
	cookies := "session_token=" + session.Value + "; csrf_token=from-an-earlier-generation"
	second := app.Do(itestRequest{Path: "/login", NoCookies: true, Headers: map[string]string{"Cookie": cookies}})
	healed := second.Cookie("csrf_token")
	if second.Code != http.StatusOK || healed == nil {
		t.Fatalf("a mismatching CSRF cookie must be replaced: %d %v", second.Code, setCookieHeaders(second))
	}
	if healed.Value != expected.Value {
		t.Error("the re-minted cookie must carry the token derived from this session")
	}
	register := app.Do(itestRequest{
		Method: http.MethodPost, Path: "/api/setup", NoCookies: true, Body: `{"username":"u","password":"` + fixturePassword + `"}`,
		Headers: map[string]string{
			"Sec-Fetch-Site": "same-origin", "Content-Type": "application/json",
			"Cookie": "session_token=" + session.Value, "X-CSRF-Token": healed.Value,
		},
	})
	if register.Code != http.StatusCreated {
		t.Errorf("register = %d", register.Code)
	}
}

func TestALeftoverUnprefixedCsrfCookieIsEvictedOnASecureDeployment(t *testing.T) {
	app := secureITestApp(t)
	first := app.Get("/login")
	session, csrf := first.Cookie("__Host-session_token"), first.Cookie("__Host-csrf_token")
	if session == nil || csrf == nil {
		t.Fatal("a Secure deployment mints the __Host- pair")
	}
	cookies := "__Host-session_token=" + session.Value + "; __Host-csrf_token=" + csrf.Value + "; csrf_token=left-over-generation"
	second := app.Do(itestRequest{Path: "/login", NoCookies: true, Headers: map[string]string{"Cookie": cookies}})
	if second.Code != http.StatusOK {
		t.Fatalf("status = %d", second.Code)
	}
	evicted := false
	for _, header := range setCookieHeaders(second) {
		evicted = evicted || (strings.HasPrefix(header, "csrf_token=") && strings.Contains(header, "Max-Age=0"))
	}
	if !evicted {
		t.Errorf("the stale unprefixed cookie must be expired: %v", setCookieHeaders(second))
	}
	live := second.Cookie("__Host-csrf_token")
	if live == nil || live.Value != csrf.Value {
		t.Error("the live CSRF cookie must be reissued unchanged alongside the removal")
	}
}

// anonymousPair is what a browser holds after one page load.
func anonymousPair(t *testing.T, app *itestApp) (session, csrf string) {
	t.Helper()
	response := app.Get("/login")
	sessionCookie, csrfCookie := response.Cookie("session_token"), response.Cookie("csrf_token")
	if sessionCookie == nil || csrfCookie == nil {
		t.Fatal("anonymous session must mint both cookies")
	}
	return sessionCookie.Value, csrfCookie.Value
}

func postWithSession(app *itestApp, session, csrf string) itestResponse {
	headers := map[string]string{"Sec-Fetch-Site": "same-origin", "Cookie": "session_token=" + session}
	if csrf != "" {
		headers["X-CSRF-Token"] = csrf
	}
	headers["Content-Type"] = "application/json"
	return app.Do(itestRequest{
		Method: http.MethodPost, Path: "/api/setup", NoCookies: true, Headers: headers,
		Body: `{"username":"u","password":"` + fixturePassword + `"}`,
	})
}

func TestAnXCsrfTokenHeaderThatDoesNotDeriveFromTheSessionIsRejected(t *testing.T) {
	app := defaultITestApp(t)
	session, csrf := anonymousPair(t, app)
	if response := postWithSession(app, session, "not-this-session's-token"); response.Code != http.StatusForbidden {
		t.Errorf("wrong token = %d", response.Code)
	}
	if response := postWithSession(app, session, csrf); response.Code != http.StatusCreated {
		t.Errorf("own token = %d", response.Code)
	}
}

func formPost(app *itestApp, session, body string) itestResponse {
	return app.Do(itestRequest{
		Method: http.MethodPost, Path: "/feeds/1/entries/mark-read", NoCookies: true, Body: body,
		Headers: map[string]string{
			"Sec-Fetch-Site": "same-origin", "Content-Type": "application/x-www-form-urlencoded",
			"Cookie": "session_token=" + session,
		},
	})
}

func TestAFormPostIsGatedOnItsCsrfField(t *testing.T) {
	app := defaultITestApp(t)
	session, csrf := anonymousPair(t, app)
	if response := formPost(app, session, "scope=all"); response.Code != http.StatusForbidden {
		t.Errorf("missing _csrf = %d", response.Code)
	}
	body := url.Values{"scope": {"all"}, "_csrf": {csrf}}.Encode()
	if response := formPost(app, session, body); response.Code == http.StatusForbidden {
		t.Error("a form carrying its session's own token must clear the guard")
	}
}

func TestABodyOverTheBufferingLimitIsRejected(t *testing.T) {
	app := defaultITestApp(t)
	session, _ := anonymousPair(t, app)
	response := formPost(app, session, strings.Repeat("x", (1<<20)+1))
	if response.Code != http.StatusForbidden {
		t.Errorf("status = %d", response.Code)
	}
}

type lockedBuffer struct {
	mu     sync.Mutex
	buffer bytes.Buffer
}

func (l *lockedBuffer) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.buffer.Write(p)
}

func (l *lockedBuffer) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.buffer.String()
}

// The rejection must name the check and identify the session by its salted
// audit id, never by the credential itself.
func TestATokenMismatchIsLoggedWithoutLeakingTheSessionCookie(t *testing.T) {
	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelWarn})))
	t.Cleanup(func() { slog.SetDefault(previous) })

	app := defaultITestApp(t)
	session, _ := anonymousPair(t, app)
	if response := postWithSession(app, session, "not-this-session's-token"); response.Code != http.StatusForbidden {
		t.Fatalf("status = %d", response.Code)
	}
	logged := logs.String()
	for _, want := range []string{"csrf.mismatch", "session="} {
		if !strings.Contains(logged, want) {
			t.Errorf("the rejection log lacks %q: %s", want, logged)
		}
	}
	if strings.Contains(logged, session) {
		t.Errorf("the session cookie reached the log: %s", logged)
	}

	crossSite := postSetup(app, map[string]string{"Sec-Fetch-Site": "cross-site"}, "")
	if crossSite.Code != http.StatusForbidden {
		t.Fatalf("cross-site = %d", crossSite.Code)
	}
	logged = logs.String()
	if !strings.Contains(logged, "csrf.cross_site") || !strings.Contains(logged, "check=sec_fetch_site") || !strings.Contains(logged, "path=/api/setup") {
		t.Errorf("the cross-site rejection log is incomplete: %s", logged)
	}

	fallback := postSetup(app, map[string]string{"Origin": "https://app.example.com:8443"}, "app.example.com")
	if fallback.Code != http.StatusForbidden {
		t.Fatalf("fallback = %d", fallback.Code)
	}
	if !strings.Contains(logs.String(), "check=origin_fallback") {
		t.Error("an Origin-fallback rejection must be told apart from a Sec-Fetch-Site one")
	}
}
