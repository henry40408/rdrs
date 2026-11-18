// Package middleware holds the HTTP layers that do not need application
// state: response headers, origin checks, timeout, compression and ETags.
package middleware

import (
	"net/http"
	"time"
)

// Middleware wraps a handler.
type Middleware func(http.Handler) http.Handler

// Chain wraps handler so the first middleware is the outermost: it sees the
// request first and the response last.
func Chain(handler http.Handler, layers ...Middleware) http.Handler {
	for i := len(layers) - 1; i >= 0; i-- {
		handler = layers[i](handler)
	}
	return handler
}

const contentSecurityPolicy = "default-src 'self'; " +
	"script-src 'self'; " +
	"style-src 'self'; " +
	"img-src 'self' data:; " +
	"font-src 'self'; " +
	"connect-src 'self'; " +
	"object-src 'none'; " +
	"base-uri 'self'; " +
	"form-action 'self'; " +
	"frame-ancestors 'none'"

// Unused features, denied. Passkey features are omitted on purpose: naming
// them would drop their `self` default and break passkeys.
const permissionsPolicy = "accelerometer=(), autoplay=(), camera=(), display-capture=(), " +
	"encrypted-media=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), " +
	"midi=(), payment=(), usb=(), xr-spatial-tracking=()"

// Not no-referrer: entry-action redirects recover the originating list from
// the same-origin Referer.
const referrerPolicy = "strict-origin-when-cross-origin"

// SecurityHeaders adds the fixed security headers to every response, keeping
// any already present (a reverse proxy's own header wins). There must be no
// skip list: these describe the host. Cross-Origin-Resource-Policy is absent
// on purpose, because same-origin would break proxied images in third-party
// Google Reader clients' webviews.
func SecurityHeaders(next http.Handler) http.Handler {
	fixed := [][2]string{
		{"Content-Security-Policy", contentSecurityPolicy},
		{"X-Content-Type-Options", "nosniff"},
		{"Referrer-Policy", referrerPolicy},
		{"Permissions-Policy", permissionsPolicy},
		{"X-Frame-Options", "DENY"},
		{"Cross-Origin-Opener-Policy", "same-origin"},
	}
	return withHead(next, func(header http.Header, _ int) {
		for _, h := range fixed {
			setIfAbsent(header, h[0], h[1])
		}
	})
}

// HSTS adds Strict-Transport-Security unless already present. Install it only
// for HTTPS deployments.
func HSTS(value string) Middleware {
	return func(next http.Handler) http.Handler {
		return withHead(next, func(header http.Header, _ int) {
			setIfAbsent(header, "Strict-Transport-Security", value)
		})
	}
}

// DateHeader sets Date on every response, replacing any value.
func DateHeader(next http.Handler) http.Handler {
	return withHead(next, func(header http.Header, _ int) {
		header.Set("Date", time.Now().UTC().Format(http.TimeFormat))
	})
}

func setIfAbsent(header http.Header, name, value string) {
	if _, present := header[http.CanonicalHeaderKey(name)]; !present {
		header.Set(name, value)
	}
}

// withHead runs edit once, just before the response head is written.
func withHead(next http.Handler, edit func(header http.Header, status int)) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hw := &headWriter{ResponseWriter: w, edit: edit}
		next.ServeHTTP(hw, r)
		hw.ensureHead(http.StatusOK)
	})
}

type headWriter struct {
	http.ResponseWriter
	edit func(header http.Header, status int)
	done bool
}

func (w *headWriter) ensureHead(status int) {
	if w.done {
		return
	}
	w.done = true
	w.edit(w.Header(), status)
}

func (w *headWriter) WriteHeader(status int) {
	// Informational heads (1xx) are not the final head.
	if status >= 200 {
		w.ensureHead(status)
	}
	w.ResponseWriter.WriteHeader(status)
}

func (w *headWriter) Write(data []byte) (int, error) {
	w.ensureHead(http.StatusOK)
	return w.ResponseWriter.Write(data)
}

func (w *headWriter) Flush() {
	w.ensureHead(http.StatusOK)
	_ = http.NewResponseController(w.ResponseWriter).Flush()
}

func (w *headWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }
