package server

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
)

// signedInFixture is an app with one admin and that admin's cookies.
type signedInFixture struct {
	*authFixture
	session *http.Cookie
	csrf    string
}

func newSignedIn(t *testing.T) *signedInFixture {
	t.Helper()
	f := newAuthFixture(t)
	f.createAdmin(t)
	login := f.do(jsonRequest("POST", "/api/session", `{"username":"root","password":"`+testPassword+`"}`))
	return &signedInFixture{
		authFixture: f,
		session:     cookieNamed(login, auth.SessionCookieName),
		csrf:        cookieNamed(login, auth.CSRFCookieName).Value,
	}
}

func (f *signedInFixture) get(path string) *httptest.ResponseRecorder {
	request := httptest.NewRequest(http.MethodGet, path, nil)
	request.AddCookie(f.session)
	return f.do(request)
}

func (f *signedInFixture) postForm(path string, fields url.Values) *httptest.ResponseRecorder {
	request := httptest.NewRequest(http.MethodPost, path, strings.NewReader(fields.Encode()))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	request.Header.Set("X-CSRF-Token", f.csrf)
	request.AddCookie(f.session)
	return f.do(request)
}

// followedPage is /categories requested with the flash cookie the response
// set, which is signed by the outer layers, so only the real page can read it.
func (f *signedInFixture) followedPage(rec *httptest.ResponseRecorder) string {
	request := httptest.NewRequest(http.MethodGet, "/categories", nil)
	request.AddCookie(f.session)
	if c := cookieNamed(rec, flash.CookieName); c != nil {
		request.AddCookie(c)
	}
	return f.do(request).Body.String()
}

func TestAnonymousPageRedirectsToLoginRememberingThePage(t *testing.T) {
	f := newAuthFixture(t)
	for target, want := range map[string]string{
		"/categories":     "/login?next=%2Fcategories",
		"/categories?a=1": "/login?next=%2Fcategories%3Fa%3D1",
	} {
		rec := f.do(httptest.NewRequest(http.MethodGet, target, nil))
		if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != want {
			t.Errorf("GET %s = %d %q, want 303 %q", target, rec.Code, rec.Header().Get("Location"), want)
		}
	}
}

func TestCategoryFormsFlashTheirOutcome(t *testing.T) {
	f := newSignedIn(t)
	cases := []struct {
		name, path string
		fields     url.Values
		text       string
	}{
		{"create", "/categories", url.Values{"name": {" News "}}, "Category created."},
		{"duplicate", "/categories", url.Values{"name": {"News"}}, "Category already exists."},
		{"empty", "/categories", url.Values{"name": {"  "}}, "Category name cannot be empty"},
		{"too long", "/categories", url.Values{"name": {strings.Repeat("x", 101)}}, "Category name is too long (max 100)"},
		{"rename missing", "/categories/99/rename", url.Values{"name": {"x"}}, "Category not found."},
		{"delete missing", "/categories/99/delete", nil, "Category not found."},
	}
	for _, tc := range cases {
		rec := f.postForm(tc.path, tc.fields)
		if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/categories" {
			t.Fatalf("%s: %d %q", tc.name, rec.Code, rec.Header().Get("Location"))
		}
		if page := f.followedPage(rec); !strings.Contains(page, tc.text) {
			t.Errorf("%s: page lacks %q", tc.name, tc.text)
		}
	}
}

func TestRenameAndDeleteOwnCategory(t *testing.T) {
	f := newSignedIn(t)
	f.postForm("/categories", url.Values{"name": {"News"}})
	f.postForm("/categories", url.Values{"name": {"Tech"}})
	id := ""
	for _, candidate := range []string{"1", "2", "3"} {
		if strings.Contains(f.get("/categories").Body.String(), "/categories/"+candidate+"/rename") {
			id = candidate
			break
		}
	}
	steps := []struct {
		action, name, want string
	}{
		{"rename", "Tech", "Category name already in use."},
		{"rename", "World", "Category renamed."},
		{"delete", "", "Category deleted."},
	}
	for _, step := range steps {
		rec := f.postForm("/categories/"+id+"/"+step.action, url.Values{"name": {step.name}})
		if page := f.followedPage(rec); !strings.Contains(page, step.want) {
			t.Errorf("%s %q: page lacks %q", step.action, step.name, step.want)
		}
	}
}

func TestAnonymousCategoryPostIsRefused(t *testing.T) {
	f := newAuthFixture(t)
	request := httptest.NewRequest(http.MethodPost, "/categories", strings.NewReader("name=x"))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	if rec := f.do(request); rec.Code != http.StatusForbidden {
		t.Errorf("anonymous POST = %d, want 403", rec.Code)
	}
}

func TestCategoryFormRequiresTheNameField(t *testing.T) {
	f := newSignedIn(t)
	if rec := f.postForm("/categories", nil); rec.Code != http.StatusUnprocessableEntity {
		t.Errorf("missing name = %d, want 422", rec.Code)
	}
}

func TestSidebarFeedsOfAnotherUsersCategoryIs404(t *testing.T) {
	f := newSignedIn(t)
	if rec := f.get("/api/sidebar/categories/999/feeds"); rec.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404", rec.Code)
	}
	if rec := f.get("/api/sidebar/categories/x/feeds"); rec.Code != http.StatusBadRequest {
		t.Errorf("non-numeric id = %d, want 400", rec.Code)
	}
}

func TestChronoUTCDropsOrKeepsTheFraction(t *testing.T) {
	base := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	for nanos, want := range map[int]string{
		0:           "2026-01-01T00:00:00Z",
		120_000_000: "2026-01-01T00:00:00.120Z",
		120_500_000: "2026-01-01T00:00:00.120500Z",
		120_500_001: "2026-01-01T00:00:00.120500001Z",
	} {
		if got := chronoUTC(base.Add(time.Duration(nanos))); got != want {
			t.Errorf("%d ns: %s, want %s", nanos, got, want)
		}
	}
}
