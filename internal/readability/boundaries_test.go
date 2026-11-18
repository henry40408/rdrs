package readability

import (
	"context"
	"net/http"
	"testing"
)

func TestStatus299IsAccepted(t *testing.T) {
	server := serve(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(299)
		_, _ = w.Write([]byte(`<html><head><title>T</title></head><body><article><p>` +
			`A paragraph of article text that is long enough to be kept by the extractor.</p></article></body></html>`))
	})
	if _, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL)); err != nil {
		t.Fatalf("status 299 rejected: %v", err)
	}
}
