package server

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

// Port of tests/forward_auth_test.rs. httptest requests come from 192.0.2.1,
// so that address stands in for the loopback peer of the Rust suite.

const (
	trustedPeerNetwork   = "192.0.2.0/24"
	untrustedPeerNetwork = "10.0.0.0/8"
)

func forwardAuthConfig(trusted string) config.Config {
	cfg := itestConfig()
	cfg.AuthProxyHeader = "Remote-User"
	cfg.AuthProxyGroupsHeader = "Remote-Groups"
	cfg.AuthProxyAdminGroup = "admins"
	cfg.TrustedProxyNetworks = []netip.Prefix{netip.MustParsePrefix(trusted)}
	return cfg
}

func forwardAuthApp(t *testing.T, mutate func(*config.Config)) *itestApp {
	t.Helper()
	cfg := forwardAuthConfig(trustedPeerNetwork)
	if mutate != nil {
		mutate(&cfg)
	}
	return newITestApp(t, cfg)
}

func (a *itestApp) seedForwardUser(name string, role models.Role) *models.User {
	a.t.Helper()
	user, err := models.CreateUser(context.Background(), a.DB, name, "!", role)
	if err != nil {
		a.t.Fatal(err)
	}
	return user
}

func (a *itestApp) mustFindUser(name string) *models.User {
	a.t.Helper()
	user, err := models.FindUserByUsername(context.Background(), a.DB, name)
	if err != nil || user == nil {
		a.t.Fatalf("user %q: %v %v", name, user, err)
	}
	return user
}

func proxyHeaders(pairs ...string) map[string]string {
	headers := map[string]string{}
	for i := 0; i < len(pairs); i += 2 {
		headers[pairs[i]] = pairs[i+1]
	}
	return headers
}

func (a *itestApp) getAs(path string, pairs ...string) itestResponse {
	return a.Do(itestRequest{Path: path, Headers: proxyHeaders(pairs...)})
}

func isRedirect(response itestResponse) bool { return response.Code >= 300 && response.Code < 400 }

func TestForwardAuthIdentity(t *testing.T) {
	cfg := forwardAuthConfig("10.0.0.0/8")
	request := func(peer, header string) *http.Request {
		r := httptest.NewRequest(http.MethodGet, "/", nil)
		r.RemoteAddr = peer
		if header != "" {
			r.Header.Set("Remote-User", header)
		}
		return r
	}
	off := cfg
	off.AuthProxyHeader = ""
	cases := []struct {
		name string
		cfg  config.Config
		r    *http.Request
		want string
	}{
		{"trusted peer with the header", cfg, request("10.1.2.3:5000", "alice"), "alice"},
		{"untrusted peer", cfg, request("192.168.0.1:5000", "alice"), ""},
		{"no peer address", cfg, request("", "alice"), ""},
		{"header missing", cfg, request("10.1.2.3:5000", ""), ""},
		{"header blank", cfg, request("10.1.2.3:5000", "  "), ""},
		{"feature off", off, request("10.1.2.3:5000", "alice"), ""},
	}
	for _, c := range cases {
		if got := auth.ForwardAuthIdentity(c.cfg, c.r); got != c.want {
			t.Errorf("%s: identity = %q, want %q", c.name, got, c.want)
		}
	}
}

func TestTrustedExistingUserGetsSession(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("alice", models.RoleUser)
	response := app.getAs("/", "Remote-User", "alice")
	if !isRedirect(response) || response.Cookie("session_token") == nil {
		t.Errorf("want a redirect with a fresh session cookie: %d %v", response.Code, setCookieHeaders(response))
	}
}

func TestTrustedUserGetsPrefixedSessionCookieWhenSecureEnabled(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) { c.CookieSecure = true })
	app.seedForwardUser("alice", models.RoleUser)
	response := app.getAs("/", "Remote-User", "alice")
	if !isRedirect(response) {
		t.Fatalf("status = %d", response.Code)
	}
	if response.Cookie("__Host-session_token") == nil {
		t.Error("forward-auth must mint the __Host- prefixed session cookie")
	}
	if response.Cookie("session_token") != nil {
		t.Error("forward-auth must not also mint the unprefixed cookie")
	}
}

func TestUntrustedPeerIgnoresHeader(t *testing.T) {
	app := newITestApp(t, forwardAuthConfig(untrustedPeerNetwork))
	app.seedForwardUser("alice", models.RoleUser)
	response := app.getAs("/", "Remote-User", "alice")
	if !isRedirect(response) || response.Header().Get("Location") != "/login" {
		t.Errorf("want a redirect to /login: %d %q", response.Code, response.Header().Get("Location"))
	}
}

func TestUnknownUserCreationDisabledRejected(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) { c.AuthProxyUserCreation = false })
	response := app.getAs("/", "Remote-User", "ghost")
	if response.Cookie("session_token") != nil || response.Header().Get("Location") != "/login" {
		t.Errorf("want no session and /login: %d %v", response.Code, setCookieHeaders(response))
	}
}

func TestUnknownUserJitCreatedAsAdminViaGroups(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) { c.AuthProxyUserCreation = true })
	response := app.getAs("/", "Remote-User", "bob", "Remote-Groups", "users,admins")
	if !isRedirect(response) || response.Cookie("session_token") == nil {
		t.Fatalf("want a redirect with a session: %d", response.Code)
	}
	if role := app.mustFindUser("bob").Role; role != models.RoleAdmin {
		t.Errorf("role = %s, want admin", role)
	}
}

func TestJitWithoutGroupMappingMakesOnlyTheFirstUserAdmin(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) {
		c.AuthProxyUserCreation = true
		c.AuthProxyGroupsHeader = ""
	})
	for _, name := range []string{"first", "second"} {
		// A valid session cookie would skip JIT creation for the next name.
		app.cookies = map[string]string{}
		if response := app.getAs("/", "Remote-User", name); !isRedirect(response) {
			t.Fatalf("%s: status = %d", name, response.Code)
		}
	}
	first, second := app.mustFindUser("first"), app.mustFindUser("second")
	if first.Role != models.RoleAdmin || second.Role != models.RoleUser {
		t.Errorf("roles = %s, %s", first.Role, second.Role)
	}
	categories, err := models.ListCategoriesByUser(context.Background(), app.DB, first.ID)
	if err != nil || len(categories) != 1 || categories[0].Name != "Uncategorized" {
		t.Errorf("categories = %v %v", categories, err)
	}
}

func TestDisabledUserRejected(t *testing.T) {
	app := forwardAuthApp(t, nil)
	carol := app.seedForwardUser("carol", models.RoleUser)
	if err := models.DisableUser(context.Background(), app.DB, carol.ID); err != nil {
		t.Fatal(err)
	}
	response := app.getAs("/", "Remote-User", "carol")
	if response.Cookie("session_token") != nil || response.Header().Get("Location") != "/login" {
		t.Errorf("want no session and /login: %d", response.Code)
	}
}

func TestExistingUserRoleRecomputedOnLogin(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("dave", models.RoleAdmin)
	response := app.getAs("/", "Remote-User", "dave", "Remote-Groups", "users")
	if !isRedirect(response) || response.Cookie("session_token") == nil {
		t.Fatalf("want a redirect with a session: %d", response.Code)
	}
	if role := app.mustFindUser("dave").Role; role != models.RoleUser {
		t.Errorf("role = %s, want demoted to user", role)
	}
}

func TestInvalidSessionCookieStillForwardAuths(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("erin", models.RoleUser)
	app.cookies["session_token"] = "stale-invalid"
	response := app.getAs("/", "Remote-User", "erin")
	fresh := response.Cookie("session_token")
	if !isRedirect(response) || fresh == nil || fresh.Value == "stale-invalid" || fresh.Value == "" {
		t.Errorf("a fresh session cookie must be minted: %d %v", response.Code, setCookieHeaders(response))
	}
}

func sidebarViaForwardAuth(t *testing.T, response itestResponse) bool {
	t.Helper()
	if response.Code != http.StatusOK {
		t.Fatalf("sidebar = %d", response.Code)
	}
	var body struct {
		ViaForwardAuth bool `json:"via_forward_auth"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	return body.ViaForwardAuth
}

func TestSidebarReportsViaForwardAuthDynamically(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("grace", models.RoleUser)
	app.getAs("/", "Remote-User", "grace")
	if !sidebarViaForwardAuth(t, app.getAs("/api/sidebar", "Remote-User", "grace")) {
		t.Error("with the proxy header, via_forward_auth must be true")
	}
	if sidebarViaForwardAuth(t, app.Get("/api/sidebar")) {
		t.Error("without the proxy header on the request, via_forward_auth must be false")
	}
}

func TestAdminPageSidebarBootstrapReportsViaForwardAuth(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("heidi", models.RoleAdmin)
	app.getAs("/", "Remote-User", "heidi", "Remote-Groups", "admins")
	response := app.getAs("/admin", "Remote-User", "heidi", "Remote-Groups", "admins")
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d", response.Code)
	}
	if !strings.Contains(response.Text(), `"via_forward_auth":true`) {
		t.Errorf("sidebar bootstrap lacks via_forward_auth:true: %.500s", response.Text())
	}
}

func TestValidSessionCookieNotReminted(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("frank", models.RoleUser)
	first := app.getAs("/", "Remote-User", "frank")
	original := first.Cookie("session_token")
	if original == nil {
		t.Fatal("forward-auth login must mint a session cookie")
	}
	response := app.getAs("/", "Remote-User", "frank")
	if reissued := response.Cookie("session_token"); reissued != nil && reissued.Value != original.Value {
		t.Error("a valid session must not be re-minted with a different token")
	}
	if response.Header().Get("Location") == "/login" {
		t.Error("a valid session must not be redirected to /login")
	}
}

func TestForwardAuthCookieIsNotOverwrittenBySlide(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("olga", models.RoleUser)
	response := app.getAs("/", "Remote-User", "olga")
	if !isRedirect(response) {
		t.Fatalf("status = %d", response.Code)
	}
	count := 0
	for _, header := range setCookieHeaders(response) {
		if strings.HasPrefix(header, "session_token=") {
			count++
		}
	}
	if count != 1 {
		t.Errorf("session_token Set-Cookie count = %d, want 1", count)
	}
}

type logoutBody struct {
	ViaForwardAuth      bool   `json:"via_forward_auth"`
	RedirectTo          string `json:"redirect_to"`
	LogoutURLConfigured bool   `json:"logout_url_configured"`
}

func (a *itestApp) logout(headers map[string]string) (itestResponse, logoutBody) {
	a.t.Helper()
	response := a.Do(itestRequest{Method: http.MethodDelete, Path: "/api/session", Headers: headers})
	if response.Code != http.StatusOK {
		a.t.Fatalf("logout = %d: %s", response.Code, response.Text())
	}
	var body logoutBody
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		a.t.Fatal(err)
	}
	return response, body
}

func TestLogoutReportsViaForwardAuth(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("ivan", models.RoleUser)
	app.ApplyCSRF(app.getAs("/", "Remote-User", "ivan"))
	_, body := app.logout(proxyHeaders("Remote-User", "ivan"))
	if !body.ViaForwardAuth || body.RedirectTo != "/login" || body.LogoutURLConfigured {
		t.Errorf("body = %+v", body)
	}
}

func TestLogoutAfterTheSessionEndedStillReportsViaForwardAuth(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("kate", models.RoleUser)
	app.ApplyCSRF(app.getAs("/", "Remote-User", "kate"))
	if err := models.DeleteUserSessions(context.Background(), app.DB, app.mustFindUser("kate").ID); err != nil {
		t.Fatal(err)
	}
	response, body := app.logout(proxyHeaders("Remote-User", "kate"))
	if !body.ViaForwardAuth || !strings.Contains(flashText(t, response), "log out at your proxy") {
		t.Errorf("body = %+v", body)
	}
}

func TestLogoutPasswordSessionReportsViaForwardAuthFalse(t *testing.T) {
	app := forwardAuthApp(t, nil)
	if response := app.PostJSON("/api/setup", map[string]string{"username": "judy", "password": fixturePassword}); response.Code != http.StatusCreated {
		t.Fatalf("setup = %d", response.Code)
	}
	app.Login("judy")
	_, body := app.logout(nil)
	if body.ViaForwardAuth || body.RedirectTo != "/login" || body.LogoutURLConfigured {
		t.Errorf("body = %+v", body)
	}
}

func ageEverySession(t *testing.T, database *db.DB) {
	t.Helper()
	stale := db.FormatRustRFC3339(time.Now().Add(-time.Hour))
	if _, err := database.ExecContext(context.Background(), "UPDATE session SET last_authenticated_at = $1", stale); err != nil {
		t.Fatal(err)
	}
}

// The proxy re-asserts the identity on every request, so there is no password
// to re-check and the re-authentication window must not apply.
func TestForwardAuthSessionIsExemptFromReauthentication(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("wendy", models.RoleUser)
	app.ApplyCSRF(app.getAs("/", "Remote-User", "wendy"))
	ageEverySession(t, app.DB)
	response := app.Do(itestRequest{Method: http.MethodPost, Path: "/api/passkey/register/start", Headers: proxyHeaders("Remote-User", "wendy")})
	if response.Code != http.StatusOK {
		t.Errorf("status = %d: %s", response.Code, response.Text())
	}
}

func TestReauthOnForwardAuthSessionRefreshesWithoutAPassword(t *testing.T) {
	app := forwardAuthApp(t, nil)
	app.seedForwardUser("xavier", models.RoleUser)
	app.ApplyCSRF(app.getAs("/", "Remote-User", "xavier"))
	ageEverySession(t, app.DB)
	headers := proxyHeaders("Remote-User", "xavier", "Content-Type", "application/json")
	response := app.Do(itestRequest{Method: http.MethodPost, Path: "/api/session/reauth", Body: "{}", Headers: headers})
	if response.Code != http.StatusNoContent {
		t.Errorf("status = %d: %s", response.Code, response.Text())
	}
}

func TestJitRefusesOverlongUsernameWithoutError(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) { c.AuthProxyUserCreation = true })
	response := app.getAs("/", "Remote-User", strings.Repeat("a", 65))
	if !isRedirect(response) || response.Header().Get("Location") != "/login" || response.Cookie("session_token") != nil {
		t.Errorf("want a plain redirect to /login: %d", response.Code)
	}
	if count, err := models.CountUsers(context.Background(), app.DB); err != nil || count != 0 {
		t.Errorf("users = %d %v", count, err)
	}
}

func TestJitAcceptsSsoStyleUsername(t *testing.T) {
	app := forwardAuthApp(t, func(c *config.Config) { c.AuthProxyUserCreation = true })
	response := app.getAs("/", "Remote-User", `CORP\Alice Smith`)
	if response.Cookie("session_token") == nil {
		t.Errorf("want a session: %d", response.Code)
	}
	if user, err := models.FindUserByUsername(context.Background(), app.DB, `CORP\Alice Smith`); err != nil || user == nil {
		t.Errorf("user not provisioned: %v %v", user, err)
	}
}
