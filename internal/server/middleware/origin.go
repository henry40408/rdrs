package middleware

import (
	"log/slog"
	"net/http"
	"strings"
)

// CrossSiteGuard is the first-line CSRF defence, header-only. It follows
// tower-http's Csrf layer rather than http.CrossOriginProtection, whose
// semantics differ:
//
//   - GET, HEAD and OPTIONS pass; TRACE does not.
//   - Sec-Fetch-Site same-origin or none passes; any other value is refused.
//   - Without Sec-Fetch-Site, no Origin passes (not a browser, or same-origin
//     from an old one); otherwise the Origin authority must equal the Host
//     byte for byte.
//
// A refusal is 403 with an empty body, logged as csrf.cross_site.
func CrossSiteGuard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		check := crossSiteCheck(r)
		if check == "" {
			next.ServeHTTP(w, r)
			return
		}
		slog.Warn("rejected a state-changing request the browser reported as cross-site",
			"event", "csrf.cross_site", "check", check, "method", r.Method, "path", r.URL.Path,
			"sec_fetch_site", headerOrDash(r, "Sec-Fetch-Site"), "origin", headerOrDash(r, "Origin"))
		w.WriteHeader(http.StatusForbidden)
	})
}

const (
	checkSecFetchSite   = "sec_fetch_site"
	checkOriginFallback = "origin_fallback"
)

// crossSiteCheck names the check that refuses r, or is empty when r passes.
// origin_fallback flags an old browser or a Host-rewriting proxy.
func crossSiteCheck(r *http.Request) string {
	switch r.Method {
	case http.MethodGet, http.MethodHead, http.MethodOptions:
		return ""
	}
	switch r.Header.Get("Sec-Fetch-Site") {
	case "same-origin", "none":
		return ""
	case "":
	default:
		return checkSecFetchSite
	}
	origin := r.Header.Get("Origin")
	if origin == "" {
		return ""
	}
	if authority, ok := httpOriginAuthority(origin); ok && authority == r.Host {
		return ""
	}
	return checkOriginFallback
}

// httpOriginAuthority is the authority of an http or https origin. Anything
// else, including the opaque "null" origin, has none.
func httpOriginAuthority(origin string) (string, bool) {
	rest, ok := strings.CutPrefix(origin, "http://")
	if !ok {
		rest, ok = strings.CutPrefix(origin, "https://")
	}
	if !ok || strings.ContainsAny(rest, "# \t") {
		return "", false
	}
	authority, _, _ := strings.Cut(rest, "/")
	authority, _, _ = strings.Cut(authority, "?")
	return authority, authority != ""
}

func headerOrDash(r *http.Request, name string) string {
	if value := r.Header.Get(name); value != "" {
		return value
	}
	return "-"
}
