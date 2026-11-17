package server

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/url"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

// Port of tests/auth_test.rs.

const itestSetupToken = "token-printed-on-stderr"

func (a *itestApp) setupJSON(username string, extra map[string]string) itestResponse {
	a.t.Helper()
	body := map[string]string{"username": username, "password": fixturePassword}
	for key, value := range extra {
		body[key] = value
	}
	return a.PostJSON("/api/setup", body)
}

func (a *itestApp) userCount() int64 {
	a.t.Helper()
	count, err := models.CountUsers(context.Background(), a.DB)
	if err != nil {
		a.t.Fatal(err)
	}
	return count
}

func (a *itestApp) createUserDirectly(username, password string) {
	a.t.Helper()
	hash, err := auth.HashPassword(password)
	if err != nil {
		a.t.Fatal(err)
	}
	if _, err := models.CreateUser(context.Background(), a.DB, username, hash, models.RoleUser); err != nil {
		a.t.Fatal(err)
	}
}

func jsonField(t testing.TB, response itestResponse, field string) any {
	t.Helper()
	var body map[string]any
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatalf("not JSON: %v: %s", err, response.Text())
	}
	return body[field]
}

func wantStatus(t testing.TB, response itestResponse, want int) {
	t.Helper()
	if response.Code != want {
		t.Fatalf("status = %d, want %d: %s", response.Code, want, response.Text())
	}
}

func TestRegisterFirstUserBecomesAdmin(t *testing.T) {
	response := defaultITestApp(t).setupJSON("admin", nil)
	wantStatus(t, response, http.StatusCreated)
	if jsonField(t, response, "username") != "admin" || jsonField(t, response, "role") != "admin" {
		t.Errorf("body = %s", response.Text())
	}
}

func TestSetupRejectsInvalidUsernames(t *testing.T) {
	for _, bad := range []string{"", "   ", "a\tb", strings.Repeat("a", 65)} {
		wantStatus(t, defaultITestApp(t).setupJSON(bad, nil), http.StatusBadRequest)
	}
}

func TestLoginMatchesTrimmedUsernameStoredBySetup(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	wantStatus(t, app.PostJSON("/api/session", map[string]string{"username": "admin ", "password": fixturePassword}), http.StatusOK)
}

func TestLoginStillMatchesLegacyUsernameWithSpaces(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly(" legacy ", fixturePassword)
	wantStatus(t, app.PostJSON("/api/session", map[string]string{"username": " legacy ", "password": fixturePassword}), http.StatusOK)
}

func TestLoginRateLimitIsSharedAcrossUsernameSpellings(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("admin", fixturePassword)
	for i := 0; i < 20; i++ {
		name := "admin"
		if i%2 == 1 {
			name = "admin "
		}
		response := app.PostJSON("/api/session", map[string]string{"username": name, "password": "wrong-password-xyz"})
		if response.Code == http.StatusTooManyRequests {
			return
		}
	}
	t.Error("both spellings must drain one account budget")
}

func TestSetupStoresTrimmedUsername(t *testing.T) {
	response := defaultITestApp(t).setupJSON("  admin  ", nil)
	wantStatus(t, response, http.StatusCreated)
	if jsonField(t, response, "username") != "admin" {
		t.Errorf("body = %s", response.Text())
	}
}

func setupTokenApp(t *testing.T) *itestApp {
	return newITestAppWith(t, Options{Config: itestConfig(), SetupToken: itestSetupToken})
}

func TestSetupRefusesAMissingOrWrongTokenAndCreatesNothing(t *testing.T) {
	app := setupTokenApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusForbidden)
	wantStatus(t, app.setupJSON("admin", map[string]string{"setup_token": "guess"}), http.StatusForbidden)
	if count := app.userCount(); count != 0 {
		t.Errorf("users = %d", count)
	}
}

func TestSetupAcceptsTheLoggedToken(t *testing.T) {
	wantStatus(t, setupTokenApp(t).setupJSON("admin", map[string]string{"setup_token": itestSetupToken}), http.StatusCreated)
}

func (a *itestApp) setupForm(token string) itestResponse {
	a.t.Helper()
	page := a.Get("/setup")
	a.ApplyCSRF(page)
	return a.PostForm("/setup", url.Values{
		"username": {"admin"}, "password": {fixturePassword},
		"confirm-password": {fixturePassword}, "setup-token": {token},
	})
}

func TestSetupFormRefusesAWrongTokenAndShowsWhy(t *testing.T) {
	app := setupTokenApp(t)
	response := app.setupForm("guess")
	if !strings.Contains(response.Text(), "Invalid setup token") {
		t.Errorf("the page must say why: %s", response.Text())
	}
	if count := app.userCount(); count != 0 {
		t.Errorf("users = %d", count)
	}
}

func TestSetupFormAcceptsTheLoggedTokenIgnoringSurroundingWhitespace(t *testing.T) {
	app := setupTokenApp(t)
	wantStatus(t, app.setupForm(" "+itestSetupToken+"\n"), http.StatusSeeOther)
	if count := app.userCount(); count != 1 {
		t.Errorf("users = %d", count)
	}
}

func TestConcurrentSetupsWithDifferentUsernamesCreateOneAccount(t *testing.T) {
	app := defaultITestApp(t)
	var wg sync.WaitGroup
	statuses := make([]int, 5)
	for i := range statuses {
		wg.Add(1)
		go func() {
			defer wg.Done()
			body := `{"username":"admin` + string(rune('0'+i)) + `","password":"` + fixturePassword + `"}`
			statuses[i] = app.Do(itestRequest{
				Method: http.MethodPost, Path: "/api/setup", Body: body, NoCookies: true,
				Headers: map[string]string{"Content-Type": "application/json"},
			}).Code
		}()
	}
	wg.Wait()
	created := 0
	for _, status := range statuses {
		if status == http.StatusCreated {
			created++
		}
	}
	if created != 1 || app.userCount() != 1 {
		t.Errorf("created = %d, users = %d (%v)", created, app.userCount(), statuses)
	}
}

func TestSetupClosesPermanentlyAfterTheFirstAccount(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	wantStatus(t, app.setupJSON("user1", nil), http.StatusForbidden)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusForbidden)
}

func TestAClosedSetupEndpointAnswersTheSameForAnyUsername(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("admin", fixturePassword)
	existing, absent := app.setupJSON("admin", nil), app.setupJSON("nobody-here", nil)
	if existing.Code != absent.Code || existing.Text() != absent.Text() {
		t.Errorf("answers differ: %d %q vs %d %q", existing.Code, existing.Text(), absent.Code, absent.Text())
	}
}

func TestRegisterDisabledStillAllowsFirstAccount(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	wantStatus(t, app.setupJSON("user", nil), http.StatusForbidden)
}

func TestRegisterMultiUserDisabled(t *testing.T) {
	config := itestConfig()
	config.MultiUser = false
	app := newITestApp(t, config)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	wantStatus(t, app.setupJSON("user1", nil), http.StatusForbidden)
}

func (a *itestApp) loginAttempt(username, password string) itestResponse {
	return a.PostJSON("/api/session", map[string]string{"username": username, "password": password})
}

func TestLoginSuccess(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	response := app.loginAttempt("admin", fixturePassword)
	wantStatus(t, response, http.StatusOK)
	app.ApplyCSRF(response)
	if jsonField(t, response, "username") != "admin" {
		t.Errorf("body = %s", response.Text())
	}
}

func TestLoginWrongPassword(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	wantStatus(t, app.loginAttempt("admin", "wrongpassword"), http.StatusUnauthorized)
}

func TestLoginNonexistentUser(t *testing.T) {
	wantStatus(t, defaultITestApp(t).loginAttempt("nonexistent", fixturePassword), http.StatusUnauthorized)
}

func exhaustLoginBudget(t *testing.T, app *itestApp) {
	t.Helper()
	app.createUserDirectly("admin", fixturePassword)
	for i := 0; i < 5; i++ {
		wantStatus(t, app.loginAttempt("admin", "wrongpassword"), http.StatusUnauthorized)
	}
}

func TestLoginRateLimitedAfterFiveFailures(t *testing.T) {
	app := defaultITestApp(t)
	exhaustLoginBudget(t, app)
	wantStatus(t, app.loginAttempt("admin", "wrongpassword"), http.StatusTooManyRequests)
}

func TestThrottledResponseCarriesRetryAfter(t *testing.T) {
	app := defaultITestApp(t)
	exhaustLoginBudget(t, app)
	response := app.loginAttempt("admin", "wrongpassword")
	wantStatus(t, response, http.StatusTooManyRequests)
	seconds, err := strconv.Atoi(response.Header().Get("Retry-After"))
	if err != nil || seconds < 1 || seconds > 60 {
		t.Errorf("Retry-After = %q, want 1..60 seconds", response.Header().Get("Retry-After"))
	}
}

func TestLoginRateLimitDoesNotLeakValidCredentials(t *testing.T) {
	app := defaultITestApp(t)
	exhaustLoginBudget(t, app)
	wantStatus(t, app.loginAttempt("admin", fixturePassword), http.StatusTooManyRequests)
}

func TestSuccessfulLoginDoesNotConsumeTheBudget(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("admin", fixturePassword)
	for i := 0; i < 10; i++ {
		response := app.loginAttempt("admin", fixturePassword)
		wantStatus(t, response, http.StatusOK)
		app.ApplyCSRF(response)
	}
}

func exhaustSetupBudget(t *testing.T, app *itestApp) {
	t.Helper()
	app.createUserDirectly("admin", fixturePassword)
	for i := 0; i < 5; i++ {
		wantStatus(t, app.setupJSON("user"+strconv.Itoa(i), nil), http.StatusForbidden)
	}
	wantStatus(t, app.setupJSON("user5", nil), http.StatusTooManyRequests)
}

func TestSetupIsRateLimited(t *testing.T) {
	exhaustSetupBudget(t, defaultITestApp(t))
}

func TestRegistrationBudgetExhaustionDoesNotLockOutLogin(t *testing.T) {
	app := defaultITestApp(t)
	exhaustSetupBudget(t, app)
	wantStatus(t, app.loginAttempt("admin", fixturePassword), http.StatusOK)
}

func TestPasskeyAuthStartIsRateLimited(t *testing.T) {
	app := defaultITestApp(t)
	for i := 0; i < 5; i++ {
		if app.Do(itestRequest{Method: http.MethodPost, Path: "/api/passkey/auth/start"}).Code == http.StatusTooManyRequests {
			t.Fatalf("attempt %d was already throttled", i+1)
		}
	}
	wantStatus(t, app.Do(itestRequest{Method: http.MethodPost, Path: "/api/passkey/auth/start"}), http.StatusTooManyRequests)
}

func (a *itestApp) seedPasskey(credentialID string) {
	a.t.Helper()
	a.createUserDirectly("admin", fixturePassword)
	user, err := models.FindUserByUsername(context.Background(), a.DB, "admin")
	if err != nil || user == nil {
		a.t.Fatalf("user: %v %v", user, err)
	}
	_, err = models.CreatePasskey(context.Background(), a.DB, models.NewPasskey{
		UserID: user.ID, CredentialID: []byte(credentialID), PublicKey: []byte("{}"), Name: "Laptop",
	})
	if err != nil {
		a.t.Fatal(err)
	}
}

func (a *itestApp) passkeyChallenge() itestResponse {
	a.t.Helper()
	response := a.Do(itestRequest{Method: http.MethodPost, Path: "/api/passkey/auth/start"})
	wantStatus(a.t, response, http.StatusOK)
	return response
}

func TestPasskeyChallengeNeverDisclosesRegisteredCredentials(t *testing.T) {
	app := defaultITestApp(t)
	app.seedPasskey("secret-credential-id")
	response := app.passkeyChallenge()
	var body struct {
		Options struct {
			PublicKey struct {
				AllowCredentials []any `json:"allowCredentials"`
			} `json:"publicKey"`
		} `json:"options"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	if len(body.Options.PublicKey.AllowCredentials) != 0 {
		t.Errorf("the challenge must name no credentials: %s", response.Text())
	}
	encoded := base64.RawURLEncoding.EncodeToString([]byte("secret-credential-id"))
	if strings.Contains(response.Text(), encoded) || strings.Contains(response.Text(), "secret-credential-id") {
		t.Errorf("a registered credential ID leaked into the challenge: %s", response.Text())
	}
}

func TestPasskeyChallengeIsIdenticalWithAndWithoutRegisteredPasskeys(t *testing.T) {
	scrubbed := func(response itestResponse) map[string]any {
		var body map[string]any
		if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
			t.Fatal(err)
		}
		publicKey := body["options"].(map[string]any)["publicKey"].(map[string]any)
		publicKey["challenge"] = nil
		return body
	}
	empty := scrubbed(defaultITestApp(t).passkeyChallenge())
	populatedApp := defaultITestApp(t)
	populatedApp.seedPasskey("cred")
	populated := scrubbed(populatedApp.passkeyChallenge())
	if !reflect.DeepEqual(empty, populated) {
		t.Errorf("challenges differ:\n%v\n%v", empty, populated)
	}
}

// signedInApp is an app with "admin" set up over /api/setup and signed in.
func signedInApp(t *testing.T, username string) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON(username, nil), http.StatusCreated)
	app.Login(username)
	return app
}

func (a *itestApp) currentUsername() string {
	a.t.Helper()
	response := a.Get("/api/user")
	wantStatus(a.t, response, http.StatusOK)
	username, _ := jsonField(a.t, response, "username").(string)
	return username
}

func TestGetCurrentUser(t *testing.T) {
	if got := signedInApp(t, "admin").currentUsername(); got != "admin" {
		t.Errorf("username = %q", got)
	}
}

func TestGetCurrentUserUnauthorized(t *testing.T) {
	wantStatus(t, defaultITestApp(t).Get("/api/user"), http.StatusUnauthorized)
}

func (a *itestApp) deleteSession() itestResponse {
	return a.Do(itestRequest{Method: http.MethodDelete, Path: "/api/session"})
}

func TestLogout(t *testing.T) {
	app := signedInApp(t, "admin")
	wantStatus(t, app.deleteSession(), http.StatusOK)
	wantStatus(t, app.Get("/api/user"), http.StatusUnauthorized)
}

// signInThenLoseTheSession returns the CSRF token of a session whose row is gone.
func signInThenLoseTheSession(t *testing.T) (*itestApp, string) {
	t.Helper()
	app := defaultITestApp(t)
	created := app.setupJSON("u", nil)
	wantStatus(t, created, http.StatusCreated)
	userID := int64(jsonField(t, created, "id").(float64))
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	csrf := login.Cookie("csrf_token").Value
	if err := models.DeleteUserSessions(context.Background(), app.DB, userID); err != nil {
		t.Fatal(err)
	}
	wantStatus(t, app.Get("/api/user"), http.StatusUnauthorized)
	return app, csrf
}

func TestLogoutAfterTheSessionAlreadyEndedStillSignsOut(t *testing.T) {
	app, csrf := signInThenLoseTheSession(t)
	response := app.Do(itestRequest{Method: http.MethodDelete, Path: "/api/session", Headers: map[string]string{"X-CSRF-Token": csrf}})
	wantStatus(t, response, http.StatusOK)
	if jsonField(t, response, "redirect_to") != "/login" || jsonField(t, response, "via_forward_auth") != false {
		t.Errorf("body = %s", response.Text())
	}
	if !strings.Contains(flashText(t, response), "You have been logged out.") {
		t.Error("the flash must say the user was logged out")
	}
	if got := response.Header().Get("Clear-Site-Data"); got != `"cache", "storage"` {
		t.Errorf("Clear-Site-Data = %q", got)
	}
	removed := false
	for _, header := range setCookieHeaders(response) {
		removed = removed || strings.HasPrefix(header, "session_token=;") || strings.HasPrefix(header, `session_token=""`)
	}
	if !removed {
		t.Errorf("the dead session cookie must still be removed: %v", setCookieHeaders(response))
	}
}

func TestLogoutFormAfterTheSessionAlreadyEndedStillSignsOut(t *testing.T) {
	app, csrf := signInThenLoseTheSession(t)
	response := app.PostForm("/logout", url.Values{"_csrf": {csrf}})
	wantStatus(t, response, http.StatusSeeOther)
	if response.Header().Get("Location") != "/login" || !strings.Contains(flashText(t, response), "You have been logged out.") {
		t.Errorf("location = %q", response.Header().Get("Location"))
	}
}

func masqueradeApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.SeedAccount("user1", fixturePassword, models.RoleUser)
	app.Login("admin")
	return app
}

func TestMasquerade(t *testing.T) {
	app := masqueradeApp(t)
	started := app.Do(itestRequest{Method: http.MethodPost, Path: "/admin/users/2/masquerade"})
	wantStatus(t, started, http.StatusSeeOther)
	app.ApplyCSRF(started)
	if got := app.currentUsername(); got != "user1" {
		t.Errorf("while masquerading: %q", got)
	}
	stopped := app.Do(itestRequest{Method: http.MethodPost, Path: "/api/admin/unmasquerade"})
	wantStatus(t, stopped, http.StatusOK)
	app.ApplyCSRF(stopped)
	if got := app.currentUsername(); got != "admin" {
		t.Errorf("after stopping: %q", got)
	}
}

func backdateSession(t *testing.T, app *itestApp, sessionCookie string, age time.Duration) {
	t.Helper()
	token, _, ok := strings.Cut(sessionCookie, ".")
	if !ok {
		t.Fatalf("session cookie %q is not <token>.<hmac>", sessionCookie)
	}
	now := time.Now()
	result, err := app.DB.ExecContext(context.Background(),
		"UPDATE session SET created_at = $1, expires_at = $2 WHERE session_token = $3",
		db.FormatRustRFC3339(now.Add(-age)), db.FormatRustRFC3339(now.Add(-age+7*24*time.Hour)), token)
	if err != nil {
		t.Fatal(err)
	}
	if affected, _ := result.RowsAffected(); affected != 1 {
		t.Fatalf("backdate matched %d session rows", affected)
	}
}

func TestSessionTokenRotatesOncePastTheRefreshWindow(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("alice", nil), http.StatusCreated)
	login := app.loginAttempt("alice", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	app.ApplyCSRF(login)
	original := login.Cookie("session_token").Value

	quiet := app.Get("/api/user")
	wantStatus(t, quiet, http.StatusOK)
	if quiet.Cookie("session_token").Value != original {
		t.Error("a fresh session must not rotate")
	}

	backdateSession(t, app, original, 6*24*time.Hour)
	rotatedResponse := app.Get("/api/user")
	wantStatus(t, rotatedResponse, http.StatusOK)
	rotated := rotatedResponse.Cookie("session_token").Value
	if rotated == original {
		t.Error("session token should have rotated")
	}
	if rotatedResponse.Cookie("csrf_token").Value == "" {
		t.Error("the CSRF cookie must be reissued")
	}
	app.ApplyCSRF(rotatedResponse)
	after := app.Get("/api/user")
	wantStatus(t, after, http.StatusOK)
	if jsonField(t, after, "username") != "alice" || after.Cookie("session_token").Value != rotated {
		t.Error("the rotated session must keep working under the same token")
	}
}

func TestPreRotationTokenStillAuthenticatesDuringGrace(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("bob", nil), http.StatusCreated)
	login := app.loginAttempt("bob", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	original := login.Cookie("session_token").Value
	backdateSession(t, app, original, 6*24*time.Hour)

	withOriginal := func() itestResponse {
		return app.Do(itestRequest{Path: "/api/user", NoCookies: true, Headers: map[string]string{"Cookie": "session_token=" + original}})
	}
	rotated := withOriginal()
	wantStatus(t, rotated, http.StatusOK)
	if rotated.Cookie("session_token").Value == original {
		t.Error("the first request past the window must rotate the token")
	}
	inFlight := withOriginal()
	wantStatus(t, inFlight, http.StatusOK)
	if jsonField(t, inFlight, "username") != "bob" {
		t.Errorf("body = %s", inFlight.Text())
	}
}

func TestMasqueradeRotatesSessionAndCsrfCookies(t *testing.T) {
	app := masqueradeApp(t)
	cookiesBefore := map[string]string{"session": app.cookies["session_token"], "csrf": app.cookies["csrf_token"]}
	started := app.Do(itestRequest{Method: http.MethodPost, Path: "/admin/users/2/masquerade"})
	wantStatus(t, started, http.StatusSeeOther)
	masqSession, masqCSRF := started.Cookie("session_token").Value, started.Cookie("csrf_token").Value
	if masqSession == cookiesBefore["session"] || masqCSRF == cookiesBefore["csrf"] {
		t.Error("starting a masquerade must rotate both cookies")
	}
	app.ApplyCSRF(started)
	stopped := app.Do(itestRequest{Method: http.MethodPost, Path: "/api/admin/unmasquerade"})
	wantStatus(t, stopped, http.StatusOK)
	restoredSession, restoredCSRF := stopped.Cookie("session_token").Value, stopped.Cookie("csrf_token").Value
	if restoredSession == masqSession || restoredSession == cookiesBefore["session"] || restoredCSRF == masqCSRF {
		t.Error("stopping a masquerade must rotate both cookies again")
	}
}

func TestMasqueradeAlreadyMasquerading(t *testing.T) {
	app := masqueradeApp(t)
	first := app.Do(itestRequest{Method: http.MethodPost, Path: "/admin/users/2/masquerade"})
	wantStatus(t, first, http.StatusSeeOther)
	if first.Header().Get("Location") != "/" {
		t.Errorf("location = %q", first.Header().Get("Location"))
	}
	app.ApplyCSRF(first)
	second := app.Do(itestRequest{Method: http.MethodPost, Path: "/admin/users/2/masquerade"})
	wantStatus(t, second, http.StatusSeeOther)
	if second.Header().Get("Location") != "/admin" {
		t.Errorf("location = %q", second.Header().Get("Location"))
	}
}

func TestUnmasqueradeNotMasquerading(t *testing.T) {
	app := signedInApp(t, "admin")
	wantStatus(t, app.Do(itestRequest{Method: http.MethodPost, Path: "/api/admin/unmasquerade"}), http.StatusBadRequest)
}
