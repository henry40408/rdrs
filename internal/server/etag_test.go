package server

import (
	"net/http"
	"strings"
	"testing"
)

// Port of tests/etag_test.rs. The client keeps cookies so the two requests of
// the 304 test share one anonymous session: /login renders that session's CSRF
// token, so a cookie-less client would get a different body every time.

func TestHtmlResponseCarriesWeakEtag(t *testing.T) {
	response := defaultITestApp(t).Get("/login")
	if response.Code != 200 {
		t.Fatalf("status %d", response.Code)
	}
	etag := response.Header().Get("ETag")
	if !strings.HasPrefix(etag, `W/"`) || !strings.HasSuffix(etag, `"`) {
		t.Fatalf("expected a weak ETag, got %q", etag)
	}
}

func TestMatchingIfNoneMatchReturns304(t *testing.T) {
	app := defaultITestApp(t)
	first := app.Get("/login")
	etag := first.Header().Get("ETag")
	if first.Code != 200 || etag == "" {
		t.Fatalf("first response: %d, ETag %q", first.Code, etag)
	}

	second := app.Do(itestRequest{Path: "/login", Headers: map[string]string{"If-None-Match": etag}})

	if second.Code != http.StatusNotModified {
		t.Fatalf("status %d, want 304", second.Code)
	}
	if second.Header().Get("ETag") != etag {
		t.Errorf("304 must echo the ETag, got %q", second.Header().Get("ETag"))
	}
	if second.Body.Len() != 0 {
		t.Error("304 has no body")
	}
}

func TestNonHtmlResponseHasNoEtag(t *testing.T) {
	response := defaultITestApp(t).Get("/favicon.svg")
	if response.Code != 200 {
		t.Fatalf("status %d", response.Code)
	}
	if etag := response.Header().Get("ETag"); etag != "" {
		t.Errorf("non-HTML responses must not be tagged, got %q", etag)
	}
}

func TestNonMatchingIfNoneMatchReturnsFullBody(t *testing.T) {
	response := defaultITestApp(t).Do(itestRequest{Path: "/login", Headers: map[string]string{"If-None-Match": `W/"deadbeef"`}})
	if response.Code != 200 || response.Body.Len() == 0 {
		t.Fatalf("status %d, %d body bytes", response.Code, response.Body.Len())
	}
}

// The ETag layer runs on the response path of NoStoreForAuthenticated: both
// headers must show up together on an anonymous HTML page.
func TestEtagStillEmittedUnderNoStore(t *testing.T) {
	response := defaultITestApp(t).Get("/login")
	if response.Code != 200 {
		t.Fatalf("status %d", response.Code)
	}
	if response.Header().Get("ETag") == "" {
		t.Error("the HTML response must still be tagged")
	}
	if got := response.Header().Get("Cache-Control"); got != "no-store" {
		t.Errorf("Cache-Control = %q", got)
	}
}
