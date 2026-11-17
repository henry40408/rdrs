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
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
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

func TestLoginPage(t *testing.T) {
	if body := defaultITestApp(t).GetOK("/login"); !strings.Contains(body, "Login") {
		t.Error("the login page must say Login")
	}
}

func TestSetupPageIsServedOnlyWhileTheInstanceIsEmpty(t *testing.T) {
	app := defaultITestApp(t)
	page := app.Get("/setup")
	wantStatus(t, page, http.StatusOK)
	if !strings.Contains(page.Text(), "setup-form") {
		t.Error("the setup page must carry the setup form")
	}
	app.createUserDirectly("admin", fixturePassword)
	closed := app.Get("/setup")
	wantStatus(t, closed, http.StatusSeeOther)
	if closed.Header().Get("Location") != "/login" {
		t.Errorf("location = %q", closed.Header().Get("Location"))
	}
}

func TestSetupRejectsAShortPasswordOrAnEmptyUsername(t *testing.T) {
	app := defaultITestApp(t)
	for _, attempt := range [][2]string{{"admin", "short"}, {"", fixturePassword}} {
		response := app.PostJSON("/api/setup", map[string]string{"username": attempt[0], "password": attempt[1]})
		if response.Code != http.StatusBadRequest {
			t.Errorf("%q / %q: status = %d", attempt[0], attempt[1], response.Code)
		}
	}
}

func (a *itestApp) setupWithPassword(username, password string) itestResponse {
	return a.PostJSON("/api/setup", map[string]string{"username": username, "password": password})
}

func TestSetupEnforcesThePasswordPolicy(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupWithPassword("shorty", strings.Repeat("a", auth.PasswordMinLength-1)), http.StatusBadRequest)
	wantStatus(t, app.setupWithPassword("longy", strings.Repeat("a", auth.PasswordMaxLength+1)), http.StatusBadRequest)
	// Clears the length gate and is refused by the estimator instead.
	wantStatus(t, app.setupWithPassword("repeater", strings.Repeat("a", auth.PasswordMinLength)), http.StatusBadRequest)
	atMin := "vT7q!mLp2zXc9Rw"
	if len([]rune(atMin)) != auth.PasswordMinLength {
		t.Fatalf("fixture is %d characters", len([]rune(atMin)))
	}
	wantStatus(t, app.setupWithPassword("atmin", atMin), http.StatusCreated)
}

func TestRegistrationRefusesAPasswordBuiltFromTheUsername(t *testing.T) {
	response := defaultITestApp(t).setupWithPassword("marigoldbadger", "marigoldbadger1")
	wantStatus(t, response, http.StatusBadRequest)
	message, _ := jsonField(t, response, "error").(string)
	if message == "" || strings.Contains(message, "at least") || strings.Contains(message, "at most") {
		t.Errorf("want a guessability message, got %q", message)
	}
}

func TestAnOverLongPasswordIsRefusedBeforeItIsHashed(t *testing.T) {
	response := defaultITestApp(t).setupWithPassword("verbose", strings.Repeat("a", 100_000))
	wantStatus(t, response, http.StatusBadRequest)
	if message, _ := jsonField(t, response, "error").(string); !strings.Contains(message, "at most") {
		t.Errorf("want the length policy to reject it, got %s", response.Text())
	}
}

func TestARefusedSetupDoesNotHashThePassword(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("owner", fixturePassword)
	wantStatus(t, app.setupWithPassword("intruder", fixturePassword), http.StatusForbidden)
	user, err := models.FindUserByUsername(context.Background(), app.DB, "intruder")
	if err != nil || user != nil {
		t.Errorf("intruder = %v, err = %v", user, err)
	}
}

func TestUnreadPage(t *testing.T) {
	body := signedInApp(t, "admin").GetOK("/")
	for _, want := range []string{`id="rdrs-sidebar-bootstrap"`, `"username":"admin"`, "data-entries-list"} {
		if !strings.Contains(body, want) {
			t.Errorf("page lacks %s", want)
		}
	}
	if strings.Contains(body, "<rdrs-entries-page>") {
		t.Error("the page must be server rendered, not a CSR shell")
	}
}

func TestPagesRedirectToLoginWhenSignedOut(t *testing.T) {
	app := defaultITestApp(t)
	for _, path := range []string{"/", "/admin"} {
		if got := app.Get(path).Code; got != http.StatusSeeOther {
			t.Errorf("%s: status = %d", path, got)
		}
	}
}

func TestAdminSeesTheAdminPanelAndLink(t *testing.T) {
	app := signedInApp(t, "admin")
	if !strings.Contains(app.GetOK("/admin"), "Admin Panel") {
		t.Error("the admin page must say Admin Panel")
	}
	if !strings.Contains(app.GetOK("/"), `"is_admin":true`) {
		t.Error("the sidebar bootstrap must carry is_admin true")
	}
}

// regularUserApp is signed in as a non-admin "user1" on an instance whose
// admin was set up first.
func regularUserApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.SeedAccount("user1", fixturePassword, models.RoleUser)
	app.Login("user1")
	return app
}

func TestAdminPageForbiddenForRegularUser(t *testing.T) {
	wantStatus(t, regularUserApp(t).Get("/admin"), http.StatusSeeOther)
}

func TestUnreadPageHidesAdminLinkForRegularUser(t *testing.T) {
	body := regularUserApp(t).GetOK("/")
	if strings.Contains(body, `data-testid="nav-admin"`) || strings.Contains(body, `href="/admin"`) {
		t.Error("a regular user must not see the admin link")
	}
}

// signedFlashValue is a flash cookie value as the server would sign it. The
// Go server signs the percent-encoded cookie value (net/http drops raw JSON
// quotes from a Cookie header), so the JSON is escaped first.
func signedFlashValue(rootSecret []byte, payload string) string {
	encoded := base64.RawURLEncoding.EncodeToString([]byte(url.PathEscape(payload)))
	tag := secret.Tag(rootSecret, secret.DomainFlash, []byte(encoded))
	return encoded + "." + base64.RawURLEncoding.EncodeToString(tag[:])
}

func (a *itestApp) signedFlash(payload string) string {
	return signedFlashValue(a.App.Config.Secret, payload)
}

func TestFlashMessageDisplayedOnLoginPage(t *testing.T) {
	app := defaultITestApp(t)
	app.cookies["flash"] = app.signedFlash(`[{"level":"success","message":"Test flash message"}]`)
	body := app.GetOK("/login")
	for _, want := range []string{"Test flash message", "banner--success", `role="status"`} {
		if !strings.Contains(body, want) {
			t.Errorf("page lacks %s", want)
		}
	}
}

func TestAForgedFlashCookieIsNotRendered(t *testing.T) {
	forged := `[{"level":"error","message":"Your account has been suspended."}]`
	wrongKey := []byte(signedFlashValue(itestConfig().Secret, forged))
	last := len(wrongKey) - 1
	wrongKey[last] = 'A'
	if wrongKey[last] == signedFlashValue(itestConfig().Secret, forged)[last] {
		wrongKey[last] = 'B'
	}
	for name, value := range map[string]string{"unsigned": forged, "wrong signature": string(wrongKey)} {
		app := defaultITestApp(t)
		app.cookies["flash"] = value
		if body := app.GetOK("/login"); strings.Contains(body, "Your account has been suspended.") {
			t.Errorf("%s: a forged flash cookie became a banner", name)
		}
	}
}

func TestTheScriptedEndpointsSetTheirOwnFlash(t *testing.T) {
	app := defaultITestApp(t)
	created := app.setupJSON("flashy", nil)
	wantStatus(t, created, http.StatusCreated)
	if !strings.Contains(flashText(t, created), "Account created") {
		t.Error("setup must flash for the client that only navigates")
	}
	signedIn := app.loginAttempt("flashy", fixturePassword)
	wantStatus(t, signedIn, http.StatusOK)
	loggedOut := app.Do(itestRequest{Method: http.MethodDelete, Path: "/api/session",
		Headers: map[string]string{"X-CSRF-Token": signedIn.Cookie("csrf_token").Value}})
	wantStatus(t, loggedOut, http.StatusOK)
	if !strings.Contains(flashText(t, loggedOut), "You have been logged out.") {
		t.Error("logout must flash")
	}
}

func TestFlashMessageClearedAfterDisplay(t *testing.T) {
	app := defaultITestApp(t)
	app.cookies["flash"] = app.signedFlash(`[{"level":"info","message":"First message"}]`)
	if !strings.Contains(app.GetOK("/login"), "First message") {
		t.Fatal("the flash must show once")
	}
	if strings.Contains(app.GetOK("/login"), "First message") {
		t.Error("the flash must be cleared after display")
	}
}

func TestFlashMessageOnUnreadPage(t *testing.T) {
	app := signedInApp(t, "admin")
	app.cookies["flash"] = app.signedFlash(`[{"level":"warning","message":"Warning test"}]`)
	body := app.GetOK("/")
	for _, want := range []string{"<rdrs-flash", `data-testid="flash-message"`, "Warning test", "banner--warning", `role="alert"`} {
		if !strings.Contains(body, want) {
			t.Errorf("page lacks %s", want)
		}
	}
}

// ageSession puts the only session in the refresh window: created five days
// ago and expiring in two hours. It returns the aged expiry.
func ageSession(t *testing.T, app *itestApp) time.Time {
	t.Helper()
	now := time.Now()
	expiry := now.Add(2 * time.Hour)
	if _, err := app.DB.ExecContext(context.Background(), "UPDATE session SET created_at = $1, expires_at = $2",
		db.FormatRustRFC3339(now.Add(-5*24*time.Hour)), db.FormatRustRFC3339(expiry)); err != nil {
		t.Fatal(err)
	}
	return expiry
}

func readExpiry(t *testing.T, app *itestApp) time.Time {
	t.Helper()
	var token string
	if err := app.DB.QueryRowContext(context.Background(), "SELECT session_token FROM session LIMIT 1").Scan(&token); err != nil {
		t.Fatal(err)
	}
	session, err := models.FindSessionByToken(context.Background(), app.DB, token)
	if err != nil || session == nil {
		t.Fatalf("session = %v, err = %v", session, err)
	}
	return session.ExpiresAt
}

func TestAPIRequestSlidesSessionExpiryForward(t *testing.T) {
	app := signedInApp(t, "admin")
	aged := ageSession(t, app)
	wantStatus(t, app.Get("/api/user"), http.StatusOK)
	refreshed := readExpiry(t, app)
	if !refreshed.After(aged) {
		t.Errorf("expiry should slide forward: aged=%v refreshed=%v", aged, refreshed)
	}
	if min := time.Now().Add(6 * 24 * time.Hour); refreshed.Before(min) {
		t.Errorf("refreshed expiry too small: %v < %v", refreshed, min)
	}
}

func TestPageRequestSlidesSessionExpiryForward(t *testing.T) {
	app := signedInApp(t, "admin")
	aged := ageSession(t, app)
	wantStatus(t, app.Get("/"), http.StatusOK)
	if refreshed := readExpiry(t, app); !refreshed.After(aged) {
		t.Errorf("page request should slide expiry forward: aged=%v refreshed=%v", aged, refreshed)
	}
}

func TestDisableLocalAuthBlocksPasswordLogin(t *testing.T) {
	cfg := itestConfig()
	cfg.DisableLocalAuth = true
	cfg.AuthProxyHeader = "Remote-User"
	networks, err := config.ParseTrustedNetworks("127.0.0.0/8")
	if err != nil {
		t.Fatal(err)
	}
	cfg.TrustedProxyNetworks = networks
	app := newITestApp(t, cfg)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	wantStatus(t, app.loginAttempt("u", fixturePassword), http.StatusForbidden)
}

// loginSessionCookie sets a fresh user up, signs it in and returns the raw
// session Set-Cookie value under whichever name cookie_secure selects.
func loginSessionCookie(t *testing.T, app *itestApp) string {
	t.Helper()
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	for _, header := range setCookieHeaders(login) {
		if strings.HasPrefix(header, "session_token=") || strings.HasPrefix(header, "__Host-session_token=") {
			return header
		}
	}
	t.Fatal("login must emit a session_token or __Host-session_token Set-Cookie")
	return ""
}

func secureConfig() config.Config {
	cfg := itestConfig()
	cfg.CookieSecure = true
	return cfg
}

func TestSessionCookieSecureWhenEnabled(t *testing.T) {
	cookie := loginSessionCookie(t, newITestApp(t, secureConfig()))
	if !strings.HasPrefix(cookie, "__Host-session_token=") {
		t.Errorf("cookie_secure must select the __Host- prefixed name: %s", cookie)
	}
	for _, want := range []string{"Secure", "HttpOnly", "SameSite=Lax", "Path=/"} {
		if !strings.Contains(cookie, want) {
			t.Errorf("cookie lacks %s: %s", want, cookie)
		}
	}
	if strings.Contains(strings.ToLower(cookie), "domain=") {
		t.Errorf("a __Host- cookie must not carry a Domain attribute: %s", cookie)
	}
}

func TestSessionCookieNotSecureByDefault(t *testing.T) {
	cookie := loginSessionCookie(t, defaultITestApp(t))
	if !strings.HasPrefix(cookie, "session_token=") {
		t.Errorf("default config must stay on the unprefixed name: %s", cookie)
	}
	if strings.Contains(cookie, "Secure") {
		t.Errorf("session cookie must not be Secure without cookie_secure: %s", cookie)
	}
}

func TestUnprefixedCookieStillAuthenticatesWhenSecureEnabled(t *testing.T) {
	cfg := secureConfig()
	app := newITestApp(t, cfg)
	ctx := context.Background()
	user, err := models.CreateUser(ctx, app.DB, "u", "!", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	session, err := models.CreateSession(ctx, app.DB, models.NewSession{UserID: user.ID, UserAgent: "test-agent", IPAddress: "127.0.0.1"})
	if err != nil {
		t.Fatal(err)
	}
	oldStyle := secret.SignSession(cfg.Secret, session.SessionToken)
	response := app.Do(itestRequest{Path: "/api/user", NoCookies: true, Headers: map[string]string{"Cookie": "session_token=" + oldStyle}})
	wantStatus(t, response, http.StatusOK)
}

func TestPrefixedCookieIsNeverEmittedWithoutSecure(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	for _, header := range setCookieHeaders(login) {
		if strings.HasPrefix(header, "__Host-") {
			t.Errorf("no __Host- cookie may be emitted when cookie_secure is false: %s", header)
		}
	}
}

func TestSessionCookieMaxAgeMatchesSlidingTTL(t *testing.T) {
	cookie := loginSessionCookie(t, defaultITestApp(t))
	if !strings.Contains(cookie, "Max-Age=604800") || strings.Contains(cookie, "Max-Age=7776000") {
		t.Errorf("Max-Age must be the 7-day sliding TTL: %s", cookie)
	}
}

func TestCSRFCookieMaxAgeMatchesSessionCookie(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	for _, header := range setCookieHeaders(login) {
		if !strings.HasPrefix(header, "csrf_token=") {
			continue
		}
		if !strings.Contains(header, "Max-Age=604800") || strings.Contains(header, "Max-Age=7776000") {
			t.Errorf("csrf cookie Max-Age must mirror the session cookie: %s", header)
		}
		return
	}
	t.Error("login must emit a csrf_token Set-Cookie")
}

// cookiesNamed returns the Set-Cookie headers whose name starts with prefix
// (including the "=").
func cookiesNamed(response itestResponse, prefix string) []string {
	var found []string
	for _, header := range setCookieHeaders(response) {
		if strings.HasPrefix(header, prefix) {
			found = append(found, header)
		}
	}
	return found
}

func TestLogoutClearsCookieWithPath(t *testing.T) {
	app := signedInApp(t, "u")
	response := app.deleteSession()
	wantStatus(t, response, http.StatusOK)
	removal := cookiesNamed(response, "session_token=")
	if len(removal) == 0 || !strings.Contains(removal[0], "Path=/") {
		t.Fatalf("the removal cookie must carry Path=/: %v", setCookieHeaders(response))
	}
	// Both names for both purposes, so a leftover from before a config flip
	// is always cleared.
	for _, name := range []string{"session_token=", "csrf_token=", "__Host-session_token=", "__Host-csrf_token="} {
		found := cookiesNamed(response, name)
		if len(found) == 0 {
			t.Fatalf("logout must emit a %s removal: %v", name, setCookieHeaders(response))
		}
		if strings.HasPrefix(name, "__Host-") && (!strings.Contains(found[0], "Secure") || !strings.Contains(found[0], "Path=/")) {
			t.Errorf("a __Host- removal must carry Secure and Path=/: %s", found[0])
		}
	}
}

func cookieValueOf(setCookie, name string) string {
	value, _, _ := strings.Cut(strings.TrimPrefix(setCookie, name), ";")
	return value
}

func TestLogoutRemovalCookieIsNotOverwrittenBySlide(t *testing.T) {
	response := signedInApp(t, "u").deleteSession()
	wantStatus(t, response, http.StatusOK)
	for _, name := range []string{"session_token=", "csrf_token="} {
		found := cookiesNamed(response, name)
		if len(found) != 1 {
			t.Fatalf("logout must emit exactly one %s Set-Cookie: %v", name, setCookieHeaders(response))
		}
		if cookieValueOf(found[0], name) != "" {
			t.Errorf("the slide must not overwrite the removal: %s", found[0])
		}
	}
}

func TestLogoutClearsPrefixedCookiesWhenSecureEnabled(t *testing.T) {
	app := newITestApp(t, secureConfig())
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	app.headers = http.Header{"X-Csrf-Token": {login.Cookie("__Host-csrf_token").Value}}
	response := app.deleteSession()
	wantStatus(t, response, http.StatusOK)
	for _, name := range []string{"__Host-session_token=", "__Host-csrf_token="} {
		found := cookiesNamed(response, name)
		if len(found) != 1 {
			t.Fatalf("expected exactly one %s Set-Cookie (the removal): %v", name, setCookieHeaders(response))
		}
		if cookieValueOf(found[0], name) != "" || !strings.Contains(found[0], "Secure") {
			t.Errorf("want an empty Secure removal: %s", found[0])
		}
	}
}

func TestTamperedSessionCookieIsRejected(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	value := login.Cookie("session_token").Value
	cut := strings.LastIndex(value, ".")
	token, signature := value[:cut], value[cut+1:]
	withCookie := func(cookie string) itestResponse {
		return app.Do(itestRequest{Path: "/api/user", NoCookies: true, Headers: map[string]string{"Cookie": "session_token=" + cookie}})
	}
	wantStatus(t, withCookie(value), http.StatusOK)
	// Token changed, signature kept: rejected before any database lookup.
	wantStatus(t, withCookie(token+"x."+signature), http.StatusUnauthorized)
	// Signature stripped (a raw database token): also rejected.
	wantStatus(t, withCookie(token), http.StatusUnauthorized)
}

func TestLogoutRedirectDefaultIsLogin(t *testing.T) {
	response := signedInApp(t, "u").deleteSession()
	if jsonField(t, response, "redirect_to") != "/login" || jsonField(t, response, "logout_url_configured") != false {
		t.Errorf("body = %s", response.Text())
	}
}

func logoutWithConfiguredURL(t *testing.T, logoutURL string) itestResponse {
	t.Helper()
	cfg := itestConfig()
	cfg.AuthProxyLogoutURL = logoutURL
	app := newITestApp(t, cfg)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	app.Login("u")
	return app.deleteSession()
}

func TestLogoutRedirectUsesConfiguredURL(t *testing.T) {
	response := logoutWithConfiguredURL(t, "https://auth.example.com/logout")
	if jsonField(t, response, "redirect_to") != "https://auth.example.com/logout" || jsonField(t, response, "logout_url_configured") != true {
		t.Errorf("body = %s", response.Text())
	}
}

func TestLogoutRedirectUsesRelativeConfiguredURL(t *testing.T) {
	response := logoutWithConfiguredURL(t, "/logout")
	if jsonField(t, response, "redirect_to") != "/logout" || jsonField(t, response, "logout_url_configured") != true {
		t.Errorf("body = %s", response.Text())
	}
}

func TestLogoutSendsClearSiteData(t *testing.T) {
	response := signedInApp(t, "u").deleteSession()
	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Clear-Site-Data"); got != `"cache", "storage"` {
		t.Errorf("Clear-Site-Data = %q", got)
	}
}

// "cookies" would race the flash cookie written right after the response and
// "executionContexts" would fight the client's own redirect.
func TestLogoutClearSiteDataOmitsCookies(t *testing.T) {
	header := signedInApp(t, "u").deleteSession().Header().Get("Clear-Site-Data")
	if header == "" || strings.Contains(header, "cookies") || strings.Contains(header, "executionContexts") {
		t.Errorf("Clear-Site-Data = %q", header)
	}
}

func TestLoginPageRedirectsAuthenticatedToRoot(t *testing.T) {
	response := signedInApp(t, "u").Get("/login")
	if response.Code < 300 || response.Code >= 400 || response.Header().Get("Location") != "/" {
		t.Errorf("status = %d, location = %q", response.Code, response.Header().Get("Location"))
	}
}

func TestLoginPageRendersWhenAnonymous(t *testing.T) {
	body := defaultITestApp(t).GetOK("/login")
	if !strings.Contains(body, "login-form") && !strings.Contains(body, "rdrs") {
		t.Error("the anonymous login page must render the form")
	}
}

func TestFreshSessionIsNotRefreshed(t *testing.T) {
	app := signedInApp(t, "admin")
	before := readExpiry(t, app)
	wantStatus(t, app.Get("/api/user"), http.StatusOK)
	if after := readExpiry(t, app); !before.Equal(after) {
		t.Errorf("a fresh session must not be refreshed: %v -> %v", before, after)
	}
}

func TestAuthenticatedRequestReissuesSessionCookie(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("u", nil), http.StatusCreated)
	login := app.loginAttempt("u", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	app.ApplyCSRF(login)
	response := app.Get("/api/user")
	wantStatus(t, response, http.StatusOK)
	reissued := cookiesNamed(response, "session_token=")
	if len(reissued) == 0 {
		t.Fatal("an ordinary authenticated request must reissue the session cookie")
	}
	if cookieValueOf(reissued[0], "session_token=") != login.Cookie("session_token").Value {
		t.Error("the session token must not be rotated when its cookie is slid")
	}
	if !strings.Contains(reissued[0], "Max-Age=604800") {
		t.Errorf("the reissued cookie must carry the refreshed Max-Age: %s", reissued[0])
	}
}

func TestStaticAssetResponseHasNoSetCookie(t *testing.T) {
	app := signedInApp(t, "u")
	if cookies := setCookieHeaders(app.Get("/static/nonexistent.css")); len(cookies) != 0 {
		t.Errorf("a static asset response must never carry Set-Cookie: %v", cookies)
	}
}
