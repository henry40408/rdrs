package middleware

import (
	"net/http"
	"strings"
	"testing"
)

func TestCompressVariesOnIdentityToo(t *testing.T) {
	rec := serve(Compress(respond(200, "text/html", strings.Repeat("x", 100))), "GET", "/")
	if rec.Header().Get("Vary") != "Accept-Encoding" || rec.Header().Get("Content-Encoding") != "" {
		t.Errorf("headers %v", rec.Header())
	}
	rec = serve(Compress(respond(200, "image/png", "png")), "GET", "/")
	if rec.Header().Get("Vary") != "" {
		t.Errorf("an uncompressible type must not vary: %v", rec.Header())
	}
}

func TestCompressSendsOversizedBodiesUncompressed(t *testing.T) {
	huge := strings.Repeat("a", maxCompressBody+1)
	rec := serve(Compress(respond(200, "application/json", huge)), "GET", "/", "Accept-Encoding", "gzip")
	if rec.Header().Get("Content-Encoding") != "" || rec.Body.Len() != len(huge) || rec.Code != 200 {
		t.Errorf("status %d, encoding %q, %d bytes", rec.Code, rec.Header().Get("Content-Encoding"), rec.Body.Len())
	}
	exact := strings.Repeat("a", maxCompressBody)
	rec = serve(Compress(respond(200, "application/json", exact)), "GET", "/", "Accept-Encoding", "gzip")
	if rec.Header().Get("Content-Encoding") != "gzip" {
		t.Error("a body of exactly the cap is still compressed")
	}
}

func TestETagOverflowDropsTheHandlersHeaders(t *testing.T) {
	handler := ETag(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html")
		w.Header().Set("Set-Cookie", "a=b")
		w.Header().Set("Content-Length", "99")
		_, _ = w.Write([]byte(strings.Repeat("x", maxETagBody+1)))
	}))
	rec := serve(handler, "GET", "/")
	if rec.Code != 500 || len(rec.Header()) != 0 {
		t.Errorf("status %d headers %v", rec.Code, rec.Header())
	}
}
