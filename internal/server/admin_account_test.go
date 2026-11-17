package server

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
)

// pageAfter requests path with the flash cookie that rec set.
func (f *signedInFixture) pageAfter(path string, rec *httptest.ResponseRecorder) string {
	request := httptest.NewRequest(http.MethodGet, path, nil)
	request.AddCookie(f.session)
	if c := cookieNamed(rec, flash.CookieName); c != nil {
		request.AddCookie(c)
	}
	return f.do(request).Body.String()
}

// expectFlash posts a form and checks the redirect and the banner it shows.
func (f *signedInFixture) expectFlash(t *testing.T, path string, fields url.Values, location, text string) {
	t.Helper()
	rec := f.postForm(path, fields)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != location {
		t.Fatalf("POST %s = %d %q, want 303 %q", path, rec.Code, rec.Header().Get("Location"), location)
	}
	if page := f.pageAfter(location, rec); !strings.Contains(page, text) {
		t.Errorf("POST %s: page lacks %q", path, text)
	}
}

func (f *signedInFixture) makeUser(t *testing.T, name string) *models.User {
	t.Helper()
	hash, err := auth.HashPassword(testPassword)
	if err != nil {
		t.Fatal(err)
	}
	user, err := models.CreateUser(context.Background(), f.database, name, hash, models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	return user
}

func (f *signedInFixture) adminID(t *testing.T) int64 {
	t.Helper()
	user, err := models.FindUserByUsername(context.Background(), f.database, "root")
	if err != nil || user == nil {
		t.Fatal(err)
	}
	return user.ID
}

func idPath(format string, id int64) string {
	return strings.Replace(format, "{id}", strconv.FormatInt(id, 10), 1)
}

func TestAdminCreateUserIssuesAnInviteLinkOnce(t *testing.T) {
	f := newSignedIn(t)
	rec := f.postForm("/admin/users", url.Values{"username": {"bob"}, "role": {"user"}})
	page := f.pageAfter("/admin", rec)
	if !strings.Contains(page, "Account link ready.") || !strings.Contains(page, "/invite/") {
		t.Fatalf("page lacks the invite link:\n%.300s", page)
	}
	bob, err := models.FindUserByUsername(context.Background(), f.database, "bob")
	if err != nil || bob == nil || bob.PasswordHash != noPasswordHash {
		t.Fatalf("bob = %+v, %v", bob, err)
	}
	if again := f.get("/admin").Body.String(); strings.Contains(again, "Account link ready.") {
		t.Error("the one-time link was shown twice")
	}
	f.expectFlash(t, "/admin/users", url.Values{"username": {"bob"}, "role": {"user"}}, "/admin", "That username is already taken.")
}

func TestAdminCannotChangeItsOwnRoleStatusOrAccount(t *testing.T) {
	f := newSignedIn(t)
	self := f.adminID(t)
	f.expectFlash(t, idPath("/admin/users/{id}/role", self), url.Values{"role": {"user"}}, "/admin", "You cannot modify your own role.")
	f.expectFlash(t, idPath("/admin/users/{id}/status", self), url.Values{"disabled": {"true"}}, "/admin", "You cannot modify your own status.")
	f.expectFlash(t, idPath("/admin/users/{id}/delete", self), nil, "/admin", "You cannot delete your own account.")
}

func TestAdminFormsNeedRecentPasswordConfirmation(t *testing.T) {
	f := newSignedIn(t)
	if _, err := f.database.ExecContext(context.Background(), "UPDATE session SET last_authenticated_at = NULL"); err != nil {
		t.Fatal(err)
	}
	create := url.Values{"username": {"carol"}, "role": {"user"}}
	f.expectFlash(t, "/admin/users", create, "/admin", "Confirm your password before changing accounts, then try again.")
	if carol, _ := models.FindUserByUsername(context.Background(), f.database, "carol"); carol != nil {
		t.Fatal("the refused form still created the account")
	}
	f.expectFlash(t, "/admin/reauth", url.Values{"password": {"wrong password entirely"}}, "/admin", "Incorrect password.")
	f.expectFlash(t, "/admin/reauth", url.Values{"password": {testPassword}}, "/admin", "Confirmed. You can now change accounts.")
	f.expectFlash(t, "/admin/users", create, "/admin", "Account link ready.")
}

func TestAdminReissuesAndRevokesInvites(t *testing.T) {
	f := newSignedIn(t)
	bob := f.makeUser(t, "bob")
	f.expectFlash(t, idPath("/admin/users/{id}/invite/revoke", bob.ID), nil, "/admin", "There is no outstanding link for that account.")
	f.expectFlash(t, idPath("/admin/users/{id}/invite", bob.ID), nil, "/admin", "Account link ready.")
	if invite, _ := models.FindLiveInviteForUser(context.Background(), f.database, bob.ID); invite == nil {
		t.Fatal("no live invite after issuing")
	}
	f.expectFlash(t, idPath("/admin/users/{id}/invite/revoke", bob.ID), nil, "/admin", "Link revoked.")
	if invite, _ := models.FindLiveInviteForUser(context.Background(), f.database, bob.ID); invite != nil {
		t.Fatal("invite survived revoking")
	}
}

func TestDisabledUserCannotBeMasqueraded(t *testing.T) {
	f := newSignedIn(t)
	bob := f.makeUser(t, "bob")
	f.expectFlash(t, idPath("/admin/users/{id}/status", bob.ID), url.Values{"disabled": {"true"}}, "/admin", "User disabled.")
	f.expectFlash(t, idPath("/admin/users/{id}/masquerade", bob.ID), nil, "/admin", "Failed to start masquerade.")
}

func TestMasqueradeHidesCredentialsAndCanBeStopped(t *testing.T) {
	f := newSignedIn(t)
	bob := f.makeUser(t, "bob")
	rec := f.postForm(idPath("/admin/users/{id}/masquerade", bob.ID), nil)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/" {
		t.Fatalf("masquerade = %d %q", rec.Code, rec.Header().Get("Location"))
	}
	session, csrf := cookieNamed(rec, auth.SessionCookieName), cookieNamed(rec, auth.CSRFCookieName)
	if session == nil || csrf == nil || session.Value == f.session.Value {
		t.Fatal("the session token was not rotated")
	}
	as := func(method, path string, fields url.Values) *httptest.ResponseRecorder {
		request := httptest.NewRequest(method, path, strings.NewReader(fields.Encode()))
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		request.Header.Set("X-CSRF-Token", csrf.Value)
		request.AddCookie(session)
		return f.do(request)
	}
	blocked := as(http.MethodPost, "/user-settings/sessions/revoke-others", nil)
	request := httptest.NewRequest(http.MethodGet, "/user-settings", nil)
	request.AddCookie(session)
	request.AddCookie(cookieNamed(blocked, flash.CookieName))
	page := f.do(request).Body.String()
	if !strings.Contains(page, "Session management is unavailable while masquerading.") {
		t.Error("revoking while masquerading was not refused")
	}
	if strings.Contains(page, "Active Sessions") {
		t.Error("the target's sessions are listed while masquerading")
	}
	if again := as(http.MethodPost, "/admin/users/"+strconv.FormatInt(bob.ID, 10)+"/masquerade", nil); again.Header().Get("Location") != "/admin" {
		t.Errorf("a second masquerade went to %q", again.Header().Get("Location"))
	}
	stop := as(http.MethodPost, "/api/admin/unmasquerade", nil)
	if stop.Code != http.StatusOK || cookieNamed(stop, auth.SessionCookieName) == nil {
		t.Fatalf("unmasquerade = %d", stop.Code)
	}
}

func TestNonAdminCannotUseAdminForms(t *testing.T) {
	f := newSignedIn(t)
	bob := f.makeUser(t, "bob")
	login := f.do(jsonRequest("POST", "/api/session", `{"username":"bob","password":"`+testPassword+`"}`))
	request := httptest.NewRequest(http.MethodPost, idPath("/admin/users/{id}/delete", f.adminID(t)), nil)
	request.Header.Set("X-CSRF-Token", cookieNamed(login, auth.CSRFCookieName).Value)
	request.AddCookie(cookieNamed(login, auth.SessionCookieName))
	if rec := f.do(request); rec.Code != http.StatusForbidden {
		t.Fatalf("status = %d, want 403", rec.Code)
	}
	if user, _ := models.FindUserByID(context.Background(), f.database, f.adminID(t)); user == nil {
		t.Fatal("a non-admin deleted the admin")
	}
	_ = bob
}

func TestUndecryptableServicesAreShownAndOnlyResubmittingOverwrites(t *testing.T) {
	f := newSignedIn(t)
	ctx := context.Background()
	sealed, err := secret.Seal([]byte("another-secret-another-secret-00"), `{"kagi":{"session_token":"old","language":null}}`)
	if err != nil {
		t.Fatal(err)
	}
	userID := f.adminID(t)
	if _, err := models.UpsertUserSettings(ctx, f.database, userID, 25); err != nil {
		t.Fatal(err)
	}
	if _, err := f.database.ExecContext(ctx, "UPDATE user_settings SET save_services = ? WHERE user_id = ?", sealed, userID); err != nil {
		t.Fatal(err)
	}
	if page := f.get("/user-settings").Body.String(); !strings.Contains(page, "cannot be decrypted") && !strings.Contains(page, "unreadable") {
		t.Fatalf("the page does not say the credentials are unreadable")
	}
	// Viewing and unrelated forms leave the sealed value alone.
	f.postForm("/user-settings/preferences", url.Values{"theme": {"dark"}, "entries_per_page": {"25"}, "retention_read_days": {"0"}})
	var stored string
	if err := f.database.QueryRowContext(ctx, "SELECT save_services FROM user_settings WHERE user_id = ?", userID).Scan(&stored); err != nil || stored != sealed {
		t.Fatalf("preferences touched the sealed value: %q %v", stored, err)
	}
	f.expectFlash(t, "/user-settings/linkding", url.Values{"api_url": {"https://ld.example"}, "api_token": {"tok"}},
		"/user-settings", "Linkding configuration updated.")
	if page := f.get("/user-settings").Body.String(); strings.Contains(page, "cannot be decrypted") {
		t.Error("still unreadable after re-entering credentials")
	}
}

func TestRevokeAPITokenIsScopedToItsOwner(t *testing.T) {
	f := newSignedIn(t)
	ctx := context.Background()
	bob := f.makeUser(t, "bob")
	mine, err := models.CreateAPIToken(ctx, f.database, models.NewAPIToken{UserID: f.adminID(t), Kind: "greader", Label: "mine"})
	if err != nil {
		t.Fatal(err)
	}
	theirs, err := models.CreateAPIToken(ctx, f.database, models.NewAPIToken{UserID: bob.ID, Kind: "greader", Label: "theirs"})
	if err != nil {
		t.Fatal(err)
	}
	f.postForm(idPath("/user-settings/api-tokens/{id}/revoke", theirs.ID), nil)
	if tokens, _ := models.ListUserTokens(ctx, f.database, bob.ID); len(tokens) != 1 {
		t.Fatal("revoked another user's token")
	}
	f.expectFlash(t, idPath("/user-settings/api-tokens/{id}/revoke", mine.ID), nil, "/user-settings", "API token revoked.")
	f.expectFlash(t, "/user-settings/api-tokens/revoke-all", nil, "/user-settings", "There were no API tokens to revoke.")
}

func TestChangePasswordChecksThenSignsEveryoneOut(t *testing.T) {
	f := newSignedIn(t)
	const next = "violet-harbor-lantern-quartz-92"
	f.expectFlash(t, "/user-settings/password", url.Values{
		"current_password": {testPassword}, "new_password": {next}, "confirm_password": {next + "x"},
	}, "/user-settings", "New passwords do not match.")
	f.expectFlash(t, "/user-settings/password", url.Values{
		"current_password": {"not the password"}, "new_password": {next}, "confirm_password": {next},
	}, "/user-settings", "Current password is incorrect.")
	rec := f.postForm("/user-settings/password", url.Values{
		"current_password": {testPassword}, "new_password": {next}, "confirm_password": {next},
	})
	if rec.Header().Get("Location") != "/login" {
		t.Fatalf("location = %q", rec.Header().Get("Location"))
	}
	if after := f.get("/user-settings"); after.Code != http.StatusSeeOther {
		t.Errorf("the old session still works: %d", after.Code)
	}
}

func TestPasskeyAPIsListRenameAndDelete(t *testing.T) {
	f := newSignedIn(t)
	ctx := context.Background()
	key, err := models.CreatePasskey(ctx, f.database, models.NewPasskey{
		UserID: f.adminID(t), CredentialID: []byte("cred"), PublicKey: []byte("pub"), Name: "Laptop",
	})
	if err != nil {
		t.Fatal(err)
	}
	if body := f.get("/api/passkeys").Body.String(); !strings.Contains(body, `"name":"Laptop"`) || strings.Contains(body, "cred") {
		t.Fatalf("list = %s", body)
	}
	send := func(method, path, body string) *httptest.ResponseRecorder {
		request := jsonRequest(method, path, body)
		request.Header.Set("X-CSRF-Token", f.csrf)
		request.AddCookie(f.session)
		return f.do(request)
	}
	target := idPath("/api/passkeys/{id}", key.ID)
	if rec := send(http.MethodPut, target, `{"name":""}`); rec.Code != http.StatusBadRequest {
		t.Errorf("empty name = %d", rec.Code)
	}
	if rec := send(http.MethodPut, target, `{"name":"Desk"}`); rec.Code != http.StatusNoContent {
		t.Errorf("rename = %d", rec.Code)
	}
	if rec := send(http.MethodDelete, idPath("/api/passkeys/{id}", key.ID+99), ""); rec.Code != http.StatusNotFound {
		t.Errorf("delete missing = %d", rec.Code)
	}
	if rec := send(http.MethodDelete, target, ""); rec.Code != http.StatusNoContent {
		t.Errorf("delete = %d", rec.Code)
	}
}

func TestClientInfoSuggestsTheBrowserTimezoneOnlyWhenItDiffers(t *testing.T) {
	f := newSignedIn(t)
	send := func(body string) string {
		request := jsonRequest(http.MethodPut, "/api/user/client-info", body)
		request.Header.Set("X-CSRF-Token", f.csrf)
		request.AddCookie(f.session)
		return f.do(request).Body.String()
	}
	if got := send(`{"timezone":"UTC"}`); got != `{"hints":[]}` {
		t.Errorf("same zone: %s", got)
	}
	got := send(`{"timezone":"Asia/Taipei"}`)
	if !strings.Contains(got, "Your browser's timezone is Asia/Taipei, but Statistics cuts days in UTC.") ||
		!strings.Contains(got, `"/user-settings?timezone=Asia%2FTaipei"`) {
		t.Errorf("different zone: %s", got)
	}
}
