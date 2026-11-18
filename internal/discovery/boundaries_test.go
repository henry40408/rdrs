package discovery

import (
	"context"
	"net/http"
	"testing"
)

func TestStatus299IsAccepted(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		w.WriteHeader(299)
		_, _ = w.Write([]byte(rssBody))
	}})
	if _, err := Discover(context.Background(), loopbackParams(t, server.URL)); err != nil {
		t.Fatalf("status 299 rejected: %v", err)
	}
}
