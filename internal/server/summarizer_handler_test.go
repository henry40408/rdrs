package server

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

// withKagi stores a Kagi session token for the signed-in admin and points the
// Kagi client at stub, which is the only host the test talks to.
func (f *signedInFixture) withKagi(t *testing.T, stub http.HandlerFunc) {
	t.Helper()
	server := httptest.NewServer(stub)
	t.Cleanup(server.Close)
	t.Setenv("RDRS_KAGI_API_BASE", server.URL)
	f.configureKagi(t)
}

func (f *signedInFixture) configureKagi(t *testing.T) {
	t.Helper()
	_, err := models.UpdateSaveServices(context.Background(), f.database, 1,
		`{"linkding":null,"kagi":{"session_token":"tok","language":null}}`, f.app.serviceKey())
	if err != nil {
		t.Fatal(err)
	}
}

func (f *signedInFixture) summarizeItem(link string) *httptest.ResponseRecorder {
	return f.postForm("/summarizer/item", url.Values{"url": {link}, "index": {"0"}})
}

func answerJSON(body string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(body))
	}
}

func TestSummarizerItemCompletesThroughKagi(t *testing.T) {
	f := newSignedIn(t)
	var token, target atomic.Value
	f.withKagi(t, func(w http.ResponseWriter, r *http.Request) {
		token.Store(r.Header.Get("Authorization"))
		target.Store(r.URL.Query().Get("url"))
		answerJSON(`{"output_data":{"markdown":"Title: Hello\n\nBody text."}}`)(w, r)
	})
	rec := f.summarizeItem("https://a.com/x")
	body := rec.Body.String()
	if rec.Code != http.StatusOK || !strings.Contains(body, `data-state="completed"`) ||
		!strings.Contains(body, "Hello") || !strings.Contains(body, "Body text.") {
		t.Fatalf("= %d %s", rec.Code, body)
	}
	if token.Load() != "tok" || target.Load() != "https://a.com/x" {
		t.Fatalf("Kagi saw token %v for %v", token.Load(), target.Load())
	}
}

func TestSummarizerItemShowsKagiFailuresAsErrorCards(t *testing.T) {
	cases := map[string]struct {
		stub http.HandlerFunc
		want string
	}{
		"bad token":    {func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusUnauthorized) }, "Invalid session token"},
		"rate limited": {func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusTooManyRequests) }, "Rate limit exceeded"},
		"kagi error":   {answerJSON(`{"error":"Page unreachable"}`), "Page unreachable"},
		"empty answer": {answerJSON(`{}`), "No summary returned from Kagi"},
		"not json":     {answerJSON(`<html>`), "Failed to parse Kagi response"},
	}
	for name, c := range cases {
		f := newSignedIn(t)
		f.withKagi(t, c.stub)
		rec := f.summarizeItem("https://a.com/x")
		body := rec.Body.String()
		if rec.Code != http.StatusOK || !strings.Contains(body, `data-state="error"`) || !strings.Contains(body, c.want) {
			t.Errorf("%s = %d %s", name, rec.Code, body)
		}
	}
}

func TestSummarizerItemRefusesWithoutCallingKagi(t *testing.T) {
	f := newSignedIn(t)
	var calls atomic.Int32
	f.withKagi(t, func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		answerJSON(`{}`)(w, r)
	})
	for link, want := range map[string]string{
		"ftp://a.com/x":      "Not a valid URL.",
		"not a url":          "Not a valid URL.",
		"http://127.0.0.1/x": "URL not allowed.",
	} {
		body := f.summarizeItem(link).Body.String()
		if !strings.Contains(body, `data-state="error"`) || !strings.Contains(body, want) {
			t.Errorf("%s = %s", link, body)
		}
	}
	if calls.Load() != 0 {
		t.Fatalf("Kagi was called %d times for refused links", calls.Load())
	}
}

func TestSummarizerItemWithoutKagiSaysSo(t *testing.T) {
	f := newSignedIn(t)
	body := f.summarizeItem("https://a.com/x").Body.String()
	if !strings.Contains(body, "Kagi is not configured.") {
		t.Fatalf("= %s", body)
	}
}

func TestSummarizerItemAllowsOneCallPerUserAtATime(t *testing.T) {
	f := newSignedIn(t)
	arrived, release := make(chan struct{}), make(chan struct{})
	f.withKagi(t, func(w http.ResponseWriter, r *http.Request) {
		close(arrived)
		<-release
		answerJSON(`{"output_data":{"markdown":"Done"}}`)(w, r)
	})
	first := make(chan *httptest.ResponseRecorder, 1)
	go func() { first <- f.summarizeItem("https://a.com/one") }()
	<-arrived
	second := f.summarizeItem("https://a.com/two").Body.String()
	close(release)
	if !strings.Contains(second, "Another summary is already in progress") {
		t.Errorf("second = %s", second)
	}
	if body := (<-first).Body.String(); !strings.Contains(body, `data-state="completed"`) {
		t.Errorf("first = %s", body)
	}
}

func TestSummarizerPageOffersTheFormOnlyWithKagi(t *testing.T) {
	f := newSignedIn(t)
	const form = `data-testid="summarizer-form"`
	if strings.Contains(f.get("/summarizer").Body.String(), form) {
		t.Fatal("the form is shown without Kagi")
	}
	f.configureKagi(t)
	if !strings.Contains(f.get("/summarizer").Body.String(), form) {
		t.Fatal("the form is missing with Kagi configured")
	}
}
