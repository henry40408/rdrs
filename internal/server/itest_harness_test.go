package server

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/testutil"
)

// Port of tests/common/mod.rs: the shared fixture of the ported Rust
// integration suites. Every app owns a fresh SQLite database.

const fixturePassword = "vulture-mango-77-quilt"

// itestConfig is default_test_config() of the Rust suites.
func itestConfig() config.Config {
	return config.Config{
		FetchAllowPrivate: "127.0.0.1", MultiUser: true,
		Secret: make([]byte, 32), UserAgent: "RDRS-Test/1.0",
		WebAuthnRPID: "localhost", WebAuthnRPOrigin: "http://localhost:8080", WebAuthnRPName: "rdrs-test",
		LoginRateLimitAttempts: auth.LoginMaxAttempts, LoginRateLimitWindowSecs: auth.LoginWindowSecs,
		HSTSMaxAge: 31_536_000, HSTSIncludeSubdomains: true,
	}
}

// itestApp is a cookie-saving client over the real handler stack, with the
// database kept for tests that seed data behind the router's back.
type itestApp struct {
	t       testing.TB
	App     *App
	DB      *db.DB
	handler http.Handler
	cookies map[string]string
	headers http.Header
}

func newITestApp(t testing.TB, config config.Config) *itestApp {
	t.Helper()
	return newITestAppWith(t, Options{Config: config})
}

// newITestAppWith lets a test set options beyond the Config.
func newITestAppWith(t testing.TB, options Options) *itestApp {
	t.Helper()
	t.Setenv("RDRS_FAST_HASH", "1")
	options.DB = testutil.SQLite(t)
	options.Audit = audit.New(slog.New(slog.DiscardHandler), options.Config.Secret)
	options.SkipSetupToken = options.SetupToken == ""
	app := NewApp(options)
	return &itestApp{
		t: t, App: app, DB: options.DB, handler: app.Handler(),
		cookies: map[string]string{}, headers: http.Header{},
	}
}

func defaultITestApp(t testing.TB) *itestApp { return newITestApp(t, itestConfig()) }

// itestRequest describes one request; the zero value is a plain GET.
type itestRequest struct {
	Method  string
	Path    string
	Body    string
	Headers map[string]string
	// Host overrides the request's Host (httptest ignores a Host header).
	Host string
	// NoCookies sends the request without the saved cookies.
	NoCookies bool
}

// itestResponse is a recorded response with its body read.
type itestResponse struct {
	*httptest.ResponseRecorder
}

func (r itestResponse) Text() string { return r.Body.String() }

func (r itestResponse) Cookie(name string) *http.Cookie {
	for _, cookie := range r.Result().Cookies() {
		if cookie.Name == name {
			return cookie
		}
	}
	return nil
}

// Do sends the request and saves the cookies the response sets.
func (a *itestApp) Do(request itestRequest) itestResponse {
	a.t.Helper()
	method := request.Method
	if method == "" {
		method = http.MethodGet
	}
	r := httptest.NewRequest(method, request.Path, strings.NewReader(request.Body))
	if request.Host != "" {
		r.Host = request.Host
	}
	for name, values := range a.headers {
		r.Header[name] = append([]string(nil), values...)
	}
	for name, value := range request.Headers {
		r.Header.Set(name, value)
	}
	if !request.NoCookies {
		for name, value := range a.cookies {
			r.AddCookie(&http.Cookie{Name: name, Value: value})
		}
	}
	recorder := httptest.NewRecorder()
	a.handler.ServeHTTP(recorder, r)
	response := itestResponse{recorder}
	if !request.NoCookies {
		a.saveCookies(response)
	}
	return response
}

func (a *itestApp) saveCookies(response itestResponse) {
	for _, cookie := range response.Result().Cookies() {
		if cookie.MaxAge < 0 || cookie.Value == "" {
			delete(a.cookies, cookie.Name)
			continue
		}
		a.cookies[cookie.Name] = cookie.Value
	}
}

func (a *itestApp) Get(path string) itestResponse {
	a.t.Helper()
	return a.Do(itestRequest{Path: path})
}

// GetOK is get_ok: GET, expect 200, return the body.
func (a *itestApp) GetOK(path string) string {
	a.t.Helper()
	response := a.Get(path)
	if response.Code != http.StatusOK {
		a.t.Fatalf("GET %s = %d, want 200: %s", path, response.Code, response.Text())
	}
	return response.Text()
}

func (a *itestApp) PostJSON(path string, body any) itestResponse {
	a.t.Helper()
	return a.sendJSON(http.MethodPost, path, body)
}

func (a *itestApp) PutJSON(path string, body any) itestResponse {
	a.t.Helper()
	return a.sendJSON(http.MethodPut, path, body)
}

func (a *itestApp) sendJSON(method, path string, body any) itestResponse {
	a.t.Helper()
	encoded, err := json.Marshal(body)
	if err != nil {
		a.t.Fatal(err)
	}
	return a.Do(itestRequest{
		Method: method, Path: path, Body: string(encoded),
		Headers: map[string]string{"Content-Type": "application/json"},
	})
}

func (a *itestApp) PostForm(path string, form url.Values) itestResponse {
	a.t.Helper()
	return a.Do(itestRequest{
		Method: http.MethodPost, Path: path, Body: form.Encode(),
		Headers: map[string]string{"Content-Type": "application/x-www-form-urlencoded"},
	})
}

// Login signs the fixture password in over POST /api/session.
func (a *itestApp) Login(username string) { a.t.Helper(); a.LoginWith(username, fixturePassword) }

// LoginWith signs in and pins the new session's CSRF token as a default
// header, like the csrf.js of a browser that just logged in.
func (a *itestApp) LoginWith(username, password string) {
	a.t.Helper()
	response := a.PostJSON("/api/session", map[string]string{"username": username, "password": password})
	if response.Code != http.StatusOK {
		a.t.Fatalf("login = %d: %s", response.Code, response.Text())
	}
	a.ApplyCSRF(response)
}

// ApplyCSRF echoes the response's csrf_token cookie as X-CSRF-Token on every
// later request, replacing a stale one.
func (a *itestApp) ApplyCSRF(response itestResponse) {
	a.t.Helper()
	cookie := response.Cookie(auth.CSRFCookieName)
	if cookie == nil {
		a.t.Fatal("response set no csrf_token cookie")
	}
	a.headers = http.Header{"X-Csrf-Token": {cookie.Value}}
}

// SeedAccount creates an account straight in the database, with the default
// category the real paths seed.
func (a *itestApp) SeedAccount(username, password string, role models.Role) *models.User {
	a.t.Helper()
	ctx := context.Background()
	hash, err := auth.HashPassword(password)
	if err != nil {
		a.t.Fatal(err)
	}
	user, err := models.CreateUser(ctx, a.DB, username, hash, role)
	if err != nil {
		a.t.Fatal(err)
	}
	if _, err := models.CreateCategory(ctx, a.DB, user.ID, "Uncategorized"); err != nil {
		a.t.Fatal(err)
	}
	return user
}

// SetupUsers makes the "admin" and "user" accounts with the fixture password
// (no default category, like setup_users) and returns their ids.
func (a *itestApp) SetupUsers() (adminID, userID int64) {
	a.t.Helper()
	ctx := context.Background()
	hash, err := auth.HashPassword(fixturePassword)
	if err != nil {
		a.t.Fatal(err)
	}
	admin, err := models.CreateUser(ctx, a.DB, "admin", hash, models.RoleAdmin)
	if err != nil {
		a.t.Fatal(err)
	}
	user, err := models.CreateUser(ctx, a.DB, "user", hash, models.RoleUser)
	if err != nil {
		a.t.Fatal(err)
	}
	return admin.ID, user.ID
}

// SeedUserWithEntry makes an admin with one category, feed and entry, like the
// setup_user_with_entry helpers of the Rust suites. It returns the user and
// entry ids.
func (a *itestApp) SeedUserWithEntry(username, password string) (userID, entryID int64) {
	a.t.Helper()
	ctx := context.Background()
	hash, err := auth.HashPassword(password)
	if err != nil {
		a.t.Fatal(err)
	}
	user, err := models.CreateUser(ctx, a.DB, username, hash, models.RoleAdmin)
	if err != nil {
		a.t.Fatal(err)
	}
	category, err := models.CreateCategory(ctx, a.DB, user.ID, "Test Category")
	if err != nil {
		a.t.Fatal(err)
	}
	title := "Test Feed"
	feed, err := models.CreateFeed(ctx, a.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/" + username + "/feed.xml", Title: &title,
	})
	if err != nil {
		a.t.Fatal(err)
	}
	entryTitle, link, content := "Test Entry", "https://example.com/entry/1", "<p>Content</p>"
	result, err := entry.UpsertEntry(ctx, a.DB, &entry.Upsert{
		FeedID: feed.ID, GUID: username + "-guid-1", Title: &entryTitle, Link: &link, Content: &content,
	})
	if err != nil {
		a.t.Fatal(err)
	}
	return user.ID, result.ID
}
