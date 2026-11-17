package middleware

import (
	"net/http"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
)

// forwardAuthSkipPrefixes never trigger forward-auth auto-login: machine
// endpoints authenticate themselves, and the PWA paths must stay cookie-free
// to be publicly cacheable.
var forwardAuthSkipPrefixes = []string{
	"/api", "/reader", "/accounts", "/events", "/static", "/favicon", "/health", "/sw.js", "/offline", "/p/",
}

// ForwardAuth signs in the identity a trusted proxy asserts. A valid session
// wins; an invalid cookie must not block forward-auth, or the user is locked
// out. It redirects to the same URL so the freshly set cookie authenticates
// the retry.
func ForwardAuth(login auth.ForwardLogin) Middleware {
	cookies := auth.Cookies{Secret: login.Config.Secret, Secure: login.Config.CookieSecure}
	return func(next http.Handler) http.Handler {
		if !login.Config.AuthProxyEnabled() {
			return next
		}
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if hasAnyPrefix(r.URL.Path, forwardAuthSkipPrefixes) || hasLiveSession(r, login, cookies) {
				next.ServeHTTP(w, r)
				return
			}
			username := auth.ForwardAuthIdentity(login.Config, r)
			if username == "" {
				next.ServeHTTP(w, r)
				return
			}
			session, err := login.SignIn(r, username)
			if err != nil {
				// Fail closed: fall back to the normal cookie flow.
				next.ServeHTTP(w, r)
				return
			}
			if session == nil {
				flash.Redirect(w, "/login", flash.NewWarning("You are not authorized to access this instance."))
				return
			}
			http.SetCookie(w, cookies.Session(session.SessionToken))
			// The CSRF cookie goes out now so the first rendered form has a valid token.
			http.SetCookie(w, cookies.CSRF(session.SessionToken))
			w.Header().Set("Location", r.URL.RequestURI())
			w.WriteHeader(http.StatusSeeOther)
		})
	}
}

func hasLiveSession(r *http.Request, login auth.ForwardLogin, cookies auth.Cookies) bool {
	token := cookies.SessionToken(r)
	if token == "" {
		return false
	}
	session, err := models.FindSessionByToken(r.Context(), login.DB, token)
	return err == nil && session != nil && !session.IsExpired()
}
