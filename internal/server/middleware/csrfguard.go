package middleware

import (
	"bytes"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/secret"
)

const (
	// csrfHeader is where a browser echoes the token for fetch-driven mutations.
	csrfHeader = "X-CSRF-Token"
	// csrfField carries the token on a body-submitted POST.
	csrfField = "_csrf"
	// csrfSkipSuffix is the only route exempt from the guard. A forged
	// ClientLogin needs credentials the attacker already knows, its response is
	// unreadable cross-origin, and login-CSRF is stopped by the origin guard.
	// It is suffix-matched because it is also mounted under /api/greader.php.
	csrfSkipSuffix = "/accounts/ClientLogin"
	// csrfMaxBodyBytes caps the body buffered to read the form field (1 MiB).
	csrfMaxBodyBytes = 1 << 20
)

// CSRFGuard is the synchronizer-token guard, the second CSRF line.
//
// An unsafe method must carry the session's token in X-CSRF-Token or the
// _csrf form field (the body is buffered and rebuilt). The expected token is a
// MAC re-derived from the signed session cookie: no database trip, and the
// readable cookie is never trusted. Multipart bodies pass through; that route
// checks the field itself.
//
// The GReader API is deliberately not exempt: bearer clients carry no cookie
// and pass anyway, but a browser with an ambient cookie there would have its
// token waived, leaving only the origin guard.
func CSRFGuard(cookies auth.Cookies) Middleware {
	guard := csrfRejecter{secret: cookies.Secret}
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if isSafeMethod(r.Method) || strings.HasSuffix(r.URL.Path, csrfSkipSuffix) {
				next.ServeHTTP(w, r)
				return
			}
			// No (valid) session cookie passes: CSRF rides the victim's cookie, so
			// this cannot be a forged authenticated action, and authentication
			// rejects it downstream. Login-CSRF is stopped by the origin guard.
			sessionToken := cookies.SessionToken(r)
			if sessionToken == "" {
				next.ServeHTTP(w, r)
				return
			}
			if guard.allows(r, sessionToken) {
				next.ServeHTTP(w, r)
				return
			}
			w.WriteHeader(http.StatusForbidden)
		})
	}
}

// isSafeMethod includes TRACE, unlike the origin guard.
func isSafeMethod(method string) bool {
	switch method {
	case http.MethodGet, http.MethodHead, http.MethodOptions, http.MethodTrace:
		return true
	}
	return false
}

type csrfRejecter struct{ secret []byte }

// allows checks the header, which never falls back to the body once present,
// then the form field. It rebuilds the body it consumed.
func (g csrfRejecter) allows(r *http.Request, sessionToken string) bool {
	if submitted, present := headerToken(r); present {
		if secret.VerifyCSRF(g.secret, sessionToken, submitted) {
			return true
		}
		g.warn(r, sessionToken, "header token does not derive from this session")
		return false
	}
	// The OPML import route validates the field itself.
	if strings.HasPrefix(strings.TrimLeft(r.Header.Get("Content-Type"), " \t"), "multipart/form-data") {
		return true
	}
	body, ok := readLimited(r)
	if !ok {
		g.warn(r, sessionToken, "body unreadable or over the buffering limit")
		return false
	}
	r.Body = io.NopCloser(bytes.NewReader(body))
	r.ContentLength = int64(len(body))
	if !secret.VerifyCSRF(g.secret, sessionToken, formField(body, csrfField)) {
		g.warn(r, sessionToken, "no _csrf field, or it does not derive from this session")
		return false
	}
	return true
}

// headerToken is the submitted header; one with bytes outside visible ASCII is
// as good as absent.
func headerToken(r *http.Request) (string, bool) {
	values := r.Header.Values(csrfHeader)
	if len(values) == 0 {
		return "", false
	}
	for i := 0; i < len(values[0]); i++ {
		if c := values[0][i]; (c < 0x20 && c != '\t') || c > 0x7e {
			return "", false
		}
	}
	return values[0], true
}

func readLimited(r *http.Request) ([]byte, bool) {
	body, err := io.ReadAll(io.LimitReader(r.Body, csrfMaxBodyBytes+1))
	if err != nil || len(body) > csrfMaxBodyBytes {
		return nil, false
	}
	return body, true
}

// formField is the first value of a urlencoded field; a malformed pair is
// skipped, as it cannot carry a hex token.
func formField(body []byte, name string) string {
	values, _ := url.ParseQuery(string(body))
	return values.Get(name)
}

// warn logs a rejection, identifying the session only by its salted audit id,
// never the live token.
func (g csrfRejecter) warn(r *http.Request, sessionToken, reason string) {
	slog.Warn("rejected a state-changing request whose CSRF token did not match its session",
		"event", "csrf.mismatch", "reason", reason, "method", r.Method, "path", r.URL.Path,
		"session", secret.AuditID(g.secret, sessionToken))
}
