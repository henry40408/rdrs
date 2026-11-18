package auth

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/secret"
)

var testSecret = []byte("01234567890123456789012345678901")

func requestWithCookies(cookies ...*http.Cookie) *http.Request {
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	for _, cookie := range cookies {
		r.AddCookie(cookie)
	}
	return r
}

func TestSessionCookieNameIsPrefixedOnlyWhenSecure(t *testing.T) {
	if SessionCookieNameFor(true) != SessionCookieNameHost || SessionCookieNameFor(false) != SessionCookieName {
		t.Error("session cookie name")
	}
	if CSRFCookieNameFor(true) != CSRFCookieNameHost || CSRFCookieNameFor(false) != CSRFCookieName {
		t.Error("csrf cookie name")
	}
}

func TestSessionCookieAttributes(t *testing.T) {
	secure := Cookies{Secret: testSecret, Secure: true}.Session("tok")
	if secure.Name != SessionCookieNameHost || !secure.Secure || secure.Path != "/" || !secure.HttpOnly {
		t.Errorf("secure cookie = %+v", secure)
	}
	// The browser rejects a __Host- cookie with a Domain.
	if secure.Domain != "" || secure.MaxAge != 7*24*3600 || secure.SameSite != http.SameSiteLaxMode {
		t.Errorf("secure cookie = %+v", secure)
	}
	plain := Cookies{Secret: testSecret}.Session("tok")
	if plain.Name != SessionCookieName || plain.Secure || plain.Domain != "" {
		t.Errorf("plain cookie = %+v", plain)
	}
	if plain.Value != secret.SignSession(testSecret, "tok") {
		t.Error("value is not the signed token")
	}
}

func TestCSRFCookieIsScriptReadable(t *testing.T) {
	cookie := Cookies{Secret: testSecret}.CSRF("tok")
	if cookie.HttpOnly || cookie.Value != secret.DeriveCSRF(testSecret, "tok") || cookie.Name != CSRFCookieName {
		t.Errorf("csrf cookie = %+v", cookie)
	}
}

func TestSessionTokenVerifiesBothNames(t *testing.T) {
	cookies := Cookies{Secret: testSecret}
	signed := secret.SignSession(testSecret, "tok")
	for _, name := range []string{SessionCookieName, SessionCookieNameHost} {
		r := requestWithCookies(&http.Cookie{Name: name, Value: signed})
		if got := cookies.SessionToken(r); got != "tok" {
			t.Errorf("%s: token = %q", name, got)
		}
	}
	forged := requestWithCookies(&http.Cookie{Name: SessionCookieName, Value: "tok.deadbeef"})
	if got := cookies.SessionToken(forged); got != "" {
		t.Errorf("forged token accepted: %q", got)
	}
	if got := cookies.SessionToken(requestWithCookies()); got != "" {
		t.Errorf("no cookie: %q", got)
	}
}

func TestInvalidHostCookieDoesNotShadowValidPlainCookie(t *testing.T) {
	r := requestWithCookies(
		&http.Cookie{Name: SessionCookieNameHost, Value: ""},
		&http.Cookie{Name: SessionCookieName, Value: secret.SignSession(testSecret, "tok")},
	)
	if got := (Cookies{Secret: testSecret}).SessionToken(r); got != "tok" {
		t.Errorf("token = %q", got)
	}
}

func TestCSRFTokenDerivesFromTheCookieToken(t *testing.T) {
	cookies := Cookies{Secret: testSecret}
	if cookies.CSRFToken(requestWithCookies()) != "" {
		t.Error("no session must give an empty token")
	}
	r := requestWithCookies(&http.Cookie{Name: SessionCookieName, Value: secret.SignSession(testSecret, "tok")})
	if got := cookies.CSRFToken(r); got != secret.DeriveCSRF(testSecret, "tok") {
		t.Errorf("token = %q", got)
	}
}

func TestRemovalCookies(t *testing.T) {
	cookies := Cookies{Secret: testSecret}
	for _, cookie := range append(cookies.RemoveSession(), cookies.RemoveCSRF()...) {
		wantSecure := strings.HasPrefix(cookie.Name, "__Host-")
		if cookie.MaxAge != -1 || cookie.Path != "/" || cookie.Secure != wantSecure {
			t.Errorf("removal %+v", cookie)
		}
	}
}

func TestHasSessionCookie(t *testing.T) {
	for name, want := range map[string]bool{SessionCookieName: true, SessionCookieNameHost: true, "unrelated": false} {
		if got := HasSessionCookie(requestWithCookies(&http.Cookie{Name: name, Value: "x"})); got != want {
			t.Errorf("%s: %v", name, got)
		}
	}
	if HasSessionCookie(requestWithCookies()) {
		t.Error("no cookies")
	}
}

func TestSetRequestCookieReplacesInsteadOfAppending(t *testing.T) {
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.Header.Add("Cookie", "a=1; csrf_token=stale")
	r.Header.Add("Cookie", "csrf_token=stale2; b=2")
	SetRequestCookie(r, &http.Cookie{Name: "csrf_token", Value: "fresh"})
	if got := r.Header.Values("Cookie"); len(got) != 1 || got[0] != "a=1; b=2; csrf_token=fresh" {
		t.Errorf("Cookie = %q", got)
	}
	if cookie, _ := r.Cookie("csrf_token"); cookie == nil || cookie.Value != "fresh" {
		t.Error("stale value shadows the new one")
	}
}

func TestHasSetCookieMatchesExactName(t *testing.T) {
	header := http.Header{}
	header.Add("Set-Cookie", "other=session_token_lookalike; Path=/")
	if HasSetCookie(header, SessionCookieName) {
		t.Error("fooled by a value containing the name")
	}
	header.Add("Set-Cookie", "csrf_token=xyz; Path=/; SameSite=Lax; Max-Age=604800; Secure")
	if !HasSetCookie(header, CSRFCookieName) || HasSetCookie(header, SessionCookieName) {
		t.Error("attribute-laden value mishandled")
	}
	header.Add("Set-Cookie", "__Host-session_token=; Path=/; Secure")
	if !HasSetCookie(header, SessionCookieName, SessionCookieNameHost) {
		t.Error("either name must count")
	}
}
