package middleware

import (
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"strings"
)

// maxETagBody is the largest body buffered to compute an ETag.
const maxETagBody = 4 << 20

// ETag adds a weak ETag to 2xx text/html responses and answers a matching
// If-None-Match with 304. It sits inside Compression so it hashes the
// uncompressed body.
func ETag(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ifNoneMatch := r.Header.Get("If-None-Match")
		dw := &deferredWriter{ResponseWriter: w, shouldBuffer: isTaggable, maxBody: maxETagBody}
		next.ServeHTTP(dw, r)
		if !dw.held() {
			return
		}
		if dw.overflow {
			// HTML should never exceed the limit, so fail loudly.
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		body := dw.body.Bytes()
		etag := weakETag(body)
		w.Header().Set("ETag", etag)
		if ifNoneMatch != "" && etagMatches(ifNoneMatch, etag) {
			w.Header().Del("Content-Length")
			w.Header().Del("Content-Type")
			w.WriteHeader(http.StatusNotModified)
			return
		}
		w.WriteHeader(dw.status)
		_, _ = w.Write(body)
	})
}

func isTaggable(status int, header http.Header) bool {
	return status >= 200 && status < 300 && strings.HasPrefix(header.Get("Content-Type"), "text/html")
}

// weakETag is the first 64 bits of the body's SHA-256, which is collision-safe
// for this use.
func weakETag(body []byte) string {
	digest := sha256.Sum256(body)
	return `W/"` + hex.EncodeToString(digest[:8]) + `"`
}

// etagMatches is the RFC 7232 weak comparison: W/ is ignored on both sides.
func etagMatches(clientValue, serverValue string) bool {
	normalize := func(s string) string {
		s = strings.TrimSpace(s)
		return strings.TrimPrefix(s, "W/")
	}
	for _, entry := range strings.Split(clientValue, ",") {
		if normalize(entry) == normalize(serverValue) {
			return true
		}
	}
	return false
}
