package middleware

import (
	"log/slog"
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/models"
)

// slideSkipPrefixes are cacheable paths that must never get a Set-Cookie.
// Narrower than anonSkipPrefixes so API clients still get Max-Age renewed;
// other public responses are caught by publiclyCacheable.
var slideSkipPrefixes = []string{"/static", "/favicon", "/health", "/sw.js", "/offline"}

func hasAnyPrefix(path string, prefixes []string) bool {
	for _, prefix := range prefixes {
		if strings.HasPrefix(path, prefix) {
			return true
		}
	}
	return false
}

// SlideSession reissues the session and CSRF cookies on every request with a
// verified session cookie, so Max-Age slides with use, and rotates the token
// when an authentication check asked for it (auth.RequestRotation).
//
// It never clobbers an inner layer's Set-Cookie: if either name of a cookie
// purpose is already set it leaves that purpose alone, or logout's removal
// cookies would be undone by a live one.
func SlideSession(authenticator *auth.Authenticator) Middleware {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			cookies := authenticator.Cookies()
			token := cookies.SessionToken(r)
			if token == "" || hasAnyPrefix(r.URL.Path, slideSkipPrefixes) {
				next.ServeHTTP(w, r)
				return
			}
			ctx, slot := auth.WithRotationSlot(r.Context())
			r = r.WithContext(ctx)
			withHead(next, func(header http.Header, _ int) {
				// A session cookie on a shared-cacheable response would leak the
				// session to the next visitor. Checked before rotating, since this
				// response cannot carry the new token.
				if publiclyCacheable(header) {
					return
				}
				if slot.Requested() {
					token = rotate(r, authenticator, token)
				}
				if !auth.HasSetCookie(header, auth.SessionCookieName, auth.SessionCookieNameHost) {
					header.Add("Set-Cookie", cookies.Session(token).String())
				}
				if !auth.HasSetCookie(header, auth.CSRFCookieName, auth.CSRFCookieNameHost) {
					header.Add("Set-Cookie", cookies.CSRF(token).String())
				}
			}).ServeHTTP(w, r)
		})
	}
}

// rotate swaps the session token. An empty result means a concurrent request
// rotated first, and our token stays valid as the grace token.
func rotate(r *http.Request, authenticator *auth.Authenticator, token string) string {
	rotated, err := models.RotateToken(r.Context(), authenticator.DB, token)
	if err != nil {
		slog.Warn("session token rotation failed; keeping current token",
			"event", "session.rotation_failed", "error", err)
		return token
	}
	if rotated == "" {
		return token
	}
	authenticator.Audit.SessionTokenRotated(token, rotated)
	return rotated
}

// publiclyCacheable reports whether Cache-Control marks the response as
// shared-cacheable (no no-store, no private). A missing header counts as not.
func publiclyCacheable(header http.Header) bool {
	values := header.Values("Cache-Control")
	if len(values) == 0 {
		return false
	}
	for _, directive := range strings.Split(values[0], ",") {
		name, _, _ := strings.Cut(directive, "=")
		name = strings.TrimSpace(name)
		if strings.EqualFold(name, "no-store") || strings.EqualFold(name, "private") {
			return false
		}
	}
	return true
}
