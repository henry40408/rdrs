package middleware

import (
	"net/http"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
)

// anonSkipPrefixes are paths that never get an anonymous session: cacheable
// assets and machine APIs. None renders a form (/offline deliberately uses a
// link).
var anonSkipPrefixes = []string{
	"/api", "/reader", "/accounts", "/static", "/favicon", "/health", "/sw.js", "/offline",
	// Tracking pixel: sessionless clients would just discard the cookie.
	"/p/",
}

// AnonymousSession gives a logged-out visitor a signed session cookie backing
// no session row, so every page can carry a CSRF token, and makes sure the
// CSRF cookie matches whatever session the request has. The cookies are
// injected into this request too, so the guard and the handler see them on the
// same round trip.
//
// Layer it inside ForwardAuth so forward-auth's Set-Cookie wins.
func AnonymousSession(cookies auth.Cookies) Middleware {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if hasAnyPrefix(r.URL.Path, anonSkipPrefixes) {
				next.ServeHTTP(w, r)
				return
			}
			issued := anonymousCookies(cookies, r)
			for _, cookie := range issued.forRequest {
				auth.SetRequestCookie(r, cookie)
			}
			if len(issued.forResponse) == 0 {
				next.ServeHTTP(w, r)
				return
			}
			withHead(next, func(header http.Header, _ int) {
				for _, cookie := range issued.forResponse {
					header.Add("Set-Cookie", cookie.String())
				}
			}).ServeHTTP(w, r)
		})
	}
}

type issuedCookies struct {
	forRequest  []*http.Cookie
	forResponse []*http.Cookie
}

func anonymousCookies(cookies auth.Cookies, r *http.Request) issuedCookies {
	token := cookies.SessionToken(r)
	if token == "" {
		token = models.GenerateToken()
		pair := []*http.Cookie{cookies.Session(token), cookies.CSRF(token)}
		return issuedCookies{forRequest: pair, forResponse: pair}
	}
	return healedCSRFCookies(cookies, r, token)
}

// healedCSRFCookies keeps the session and verifies, not just detects, the CSRF
// cookie, so a stale one heals instead of failing every unsafe request,
// logout included. Overwriting is safe: the CSRF cookie is never the
// credential.
func healedCSRFCookies(cookies auth.Cookies, r *http.Request, token string) issuedCookies {
	current, err := r.Cookie(auth.CSRFCookieNameFor(cookies.Secure))
	matches := err == nil && secret.VerifyCSRF(cookies.Secret, token, current.Value)

	// A cookie under the other name is a leftover that drifts and can be
	// picked over the live one by document.cookie readers.
	staleName := auth.CSRFCookieNameFor(!cookies.Secure)
	_, staleErr := r.Cookie(staleName)
	stale := staleErr == nil

	if matches && !stale {
		return issuedCookies{}
	}
	// Always reissue alongside a removal: SlideSession skips its own reissue
	// on seeing either CSRF name, which would leave no live token.
	csrf := cookies.CSRF(token)
	issued := issuedCookies{forRequest: []*http.Cookie{csrf}, forResponse: []*http.Cookie{csrf}}
	if stale {
		issued.forResponse = append(issued.forResponse, csrfRemoval(staleName))
	}
	return issued
}

// csrfRemoval evicts a CSRF cookie under the name this deployment no longer
// writes. The __Host- one always carries Secure, or browsers ignore it.
func csrfRemoval(name string) *http.Cookie {
	return &http.Cookie{Name: name, Path: "/", Secure: name == auth.CSRFCookieNameHost, MaxAge: -1}
}
