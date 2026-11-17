package auth

import (
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
)

// Cookie names. The __Host- variants stop a sibling subdomain from shadowing
// the cookie; they must never be written without Secure (browsers discard
// them), so write only through SessionCookieName and CSRFCookieName.
const (
	SessionCookieName     = "session_token"
	SessionCookieNameHost = "__Host-session_token"
	CSRFCookieName        = "csrf_token"
	CSRFCookieNameHost    = "__Host-csrf_token"
)

const sessionCookieMaxAge = models.SessionExpiryDays * 24 * 60 * 60

// SessionCookieNameFor is the session cookie name to write: prefixed only when
// Secure is in effect.
func SessionCookieNameFor(secure bool) string {
	if secure {
		return SessionCookieNameHost
	}
	return SessionCookieName
}

// CSRFCookieNameFor is the CSRF cookie name to write.
func CSRFCookieNameFor(secure bool) string {
	if secure {
		return CSRFCookieNameHost
	}
	return CSRFCookieName
}

// Cookies builds and reads the session and CSRF cookies of one deployment.
type Cookies struct {
	Secret []byte
	Secure bool
}

// Session builds the session cookie for token. Every login path must use it:
// the value is HMAC-signed, so forgeries fail before any lookup and a leaked
// session.session_token is useless without the root key.
func (c Cookies) Session(token string) *http.Cookie {
	return &http.Cookie{
		Name:     SessionCookieNameFor(c.Secure),
		Value:    secret.SignSession(c.Secret, token),
		Path:     "/",
		HttpOnly: true,
		Secure:   c.Secure,
		SameSite: http.SameSiteLaxMode,
		MaxAge:   sessionCookieMaxAge,
	}
}

// CSRF builds the CSRF cookie for a session token. It is readable by script
// on purpose: it is never the credential, because the guard re-derives the
// expected token from the signed session cookie.
func (c Cookies) CSRF(sessionToken string) *http.Cookie {
	return &http.Cookie{
		Name:     CSRFCookieNameFor(c.Secure),
		Value:    secret.DeriveCSRF(c.Secret, sessionToken),
		Path:     "/",
		Secure:   c.Secure,
		SameSite: http.SameSiteLaxMode,
		MaxAge:   sessionCookieMaxAge,
	}
}

// RemoveSession is the cookie that evicts the session cookie under both
// names, so logout works whichever one an earlier configuration wrote.
func (c Cookies) RemoveSession() []*http.Cookie {
	return []*http.Cookie{removal(SessionCookieName, false), removal(SessionCookieNameHost, true)}
}

// RemoveCSRF evicts the CSRF cookie under both names.
func (c Cookies) RemoveCSRF() []*http.Cookie {
	return []*http.Cookie{removal(CSRFCookieName, false), removal(CSRFCookieNameHost, true)}
}

// removal expires a cookie. The __Host- one always carries Secure, or
// browsers ignore it.
func removal(name string, secure bool) *http.Cookie {
	return &http.Cookie{Name: name, Path: "/", Secure: secure, MaxAge: -1}
}

// SessionToken returns the verified session token from the request, or "".
//
// It tries the __Host- name first and then the plain one even when the
// prefixed cookie is present but invalid: flipping RDRS_COOKIE_SECURE must not
// log everyone out, and a stale empty __Host- cookie must not shadow a valid
// one. That is safe because forgery resistance comes from the HMAC alone.
func (c Cookies) SessionToken(r *http.Request) string {
	for _, name := range []string{SessionCookieNameHost, SessionCookieName} {
		cookie, err := r.Cookie(name)
		if err != nil {
			continue
		}
		if token, ok := secret.VerifySession(c.Secret, cookie.Value); ok {
			return token
		}
	}
	return ""
}

// CSRFToken is the token to render into a form: derived from the session
// cookie, as the guard does, not from the session row (they differ during the
// grace interval after a rotation). Empty without a session cookie.
func (c Cookies) CSRFToken(r *http.Request) string {
	token := c.SessionToken(r)
	if token == "" {
		return ""
	}
	return secret.DeriveCSRF(c.Secret, token)
}

// HasSessionCookie reports whether a request carries a session cookie of
// either name, valid or not.
func HasSessionCookie(r *http.Request) bool {
	for _, name := range []string{SessionCookieName, SessionCookieNameHost} {
		if _, err := r.Cookie(name); err == nil {
			return true
		}
	}
	return false
}

// SetRequestCookie replaces (not appends) a cookie in the request's Cookie
// header: lookups return the first match, so a stale value would shadow it.
func SetRequestCookie(r *http.Request, cookie *http.Cookie) {
	prefix := cookie.Name + "="
	kept := make([]string, 0, 4)
	for _, line := range r.Header.Values("Cookie") {
		for _, pair := range strings.Split(line, ";") {
			pair = strings.TrimSpace(pair)
			if pair != "" && !strings.HasPrefix(pair, prefix) {
				kept = append(kept, pair)
			}
		}
	}
	kept = append(kept, prefix+cookie.Value)
	r.Header.Set("Cookie", strings.Join(kept, "; "))
}

// HasSetCookie reports whether the response header sets a cookie of one of
// names (exact name match, not a substring search).
func HasSetCookie(header http.Header, names ...string) bool {
	for _, line := range header.Values("Set-Cookie") {
		name, _, found := strings.Cut(line, "=")
		if !found {
			continue
		}
		name = strings.TrimSpace(name)
		for _, wanted := range names {
			if name == wanted {
				return true
			}
		}
	}
	return false
}
