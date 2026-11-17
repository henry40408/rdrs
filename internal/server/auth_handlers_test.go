package server

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/testutil"
)

const testPassword = "correct horse battery staple"

type authFixture struct {
	handler  http.Handler
	database *db.DB
	secret   []byte
}

func newAuthFixture(t *testing.T) *authFixture {
	t.Helper()
	t.Setenv("RDRS_FAST_HASH", "1")
	database := testutil.SQLite(t)
	key := []byte("01234567890123456789012345678901")
	app := NewApp(Options{
		Config: config.Config{Secret: key, MultiUser: true}, DB: database,
		Audit:   audit.New(slog.New(slog.DiscardHandler), key),
		Limiter: auth.NewRateLimiter(0, 60), SkipSetupToken: true,
	})
	return &authFixture{handler: app.Handler(), database: database, secret: key}
}

func (f *authFixture) do(r *http.Request) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	f.handler.ServeHTTP(rec, r)
	return rec
}

func jsonRequest(method, path, body string) *http.Request {
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	r.Header.Set("Content-Type", "application/json")
	return r
}

func (f *authFixture) createAdmin(t *testing.T) {
	t.Helper()
	rec := f.do(jsonRequest("POST", "/api/setup", `{"username":"root","password":"`+testPassword+`"}`))
	if rec.Code != http.StatusCreated {
		t.Fatalf("setup = %d %s", rec.Code, rec.Body)
	}
}

func cookieNamed(rec *httptest.ResponseRecorder, name string) *http.Cookie {
	for _, c := range rec.Result().Cookies() {
		if c.Name == name {
			return c
		}
	}
	return nil
}

func TestSetupAPICreatesTheFirstAdminOnce(t *testing.T) {
	f := newAuthFixture(t)
	f.createAdmin(t)
	user, _ := models.FindUserByUsername(context.Background(), f.database, "root")
	if user == nil || user.Role != models.RoleAdmin {
		t.Fatalf("user = %+v", user)
	}
	rec := f.do(jsonRequest("POST", "/api/setup", `{"username":"second","password":"`+testPassword+`"}`))
	if rec.Code != http.StatusForbidden {
		t.Errorf("second setup = %d, want 403", rec.Code)
	}
}

func TestJSONEndpointsAreStrictAboutTheBody(t *testing.T) {
	f := newAuthFixture(t)
	plain := httptest.NewRequest("POST", "/api/session", strings.NewReader(`{"username":"a","password":"b"}`))
	plain.Header.Set("Content-Type", "text/plain")
	multipart := httptest.NewRequest("POST", "/api/session", strings.NewReader("x"))
	multipart.Header.Set("Content-Type", "multipart/form-data; boundary=x")
	for name, tc := range map[string]struct {
		request *http.Request
		want    int
	}{
		"wrong content type": {plain, http.StatusUnsupportedMediaType},
		"multipart":          {multipart, http.StatusUnsupportedMediaType},
		"missing field":      {jsonRequest("POST", "/api/session", `{"username":"a"}`), http.StatusUnprocessableEntity},
		"unknown account":    {jsonRequest("POST", "/api/session", `{"username":"a","password":"b"}`), http.StatusUnauthorized},
		"wrong method":       {httptest.NewRequest("PUT", "/api/session", nil), http.StatusMethodNotAllowed},
	} {
		if got := f.do(tc.request).Code; got != tc.want {
			t.Errorf("%s: status = %d, want %d", name, got, tc.want)
		}
	}
}

func TestLoginSetsFreshCookiesEvenWhenAnOldSessionIsPresent(t *testing.T) {
	f := newAuthFixture(t)
	f.createAdmin(t)
	request := jsonRequest("POST", "/api/session", `{"username":"root","password":"`+testPassword+`"}`)
	request.AddCookie(&http.Cookie{Name: auth.SessionCookieName, Value: "old.00"})
	rec := f.do(request)
	if rec.Code != http.StatusOK {
		t.Fatalf("login = %d %s", rec.Code, rec.Body)
	}
	session := cookieNamed(rec, auth.SessionCookieName)
	if session == nil || session.Value == "old.00" || session.MaxAge < 0 {
		t.Errorf("session cookie = %+v", session)
	}
	if cookieNamed(rec, auth.CSRFCookieName) == nil {
		t.Error("the CSRF cookie must be reissued with the session")
	}
	var identity map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &identity); err != nil || identity["username"] != "root" {
		t.Errorf("body = %s", rec.Body)
	}
}

func TestLoginFormWrongPasswordRendersTheFormAgain(t *testing.T) {
	f := newAuthFixture(t)
	f.createAdmin(t)
	page := f.do(httptest.NewRequest("GET", "/login", nil))
	cookie := cookieNamed(page, auth.SessionCookieName)
	csrf := csrfField.FindStringSubmatch(page.Body.String())[1]
	request := formRequest("/login", "_csrf="+csrf+"&username=root&password=wrong")
	request.AddCookie(cookie)
	rec := f.do(request)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Invalid credentials") {
		t.Errorf("status %d body %.300s", rec.Code, rec.Body)
	}
}

func TestLogoutClearsCookiesAndSiteData(t *testing.T) {
	f := newAuthFixture(t)
	f.createAdmin(t)
	login := f.do(jsonRequest("POST", "/api/session", `{"username":"root","password":"`+testPassword+`"}`))
	session, csrf := cookieNamed(login, auth.SessionCookieName), cookieNamed(login, auth.CSRFCookieName)

	request := httptest.NewRequest("DELETE", "/api/session", nil)
	request.AddCookie(session)
	request.Header.Set("X-CSRF-Token", csrf.Value)
	rec := f.do(request)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Header().Get("Clear-Site-Data"), "storage") {
		t.Fatalf("logout = %d %v", rec.Code, rec.Header())
	}
	token, _ := secret.VerifySession(f.secret, session.Value)
	if found, _ := models.FindSessionByToken(context.Background(), f.database, token); found != nil {
		t.Error("the session row must be gone")
	}
}

func TestReauthRejectsAWrongPasswordAndAcceptsTheRightOne(t *testing.T) {
	f := newAuthFixture(t)
	f.createAdmin(t)
	login := f.do(jsonRequest("POST", "/api/session", `{"username":"root","password":"`+testPassword+`"}`))
	session, csrf := cookieNamed(login, auth.SessionCookieName), cookieNamed(login, auth.CSRFCookieName)
	for password, want := range map[string]int{"wrong": http.StatusUnauthorized, testPassword: http.StatusNoContent} {
		request := jsonRequest("POST", "/api/session/reauth", `{"password":"`+password+`"}`)
		request.AddCookie(session)
		request.Header.Set("X-CSRF-Token", csrf.Value)
		if got := f.do(request).Code; got != want {
			t.Errorf("reauth(%q) = %d, want %d", password, got, want)
		}
	}
}

func TestInviteRedeemSetsThePasswordOnceAndSpendsTheLink(t *testing.T) {
	f := newAuthFixture(t)
	ctx := context.Background()
	user, err := models.CreateUser(ctx, f.database, "carol", "!", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := models.IssueInvite(ctx, f.database, models.IssueInviteParams{SecretKey: f.secret, UserID: user.ID, Token: "tok-1"}); err != nil {
		t.Fatal(err)
	}
	redeem := func(password string) *httptest.ResponseRecorder {
		page := f.do(httptest.NewRequest("GET", "/invite/tok-1", nil))
		match := csrfField.FindStringSubmatch(page.Body.String())
		if match == nil {
			return page
		}
		request := formRequest("/invite/tok-1", "_csrf="+match[1]+"&password="+password+"&confirm_password="+password)
		request.AddCookie(cookieNamed(page, auth.SessionCookieName))
		return f.do(request)
	}

	if rec := redeem("short"); rec.Code != http.StatusOK {
		t.Errorf("a weak password re-renders the form, got %d", rec.Code)
	}
	rec := redeem("correct+horse+battery+staple")
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/login" {
		t.Fatalf("redeem = %d %q", rec.Code, rec.Header().Get("Location"))
	}
	again := f.do(httptest.NewRequest("GET", "/invite/tok-1", nil))
	if strings.Contains(again.Body.String(), "carol") {
		t.Error("a spent link must not reveal the username")
	}
}
