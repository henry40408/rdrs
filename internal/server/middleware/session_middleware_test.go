package middleware

import (
	"bytes"
	"context"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/testutil"
)

var rootSecret = []byte("01234567890123456789012345678901")

func okHandler(w http.ResponseWriter, _ *http.Request) {
	_, _ = io.WriteString(w, "ok")
}

func serveReq(handler http.Handler, r *http.Request) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, r)
	return rec
}

func signedSession(token string) *http.Cookie {
	return &http.Cookie{Name: auth.SessionCookieName, Value: secret.SignSession(rootSecret, token)}
}

func setCookieLines(rec *httptest.ResponseRecorder) []string {
	return rec.Header().Values("Set-Cookie")
}

func setCookieNamed(rec *httptest.ResponseRecorder, name string) *http.Cookie {
	for _, cookie := range rec.Result().Cookies() {
		if cookie.Name == name {
			return cookie
		}
	}
	return nil
}

// --- flash signing ---

func flashEcho(w http.ResponseWriter, r *http.Request) {
	messages := flash.Take(w, r)
	texts := make([]string, len(messages))
	for i, m := range messages {
		texts[i] = m.Text
	}
	_, _ = io.WriteString(w, strings.Join(texts, "|"))
}

func TestFlashCookieIsSignedOutAndVerifiedIn(t *testing.T) {
	sign := SignFlashCookies(rootSecret)
	setter := sign(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		flash.Redirect(w, "/", flash.NewSuccess("Saved"))
	}))
	rec := serveReq(setter, httptest.NewRequest(http.MethodPost, "/", nil))
	out := setCookieNamed(rec, flash.CookieName)
	if out == nil || strings.Count(out.Value, ".") != 1 {
		t.Fatalf("outgoing flash %+v is not payload.signature", out)
	}

	reader := sign(http.HandlerFunc(flashEcho))
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(out)
	if got := serveReq(reader, r).Body.String(); got != "Saved" {
		t.Errorf("signed flash read back as %q", got)
	}
}

func TestForgedFlashCookiesAreDropped(t *testing.T) {
	reader := SignFlashCookies(rootSecret)(http.HandlerFunc(flashEcho))
	signed := signSetCookieValue(t, `[{"level":"info","message":"real"}]`)
	tampered := strings.Replace(signed, ".", "x.", 1)
	otherKey := strings.Repeat("z", 32)
	forgedWithOtherKey := flashSignature([]byte(otherKey), base64url.EncodeToString([]byte("[]")))
	for name, value := range map[string]string{
		"raw json":            `[{"level":"info","message":"spoof"}]`,
		"tampered payload":    tampered,
		"wrong key signature": base64url.EncodeToString([]byte("[]")) + "." + forgedWithOtherKey,
		"no separator":        "abc",
	} {
		r := httptest.NewRequest(http.MethodGet, "/", nil)
		r.Header.Set("Cookie", "keep=1; flash="+value+"; other=2")
		rec := serveReq(reader, r)
		if rec.Body.Len() != 0 {
			t.Errorf("%s: spoofed flash shown: %q", name, rec.Body.String())
		}
	}
}

func signSetCookieValue(t *testing.T, payload string) string {
	t.Helper()
	line, ok := signSetCookie("flash="+payload+"; Path=/", rootSecret)
	if !ok {
		t.Fatal("not signed")
	}
	return strings.SplitN(strings.TrimPrefix(line, "flash="), ";", 2)[0]
}

func TestFlashVerificationKeepsOtherCookiesAndHeaders(t *testing.T) {
	var seen []string
	handler := SignFlashCookies(rootSecret)(http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) {
		seen = r.Header.Values("Cookie")
	}))
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.Header.Add("Cookie", "a=1; flash=forged")
	r.Header.Add("Cookie", "flash=forged2")
	r.Header.Add("Cookie", "b=2")
	serveReq(handler, r)
	// The header holding only a rejected flash is dropped, not sent empty.
	if len(seen) != 2 || seen[0] != "a=1" || seen[1] != "b=2" {
		t.Errorf("Cookie headers = %q", seen)
	}
}

func TestFlashRemovalCookieStaysUnsigned(t *testing.T) {
	handler := SignFlashCookies(rootSecret)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.SetCookie(w, &http.Cookie{Name: "flash", Path: "/", MaxAge: -1})
		http.SetCookie(w, &http.Cookie{Name: "other", Value: "v", Path: "/"})
	}))
	rec := serveReq(handler, httptest.NewRequest(http.MethodGet, "/", nil))
	lines := setCookieLines(rec)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "flash=;") || !strings.HasPrefix(lines[1], "other=v;") {
		t.Errorf("Set-Cookie = %q", lines)
	}
}

// --- CSRF guard ---

func csrfGuarded() http.Handler {
	return CSRFGuard(auth.Cookies{Secret: rootSecret})(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		_, _ = w.Write(body)
	}))
}

func postForm(path, body string, header map[string]string) *http.Request {
	r := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	for name, value := range header {
		r.Header.Set(name, value)
	}
	r.AddCookie(signedSession("tok"))
	return r
}

func TestCSRFGuardMatrix(t *testing.T) {
	good := secret.DeriveCSRF(rootSecret, "tok")
	wrong := secret.DeriveCSRF(rootSecret, "other")
	for _, tc := range []struct {
		name string
		req  *http.Request
		want int
	}{
		{"get passes", func() *http.Request {
			r := httptest.NewRequest(http.MethodGet, "/", nil)
			r.AddCookie(signedSession("tok"))
			return r
		}(), http.StatusOK},
		{"trace passes", func() *http.Request {
			r := httptest.NewRequest(http.MethodTrace, "/", nil)
			r.AddCookie(signedSession("tok"))
			return r
		}(), http.StatusOK},
		{"post without a session cookie passes", httptest.NewRequest(http.MethodPost, "/", nil), http.StatusOK},
		{"post with a forged session cookie passes as anonymous", func() *http.Request {
			r := httptest.NewRequest(http.MethodPost, "/", nil)
			r.AddCookie(&http.Cookie{Name: auth.SessionCookieName, Value: "tok.00"})
			return r
		}(), http.StatusOK},
		{"post without a token", postForm("/x", "a=1", nil), http.StatusForbidden},
		{"header token", postForm("/x", "a=1", map[string]string{"X-CSRF-Token": good}), http.StatusOK},
		{"wrong header token", postForm("/x", "a=1", map[string]string{"X-CSRF-Token": wrong}), http.StatusForbidden},
		{"a header never falls back to the body", postForm("/x", "_csrf="+good, map[string]string{"X-CSRF-Token": wrong}), http.StatusForbidden},
		{"empty header never falls back to the body", postForm("/x", "_csrf="+good, map[string]string{"X-CSRF-Token": ""}), http.StatusForbidden},
		{"non-hex header token", postForm("/x", "", map[string]string{"X-CSRF-Token": "zz"}), http.StatusForbidden},
		{"form field", postForm("/x", "a=1&_csrf="+good, nil), http.StatusOK},
		{"wrong form field", postForm("/x", "_csrf="+wrong, nil), http.StatusForbidden},
		{"first of repeated form fields counts", postForm("/x", "_csrf="+wrong+"&_csrf="+good, nil), http.StatusForbidden},
		{"multipart passes through on the OPML import", func() *http.Request {
			r := postForm("/feeds/import", "x", nil)
			r.Header.Set("Content-Type", "multipart/form-data; boundary=b")
			return r
		}(), http.StatusOK},
		{"multipart is checked everywhere else", func() *http.Request {
			r := postForm("/api/feeds", "x", nil)
			r.Header.Set("Content-Type", "multipart/form-data; boundary=b")
			return r
		}(), http.StatusForbidden},
		{"ClientLogin is exempt", postForm("/accounts/ClientLogin", "a=1", nil), http.StatusOK},
		{"ClientLogin under the greader prefix is exempt", postForm("/api/greader.php/accounts/ClientLogin", "a=1", nil), http.StatusOK},
		{"a longer ClientLogin lookalike is not exempt", postForm("/accounts/ClientLogin/x", "a=1", nil), http.StatusForbidden},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := serveReq(csrfGuarded(), tc.req).Code; got != tc.want {
				t.Errorf("status = %d, want %d", got, tc.want)
			}
		})
	}
}

func TestCSRFGuardRebuildsTheBody(t *testing.T) {
	body := "name=x&_csrf=" + secret.DeriveCSRF(rootSecret, "tok")
	rec := serveReq(csrfGuarded(), postForm("/x", body, nil))
	if rec.Code != http.StatusOK || rec.Body.String() != body {
		t.Errorf("handler saw %q (status %d)", rec.Body.String(), rec.Code)
	}
}

func TestCSRFGuardBodyLimitIsInclusive(t *testing.T) {
	token := secret.DeriveCSRF(rootSecret, "tok")
	prefix := "_csrf=" + token + "&pad="
	atLimit := prefix + strings.Repeat("a", csrfMaxBodyBytes-len(prefix))
	if got := serveReq(csrfGuarded(), postForm("/x", atLimit, nil)).Code; got != http.StatusOK {
		t.Errorf("a body of exactly the limit = %d, want 200", got)
	}
	if got := serveReq(csrfGuarded(), postForm("/x", atLimit+"a", nil)).Code; got != http.StatusForbidden {
		t.Errorf("a body one byte over the limit = %d, want 403", got)
	}
}

func TestCSRFGuardRejectionIsBodylessAndLogged(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	rec := serveReq(csrfGuarded(), postForm("/x", "a=1", nil))
	if rec.Code != http.StatusForbidden || rec.Body.Len() != 0 {
		t.Errorf("rejection = %d %q", rec.Code, rec.Body.String())
	}
	line := logs.String()
	if !strings.Contains(line, "event=csrf.mismatch") || strings.Contains(line, "tok") && strings.Contains(line, secret.SignSession(rootSecret, "tok")) {
		t.Errorf("log = %s", line)
	}
	if !strings.Contains(line, secret.AuditID(rootSecret, "tok")) {
		t.Errorf("session must be identified by its audit id: %s", line)
	}
}

// --- anonymous session ---

func anonymousChain(cookies auth.Cookies, inner http.HandlerFunc) http.Handler {
	return AnonymousSession(cookies)(inner)
}

func TestAnonymousSessionMintsCookiesForAVisitor(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret}
	var seenSession, seenCSRF string
	handler := anonymousChain(cookies, func(w http.ResponseWriter, r *http.Request) {
		seenSession = cookies.SessionToken(r)
		seenCSRF = cookies.CSRFToken(r)
		okHandler(w, r)
	})
	rec := serveReq(handler, httptest.NewRequest(http.MethodGet, "/login", nil))
	session, csrf := setCookieNamed(rec, auth.SessionCookieName), setCookieNamed(rec, auth.CSRFCookieName)
	if session == nil || csrf == nil {
		t.Fatalf("cookies = %q", setCookieLines(rec))
	}
	token, ok := secret.VerifySession(rootSecret, session.Value)
	if !ok || csrf.Value != secret.DeriveCSRF(rootSecret, token) {
		t.Error("cookies do not belong to one token")
	}
	if seenSession != token || seenCSRF != csrf.Value {
		t.Error("the handler must see the cookies on the same round trip")
	}
}

func TestAnonymousSessionSkipsMachineAndCacheablePaths(t *testing.T) {
	handler := anonymousChain(auth.Cookies{Secret: rootSecret}, okHandler)
	for _, path := range []string{"/api/x", "/reader/y", "/accounts/ClientLogin", "/static/a.css", "/favicon.ico", "/health", "/sw.js", "/offline", "/p/1-2-abc.gif"} {
		if lines := setCookieLines(serveReq(handler, httptest.NewRequest(http.MethodGet, path, nil))); len(lines) != 0 {
			t.Errorf("%s got cookies %q", path, lines)
		}
	}
}

func TestAnonymousSessionLeavesAHealthyPairAlone(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret}
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(signedSession("tok"))
	r.AddCookie(cookies.CSRF("tok"))
	if lines := setCookieLines(serveReq(anonymousChain(cookies, okHandler), r)); len(lines) != 0 {
		t.Errorf("healthy pair reissued: %q", lines)
	}
}

func TestAnonymousSessionHealsAStaleCSRFCookie(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret}
	for name, extra := range map[string]*http.Cookie{
		"missing":   nil,
		"wrong one": {Name: auth.CSRFCookieName, Value: secret.DeriveCSRF(rootSecret, "someone-else")},
	} {
		var seen string
		handler := anonymousChain(cookies, func(w http.ResponseWriter, r *http.Request) {
			seen = cookies.CSRFToken(r)
			cookie, _ := r.Cookie(auth.CSRFCookieName)
			if cookie == nil || cookie.Value != seen {
				t.Errorf("%s: request carries %v", name, cookie)
			}
		})
		r := httptest.NewRequest(http.MethodPost, "/logout", nil)
		r.AddCookie(signedSession("tok"))
		if extra != nil {
			r.AddCookie(extra)
		}
		rec := serveReq(handler, r)
		csrf := setCookieNamed(rec, auth.CSRFCookieName)
		if csrf == nil || !secret.VerifyCSRF(rootSecret, "tok", csrf.Value) {
			t.Errorf("%s: not healed: %q", name, setCookieLines(rec))
		}
		if setCookieNamed(rec, auth.SessionCookieName) != nil {
			t.Errorf("%s: the session cookie must be kept, not reissued", name)
		}
	}
}

func TestAnonymousSessionEvictsTheLeftoverCSRFName(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret}
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.AddCookie(signedSession("tok"))
	r.AddCookie(cookies.CSRF("tok"))
	r.AddCookie(&http.Cookie{Name: auth.CSRFCookieNameHost, Value: "old"})
	rec := serveReq(anonymousChain(cookies, okHandler), r)

	lines := setCookieLines(rec)
	if len(lines) != 2 {
		t.Fatalf("Set-Cookie = %q", lines)
	}
	// Reissued alongside the removal, or the slide layer would see a CSRF name
	// and skip its own reissue.
	if !strings.HasPrefix(lines[0], "csrf_token="+secret.DeriveCSRF(rootSecret, "tok")) {
		t.Errorf("first line = %s", lines[0])
	}
	if !strings.HasPrefix(lines[1], "__Host-csrf_token=;") || !strings.Contains(lines[1], "Secure") || !strings.Contains(lines[1], "Max-Age=0") {
		t.Errorf("removal = %s", lines[1])
	}
}

func TestAnonymousSessionUsesHostNamesWhenSecure(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret, Secure: true}
	rec := serveReq(anonymousChain(cookies, okHandler), httptest.NewRequest(http.MethodGet, "/", nil))
	if setCookieNamed(rec, auth.SessionCookieNameHost) == nil || setCookieNamed(rec, auth.CSRFCookieNameHost) == nil {
		t.Errorf("cookies = %q", setCookieLines(rec))
	}
}

// --- no-store ---

func TestNoStoreForAuthenticated(t *testing.T) {
	withCookie := func(name string) *http.Request {
		r := httptest.NewRequest(http.MethodGet, "/", nil)
		r.AddCookie(&http.Cookie{Name: name, Value: "x"})
		return r
	}
	handler := func(set func(http.Header)) http.Handler {
		return NoStoreForAuthenticated(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			set(w.Header())
			_, _ = io.WriteString(w, "ok")
		}))
	}
	nothing := func(http.Header) {}

	rec := serveReq(handler(nothing), withCookie(auth.SessionCookieName))
	if rec.Header().Get("Cache-Control") != "no-store" || rec.Header().Get("Vary") != "Cookie" {
		t.Errorf("headers = %v", rec.Header())
	}
	if rec := serveReq(handler(nothing), withCookie(auth.SessionCookieNameHost)); rec.Header().Get("Cache-Control") != "no-store" {
		t.Error("the __Host- cookie must count")
	}
	if rec := serveReq(handler(nothing), withCookie("unrelated")); rec.Header().Get("Cache-Control") != "" || rec.Header().Get("Vary") != "" {
		t.Errorf("no session cookie must stay cacheable: %v", rec.Header())
	}
	keep := serveReq(handler(func(h http.Header) { h.Set("Cache-Control", "public, max-age=86400") }), withCookie(auth.SessionCookieName))
	if keep.Header().Get("Cache-Control") != "public, max-age=86400" || keep.Header().Get("Vary") != "" {
		t.Errorf("a deliberate Cache-Control must survive untouched: %v", keep.Header())
	}
}

func TestMergedVary(t *testing.T) {
	for _, tc := range []struct {
		existing []string
		want     string
		changed  bool
	}{
		{nil, "Cookie", true},
		{[]string{"Accept-Encoding"}, "Accept-Encoding, Cookie", true},
		{[]string{"Accept-Encoding", "Accept-Language"}, "Accept-Encoding, Accept-Language, Cookie", true},
		{[]string{"Accept-Encoding, Cookie"}, "", false},
		{[]string{"cookie"}, "", false},
		{[]string{"*"}, "", false},
	} {
		got, changed := mergedVary(tc.existing)
		if got != tc.want || changed != tc.changed {
			t.Errorf("mergedVary(%q) = %q, %v", tc.existing, got, changed)
		}
	}
}

// --- slide session ---

type sessionFixture struct {
	database      *db.DB
	authenticator *auth.Authenticator
}

func newSessionFixture(t *testing.T) sessionFixture {
	t.Helper()
	database := testutil.SQLite(t)
	logger := slog.New(slog.DiscardHandler)
	return sessionFixture{database, &auth.Authenticator{
		DB: database, Config: config.Config{Secret: rootSecret}, Audit: audit.New(logger, rootSecret),
	}}
}

func (f sessionFixture) login(t *testing.T) (*models.User, *models.Session) {
	t.Helper()
	user, err := models.CreateUser(context.Background(), f.database, "alice", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	session, err := models.CreateSession(context.Background(), f.database, models.NewSession{UserID: user.ID})
	if err != nil {
		t.Fatal(err)
	}
	return user, session
}

func TestSlideSessionReissuesBothCookies(t *testing.T) {
	f := newSessionFixture(t)
	_, session := f.login(t)
	handler := SlideSession(f.authenticator)(http.HandlerFunc(okHandler))
	r := httptest.NewRequest(http.MethodGet, "/feeds", nil)
	r.AddCookie(signedSession(session.SessionToken))
	rec := serveReq(handler, r)

	got := setCookieNamed(rec, auth.SessionCookieName)
	if got == nil || got.MaxAge != 7*24*3600 || !got.HttpOnly {
		t.Fatalf("session cookie = %+v", got)
	}
	if token, _ := secret.VerifySession(rootSecret, got.Value); token != session.SessionToken {
		t.Error("no rotation was requested, the token must stay")
	}
	if csrf := setCookieNamed(rec, auth.CSRFCookieName); csrf == nil || csrf.HttpOnly {
		t.Errorf("csrf cookie = %+v", csrf)
	}
}

func TestSlideSessionSkipsWithoutASessionOrOnCacheablePaths(t *testing.T) {
	f := newSessionFixture(t)
	_, session := f.login(t)
	handler := SlideSession(f.authenticator)(http.HandlerFunc(okHandler))
	if lines := setCookieLines(serveReq(handler, httptest.NewRequest(http.MethodGet, "/feeds", nil))); len(lines) != 0 {
		t.Errorf("anonymous request got %q", lines)
	}
	for _, path := range []string{"/static/a.js", "/favicon.ico", "/health", "/sw.js", "/offline"} {
		r := httptest.NewRequest(http.MethodGet, path, nil)
		r.AddCookie(signedSession(session.SessionToken))
		if lines := setCookieLines(serveReq(handler, r)); len(lines) != 0 {
			t.Errorf("%s got %q", path, lines)
		}
	}
}

func TestSlideSessionNeverAttachesCookiesToPubliclyCacheableResponses(t *testing.T) {
	f := newSessionFixture(t)
	_, session := f.login(t)
	run := func(cacheControl string) bool {
		handler := SlideSession(f.authenticator)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if cacheControl != "" {
				w.Header().Set("Cache-Control", cacheControl)
			}
			auth.RequestRotation(r.Context())
			okHandler(w, r)
		}))
		r := httptest.NewRequest(http.MethodGet, "/feeds", nil)
		r.AddCookie(signedSession(session.SessionToken))
		return len(setCookieLines(serveReq(handler, r))) > 0
	}
	for _, cacheControl := range []string{"public, max-age=60", "max-age=600"} {
		if run(cacheControl) {
			t.Errorf("Cache-Control %q: cookies attached", cacheControl)
		}
	}
	// A cacheable response cannot carry the new token, so no rotation happens.
	found, _ := models.FindSessionByToken(context.Background(), f.database, session.SessionToken)
	if found == nil || found.SessionToken != session.SessionToken {
		t.Error("rotation happened although the response could not carry it")
	}
	for _, cacheControl := range []string{"no-store", "private, max-age=0", "No-Store", ""} {
		if !run(cacheControl) {
			t.Errorf("Cache-Control %q: no cookies", cacheControl)
		}
	}
}

func TestSlideSessionRotatesWhenAskedAndKeepsTheGraceToken(t *testing.T) {
	f := newSessionFixture(t)
	_, session := f.login(t)
	handler := SlideSession(f.authenticator)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth.RequestRotation(r.Context())
		okHandler(w, r)
	}))
	r := httptest.NewRequest(http.MethodGet, "/feeds", nil)
	r.AddCookie(signedSession(session.SessionToken))
	rec := serveReq(handler, r)

	cookie := setCookieNamed(rec, auth.SessionCookieName)
	rotated, ok := secret.VerifySession(rootSecret, cookie.Value)
	if !ok || rotated == session.SessionToken {
		t.Fatalf("cookie token = %q (valid %v)", rotated, ok)
	}
	if csrf := setCookieNamed(rec, auth.CSRFCookieName); csrf == nil || !secret.VerifyCSRF(rootSecret, rotated, csrf.Value) {
		t.Error("the CSRF cookie must follow the rotated token")
	}
	for _, token := range []string{rotated, session.SessionToken} {
		if found, _ := models.FindSessionByToken(context.Background(), f.database, token); found == nil {
			t.Errorf("token %q no longer authenticates", token)
		}
	}
}

func TestSlideSessionDoesNotClobberInnerSetCookies(t *testing.T) {
	f := newSessionFixture(t)
	_, session := f.login(t)
	cookies := f.authenticator.Cookies()
	handler := SlideSession(f.authenticator)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Logout: removal cookies under both names must stand.
		for _, cookie := range append(cookies.RemoveSession(), cookies.RemoveCSRF()...) {
			http.SetCookie(w, cookie)
		}
		okHandler(w, r)
	}))
	r := httptest.NewRequest(http.MethodPost, "/logout", nil)
	r.AddCookie(signedSession(session.SessionToken))
	lines := setCookieLines(serveReq(handler, r))
	if len(lines) != 4 {
		t.Fatalf("Set-Cookie = %q", lines)
	}
	for _, line := range lines {
		if !strings.Contains(line, "Max-Age=0") {
			t.Errorf("a live cookie undid the logout: %s", line)
		}
	}
}

// --- forward auth ---

func forwardConfig(t *testing.T) config.Config {
	t.Helper()
	networks, err := config.ParseTrustedNetworks("10.0.0.0/8")
	if err != nil {
		t.Fatal(err)
	}
	return config.Config{
		Secret: rootSecret, AuthProxyHeader: "X-Auth-User", TrustedProxyNetworks: networks,
		AuthProxyUserCreation: true,
	}
}

func forwardRequest(path, username string) *http.Request {
	r := httptest.NewRequest(http.MethodGet, path, nil)
	r.RemoteAddr = "10.1.2.3:5555"
	if username != "" {
		r.Header.Set("X-Auth-User", username)
	}
	return r
}

func forwardHandler(t *testing.T, cfg config.Config, database *db.DB) http.Handler {
	t.Helper()
	login := auth.ForwardLogin{DB: database, Config: cfg, Audit: audit.New(slog.New(slog.DiscardHandler), rootSecret)}
	return ForwardAuth(login)(http.HandlerFunc(okHandler))
}

func TestForwardAuthSignsInAndRedirectsToTheSameURL(t *testing.T) {
	database := testutil.SQLite(t)
	rec := serveReq(forwardHandler(t, forwardConfig(t), database), forwardRequest("/feeds?x=1", "alice"))

	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/feeds?x=1" {
		t.Fatalf("response = %d %q", rec.Code, rec.Header().Get("Location"))
	}
	session := setCookieNamed(rec, auth.SessionCookieName)
	token, ok := secret.VerifySession(rootSecret, session.Value)
	if !ok {
		t.Fatal("unsigned session cookie")
	}
	if csrf := setCookieNamed(rec, auth.CSRFCookieName); csrf == nil || !secret.VerifyCSRF(rootSecret, token, csrf.Value) {
		t.Error("the CSRF cookie must be set with the session")
	}
	found, _ := models.FindSessionByToken(context.Background(), database, token)
	user, _ := models.FindUserByUsername(context.Background(), database, "alice")
	if found == nil || user == nil || found.UserID != user.ID {
		t.Fatalf("session %+v user %+v", found, user)
	}
	// First account on an empty instance is the admin, with no usable password.
	if user.Role != models.RoleAdmin || user.PasswordHash != "!" {
		t.Errorf("user = %+v", user)
	}
}

func TestForwardAuthSecondAccountIsAnOrdinaryUserWithADefaultCategory(t *testing.T) {
	database := testutil.SQLite(t)
	handler := forwardHandler(t, forwardConfig(t), database)
	serveReq(handler, forwardRequest("/", "alice"))
	serveReq(handler, forwardRequest("/", "bob"))
	bob, _ := models.FindUserByUsername(context.Background(), database, "bob")
	if bob == nil || bob.Role != models.RoleUser {
		t.Fatalf("bob = %+v", bob)
	}
	var count int
	if err := database.QueryRowContext(context.Background(),
		"SELECT COUNT(*) FROM category WHERE user_id = $1 AND name = 'Uncategorized'", bob.ID).Scan(&count); err != nil || count != 1 {
		t.Errorf("default category count = %d, %v", count, err)
	}
}

func TestForwardAuthPassesThroughWhenNotApplicable(t *testing.T) {
	database := testutil.SQLite(t)
	cfg := forwardConfig(t)
	handler := forwardHandler(t, cfg, database)

	untrusted := forwardRequest("/", "alice")
	untrusted.RemoteAddr = "203.0.113.5:1"
	for name, r := range map[string]*http.Request{
		"untrusted peer":     untrusted,
		"missing header":     forwardRequest("/", ""),
		"blank header":       forwardRequest("/", "   "),
		"api path":           forwardRequest("/api/x", "alice"),
		"events path":        forwardRequest("/events", "alice"),
		"static path":        forwardRequest("/static/a.css", "alice"),
		"tracking pixel":     forwardRequest("/p/1.gif", "alice"),
		"greader accounts":   forwardRequest("/accounts/ClientLogin", "alice"),
		"offline":            forwardRequest("/offline", "alice"),
		"reader api prefix":  forwardRequest("/reader/api/0/x", "alice"),
		"health":             forwardRequest("/health", "alice"),
		"service worker":     forwardRequest("/sw.js", "alice"),
		"favicon":            forwardRequest("/favicon.ico", "alice"),
		"api lookalike path": forwardRequest("/apiary", "alice"),
	} {
		if rec := serveReq(handler, r); rec.Code != http.StatusOK || rec.Body.String() != "ok" {
			t.Errorf("%s: response = %d %q", name, rec.Code, rec.Body.String())
		}
	}
	if user, _ := models.FindUserByUsername(context.Background(), database, "alice"); user != nil {
		t.Error("a pass-through request created an account")
	}
	// Feature off: the layer is a no-op even with the header present.
	off := forwardHandler(t, config.Config{Secret: rootSecret}, database)
	if rec := serveReq(off, forwardRequest("/", "alice")); rec.Code != http.StatusOK {
		t.Errorf("disabled feature: %d", rec.Code)
	}
}

func TestForwardAuthLetsAValidSessionWinButNotAnInvalidCookie(t *testing.T) {
	database := testutil.SQLite(t)
	handler := forwardHandler(t, forwardConfig(t), database)
	user, err := models.CreateUser(context.Background(), database, "carol", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	session, err := models.CreateSession(context.Background(), database, models.NewSession{UserID: user.ID})
	if err != nil {
		t.Fatal(err)
	}
	valid := forwardRequest("/", "mallory")
	valid.AddCookie(signedSession(session.SessionToken))
	if rec := serveReq(handler, valid); rec.Code != http.StatusOK {
		t.Errorf("valid session: %d", rec.Code)
	}
	if user, _ := models.FindUserByUsername(context.Background(), database, "mallory"); user != nil {
		t.Error("the proxy header overrode a live session")
	}

	stale := forwardRequest("/", "dave")
	stale.AddCookie(signedSession("not-in-the-database"))
	if rec := serveReq(handler, stale); rec.Code != http.StatusSeeOther {
		t.Errorf("an unknown session token blocked forward-auth: %d", rec.Code)
	}
}

func TestForwardAuthRefusals(t *testing.T) {
	database := testutil.SQLite(t)
	cfg := forwardConfig(t)
	existing, err := models.CreateUser(context.Background(), database, "erin", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	if err := models.DisableUser(context.Background(), database, existing.ID); err != nil {
		t.Fatal(err)
	}
	cfg.AuthProxyUserCreation = false

	for name, username := range map[string]string{
		"disabled account":              "erin",
		"unknown account, creation off": "frank",
	} {
		rec := serveReq(forwardHandler(t, cfg, database), forwardRequest("/feeds", username))
		if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" {
			t.Errorf("%s: %d %q", name, rec.Code, rec.Header().Get("Location"))
		}
		if setCookieNamed(rec, auth.SessionCookieName) != nil {
			t.Errorf("%s: a session cookie was issued", name)
		}
		if setCookieNamed(rec, flash.CookieName) == nil {
			t.Errorf("%s: no flash explaining the refusal", name)
		}
	}

	cfg.AuthProxyUserCreation = true
	rec := serveReq(forwardHandler(t, cfg, database), forwardRequest("/feeds", strings.Repeat("x", 65)))
	if rec.Header().Get("Location") != "/login" {
		t.Errorf("an invalid username must be refused: %d %q", rec.Code, rec.Header().Get("Location"))
	}
}

func TestForwardAuthGroupMappingSetsTheRoleOnEveryLogin(t *testing.T) {
	database := testutil.SQLite(t)
	cfg := forwardConfig(t)
	cfg.AuthProxyGroupsHeader = "X-Auth-Groups"
	cfg.AuthProxyAdminGroup = "admins"
	handler := forwardHandler(t, cfg, database)

	login := func(groups string) {
		r := forwardRequest("/", "gina")
		r.Header.Set("X-Auth-Groups", groups)
		if rec := serveReq(handler, r); rec.Code != http.StatusSeeOther {
			t.Fatalf("login with %q: %d", groups, rec.Code)
		}
	}
	roleOf := func() models.Role {
		user, _ := models.FindUserByUsername(context.Background(), database, "gina")
		return user.Role
	}
	login("staff, admins")
	if roleOf() != models.RoleAdmin {
		t.Error("the admin group must make an admin")
	}
	login("staff")
	if roleOf() != models.RoleUser {
		t.Error("losing the group must demote on the next login")
	}
}

func TestForwardAuthParsesGroups(t *testing.T) {
	if got := auth.ParseGroups("admins, users ,, dev"); strings.Join(got, "|") != "admins|users|dev" {
		t.Errorf("groups = %q", got)
	}
	if got := auth.ParseGroups("   "); len(got) != 0 {
		t.Errorf("blank = %q", got)
	}
	if auth.RoleFromGroups([]string{"users", "admins"}, "admins") != models.RoleAdmin ||
		auth.RoleFromGroups([]string{"users"}, "admins") != models.RoleUser ||
		auth.RoleFromGroups(nil, "admins") != models.RoleUser {
		t.Error("role from groups")
	}
}

func TestAuthenticatedRequestReportsForwardAuth(t *testing.T) {
	database := testutil.SQLite(t)
	cfg := forwardConfig(t)
	authenticator := &auth.Authenticator{DB: database, Config: cfg, Audit: audit.New(slog.New(slog.DiscardHandler), rootSecret)}
	rec := serveReq(forwardHandler(t, cfg, database), forwardRequest("/", "alice"))
	token, _ := secret.VerifySession(rootSecret, setCookieNamed(rec, auth.SessionCookieName).Value)

	r := forwardRequest("/", "alice")
	r.AddCookie(signedSession(token))
	principal, err := authenticator.Authenticate(r)
	if err != nil || !principal.ViaForwardAuth {
		t.Fatalf("principal = %+v, %v", principal, err)
	}
	direct := httptest.NewRequest(http.MethodGet, "/", nil)
	direct.AddCookie(signedSession(token))
	if principal, err := authenticator.Authenticate(direct); err != nil || principal.ViaForwardAuth {
		t.Errorf("without the proxy header: %+v, %v", principal, err)
	}
}

var _ = time.Second
