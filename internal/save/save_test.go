package save

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
)

var noRetry = fetch.RetryConfig{InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond}

func ptr(s string) *string { return &s }

type seenRequest struct {
	method, path, authorization, contentType, body string
}

func linkdingServer(t *testing.T, status int, body string, seen *seenRequest) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if seen != nil {
			payload, _ := io.ReadAll(r.Body)
			*seen = seenRequest{r.Method, r.URL.Path, r.Header.Get("Authorization"), r.Header.Get("Content-Type"), string(payload)}
		}
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(server.Close)
	return server
}

func saveTo(t *testing.T, server *httptest.Server, bookmark BookmarkData) (*Result, error) {
	t.Helper()
	return SaveToLinkding(context.Background(), LinkdingRequest{
		Config:   LinkdingConfig{APIURL: server.URL, APIToken: "tok123"},
		Bookmark: bookmark, Client: server.Client(), Retry: noRetry,
	})
}

func TestSaveSuccess(t *testing.T) {
	var seen seenRequest
	server := linkdingServer(t, 201, `{"id":42}`, &seen)
	result, err := saveTo(t, server, BookmarkData{
		URL: "https://example.com/post", Title: ptr("Title"), Tags: []string{"rust"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Success || result.Service != "linkding" || result.Message != "Saved to Linkding" ||
		*result.BookmarkURL != server.URL+"/bookmarks/42" {
		t.Fatalf("result = %+v", result)
	}
	if seen.method != "POST" || seen.path != "/api/bookmarks/" || seen.authorization != "Token tok123" ||
		seen.contentType != "application/json" {
		t.Errorf("request = %+v", seen)
	}
	if seen.body != `{"url":"https://example.com/post","title":"Title","tag_names":["rust"]}` {
		t.Errorf("body = %s", seen.body)
	}
}

func TestEmptyOptionalFieldsAreOmitted(t *testing.T) {
	var seen seenRequest
	if _, err := saveTo(t, linkdingServer(t, 201, `{"id":1}`, &seen), BookmarkData{URL: "https://x/"}); err != nil {
		t.Fatal(err)
	}
	if seen.body != `{"url":"https://x/"}` {
		t.Errorf("body = %s", seen.body)
	}
}

func TestAnEmptyTitleIsStillSent(t *testing.T) {
	var seen seenRequest
	if _, err := saveTo(t, linkdingServer(t, 201, `{"id":1}`, &seen), BookmarkData{URL: "u", Description: ptr("")}); err != nil {
		t.Fatal(err)
	}
	if seen.body != `{"url":"u","description":""}` {
		t.Errorf("body = %s", seen.body)
	}
}

func TestNotConfigured(t *testing.T) {
	for _, config := range []LinkdingConfig{{}, {APIURL: "u"}, {APIToken: "t"}} {
		result, err := SaveToLinkding(context.Background(), LinkdingRequest{Config: config})
		if err != nil || result.Success || result.Message != "Linkding is not configured" || result.BookmarkURL != nil {
			t.Errorf("%+v: %+v %v", config, result, err)
		}
	}
}

func TestErrorStatusesBecomeFriendlyMessages(t *testing.T) {
	for _, c := range []struct {
		status int
		body   string
		want   string
	}{
		{400, `{"url":["already exists"]}`, "Bookmark already exists in Linkding"},
		{400, `unique constraint`, "Bookmark already exists in Linkding"},
		{400, "bad field", "Bad request: bad field"},
		{401, "", "Invalid API token"},
		{403, "", "Access forbidden"},
		{404, "", "Linkding API endpoint not found"},
		{500, "boom", "Linkding error (500 Internal Server Error): boom"},
		{418, "tea", "Linkding error (418 I'm a teapot): tea"},
	} {
		result, err := saveTo(t, linkdingServer(t, c.status, c.body, nil), BookmarkData{URL: "u"})
		if err != nil || result.Success || result.Message != c.want || result.BookmarkURL != nil {
			t.Errorf("%d %q: %+v %v", c.status, c.body, result, err)
		}
	}
}

func TestMalformedJSONIsAnError(t *testing.T) {
	_, err := saveTo(t, linkdingServer(t, 201, "{not json", nil), BookmarkData{URL: "u"})
	if err == nil || !strings.HasPrefix(err.Error(), "Failed to parse Linkding response: ") {
		t.Fatalf("err = %v", err)
	}
}

func TestConnectionFailureIsAnError(t *testing.T) {
	server := linkdingServer(t, 201, "{}", nil)
	server.Close()
	_, err := saveTo(t, server, BookmarkData{URL: "u"})
	var target *Error
	if !errors.As(err, &target) || !strings.HasPrefix(err.Error(), "Failed to connect to Linkding: ") {
		t.Fatalf("err = %v", err)
	}
}

func TestNormalizeAPIURL(t *testing.T) {
	for input, want := range map[string]string{
		"https://l.example.com":                    "https://l.example.com/api/bookmarks/",
		"https://l.example.com/":                   "https://l.example.com/api/bookmarks/",
		"https://l.example.com///":                 "https://l.example.com/api/bookmarks/",
		"https://l.example.com/api":                "https://l.example.com/api/bookmarks/",
		"https://l.example.com/api/bookmarks":      "https://l.example.com/api/bookmarks/",
		"https://l.example.com/api/bookmarks/":     "https://l.example.com/api/bookmarks/",
		"https://l.example.com/sub/api/other":      "https://l.example.com/sub/api/other/",
		"https://l.example.com/x/api/bookmarks/y/": "https://l.example.com/x/api/bookmarks/y/",
	} {
		if got := NormalizeAPIURL(input); got != want {
			t.Errorf("NormalizeAPIURL(%q) = %q, want %q", input, got, want)
		}
	}
}

func TestBookmarkPageURL(t *testing.T) {
	for input, want := range map[string]string{
		"https://l.example.com":                "https://l.example.com/bookmarks/123",
		"https://l.example.com/":               "https://l.example.com/bookmarks/123",
		"https://l.example.com/api/bookmarks/": "https://l.example.com/bookmarks/123",
		"https://l.example.com/sub/api":        "https://l.example.com/sub/bookmarks/123",
	} {
		if got := bookmarkPageURL(input, 123); got != want {
			t.Errorf("bookmarkPageURL(%q) = %q, want %q", input, got, want)
		}
	}
}

func TestServicesConfigRoundTrip(t *testing.T) {
	config, err := ParseServicesConfig(`{"linkding":{"api_url":"https://l","api_token":"t"},"unknown":1}`)
	if err != nil {
		t.Fatal(err)
	}
	if !config.HasAnyService() || strings.Join(config.ConfiguredServices(), ",") != "linkding" {
		t.Fatalf("config = %+v", config)
	}
	encoded, err := config.JSON()
	if err != nil || encoded != `{"linkding":{"api_url":"https://l","api_token":"t"},"kagi":null}` {
		t.Fatalf("encoded = %s, %v", encoded, err)
	}
}

func TestServicesConfigWithKagi(t *testing.T) {
	config, err := ParseServicesConfig(`{"kagi":{"session_token":"k"}}`)
	if err != nil {
		t.Fatal(err)
	}
	if config.HasAnyService() || len(config.ConfiguredServices()) != 0 {
		t.Fatal("Kagi is not a saving service")
	}
	if config.Kagi == nil || config.Kagi.SessionToken != "k" || config.Kagi.Language != nil {
		t.Fatalf("kagi = %+v", config.Kagi)
	}
	encoded, _ := config.JSON()
	if encoded != `{"linkding":null,"kagi":{"session_token":"k","language":null}}` {
		t.Fatalf("encoded = %s", encoded)
	}
}

func TestEmptyAndHalfFilledConfigHaveNoService(t *testing.T) {
	for _, stored := range []string{`{}`, `{"linkding":null}`, `{"linkding":{"api_url":"u","api_token":""}}`} {
		config, err := ParseServicesConfig(stored)
		if err != nil || config.HasAnyService() {
			t.Errorf("%s: %+v %v", stored, config, err)
		}
		if services := config.ConfiguredServices(); services == nil || len(services) != 0 {
			t.Errorf("%s: services = %#v", stored, services)
		}
	}
}

func TestInvalidServicesJSON(t *testing.T) {
	if _, err := ParseServicesConfig(`{`); err == nil {
		t.Fatal("expected an error")
	}
}

func TestConfigOfStoredServices(t *testing.T) {
	config := ConfigOf(models.StoredServices{JSON: `{"linkding":{"api_url":"u","api_token":"t"}}`})
	if !config.HasAnyService() {
		t.Fatal("stored document should parse")
	}
	for _, stored := range []models.StoredServices{{}, {JSON: "{garbage"}, {Undecryptable: true}} {
		if ConfigOf(stored).HasAnyService() {
			t.Errorf("%+v should read as empty", stored)
		}
	}
}

func TestUsableConfigOf(t *testing.T) {
	if _, err := UsableConfigOf(models.StoredServices{Undecryptable: true}); err == nil ||
		!strings.Contains(err.Error(), "cannot be decrypted with the current RDRS_SECRET") {
		t.Fatalf("err = %v", err)
	}
	config, err := UsableConfigOf(models.StoredServices{JSON: `{"kagi":{"session_token":"k"}}`})
	if err != nil || config.Kagi == nil {
		t.Fatalf("%+v %v", config, err)
	}
}
