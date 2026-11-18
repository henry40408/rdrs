package middleware

import (
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
)

// NoStoreForAuthenticated sets "Cache-Control: no-store" and "Vary: Cookie"
// when the request carries a session cookie and the handler set no
// Cache-Control (OWASP Web Content Caching): a previous user's pages must not
// reappear via the back button or a cache.
//
// It only fills in a missing Cache-Control, so static assets, feed icons and
// the image proxy keep their own; requests without a session cookie stay
// cacheable. no-store alone subsumes no-cache and max-age=0, so those are not
// appended. Cookie is merged into any existing Vary.
//
// Anonymous HTML gets no-store too, since the anonymous-session layer sets a
// cookie there. no-store makes ETag useless for authenticated responses; that
// is a deliberate trade-off, and the dead ETag is not a bug to clean up.
func NoStoreForAuthenticated(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !auth.HasSessionCookie(r) {
			next.ServeHTTP(w, r)
			return
		}
		withHead(next, func(header http.Header, _ int) {
			if _, present := header["Cache-Control"]; present {
				return
			}
			header.Set("Cache-Control", "no-store")
			if merged, ok := mergedVary(header.Values("Vary")); ok {
				header.Set("Vary", merged)
			}
		}).ServeHTTP(w, r)
	})
}

// mergedVary is Cookie merged into all existing Vary lines (one HTTP list). It
// reports false when the response already covers Cookie or is "*".
func mergedVary(existing []string) (string, bool) {
	if len(existing) == 0 {
		return "Cookie", true
	}
	var parts []string
	for _, line := range existing {
		for _, part := range strings.Split(line, ",") {
			part = strings.TrimSpace(part)
			if part == "" {
				continue
			}
			if part == "*" || strings.EqualFold(part, "cookie") {
				return "", false
			}
			parts = append(parts, part)
		}
	}
	return strings.Join(append(parts, "Cookie"), ", "), true
}
