package summarize

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/fetch"
)

var noRetry = fetch.RetryConfig{InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond}

func kagiServer(t *testing.T, status int, body string, seen *http.Request) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if seen != nil {
			*seen = *r
		}
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(server.Close)
	return server
}

func summarizeAt(t *testing.T, server *httptest.Server, config KagiConfig) (*Result, error) {
	t.Helper()
	return Summarize(context.Background(), Request{
		Config: config, URL: "https://example.com/post", Base: server.URL, Client: server.Client(), Retry: noRetry,
	})
}

func text(s *string) string {
	if s == nil {
		return "<nil>"
	}
	return *s
}

func TestIsConfigured(t *testing.T) {
	if !(KagiConfig{SessionToken: "t"}).IsConfigured() || (KagiConfig{}).IsConfigured() {
		t.Fatal("only the session token decides")
	}
}

func TestSummarySendsTokenAndQuery(t *testing.T) {
	var seen http.Request
	server := kagiServer(t, 200, `{"output_data":{"markdown":"Title: Foo Bar\n\nThe body."}}`, &seen)
	language := "ZH-HANT"
	result, err := summarizeAt(t, server, KagiConfig{SessionToken: "tok", Language: &language})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Success || text(result.OutputText) != "The body." || text(result.Title) != "Foo Bar" || result.Error != nil {
		t.Fatalf("result = %+v", result)
	}
	if seen.Header.Get("Authorization") != "tok" || seen.Header.Get("Content-Type") != "application/json" {
		t.Errorf("headers = %v", seen.Header)
	}
	if got := seen.URL.RawQuery; got != "summary_type=summary&url=https%3A%2F%2Fexample.com%2Fpost&target_language=ZH-HANT" {
		t.Errorf("query = %s", got)
	}
	if seen.URL.Path != "/mother/summary_labs" {
		t.Errorf("path = %s", seen.URL.Path)
	}
}

func TestEmptyLanguageIsOmitted(t *testing.T) {
	var seen http.Request
	server := kagiServer(t, 200, `{"output_data":{"markdown":"x"}}`, &seen)
	empty := ""
	if _, err := summarizeAt(t, server, KagiConfig{SessionToken: "tok", Language: &empty}); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(seen.URL.RawQuery, "target_language") {
		t.Errorf("query = %s", seen.URL.RawQuery)
	}
}

func TestMarkdownWithoutTitleKeepsEverything(t *testing.T) {
	for markdown, wantText := range map[string]string{
		`Plain body`:            "Plain body",
		`Title: only a title`:   "Title: only a title",
		`Title: T\n\nBody\n\nX`: "Body\n\nX",
	} {
		body := fmt.Sprintf(`{"output_data":{"markdown":%q}}`, strings.ReplaceAll(markdown, `\n`, "\n"))
		result, err := summarizeAt(t, kagiServer(t, 200, body, nil), KagiConfig{SessionToken: "t"})
		if err != nil || !result.Success {
			t.Fatalf("%q: %+v %v", markdown, result, err)
		}
		if text(result.OutputText) != wantText {
			t.Errorf("%q: text = %q, want %q", markdown, text(result.OutputText), wantText)
		}
	}
}

func TestNoErrorMemberStillYieldsTheSummary(t *testing.T) {
	config := KagiConfig{SessionToken: "t"}
	for _, body := range []string{
		`{"error":false,"output_data":{"markdown":"ok"}}`,
		`{"error":null,"output_data":{"markdown":"ok"}}`,
		`{"error":"","output_data":{"markdown":"ok"}}`,
	} {
		result, err := summarizeAt(t, kagiServer(t, 200, body, nil), config)
		if err != nil || !result.Success || text(result.OutputText) != "ok" {
			t.Errorf("%s: %+v %v", body, result, err)
		}
	}
}

func TestNotConfigured(t *testing.T) {
	result, err := Summarize(context.Background(), Request{URL: "https://x"})
	if err != nil || result.Success || text(result.Error) != "Kagi is not configured" {
		t.Fatalf("%+v %v", result, err)
	}
}

func TestBodyLevelFailures(t *testing.T) {
	config := KagiConfig{SessionToken: "t"}
	for body, want := range map[string]string{
		`{"error":"nope"}`:                  "nope",
		`{"error":true}`:                    "Kagi reported an error",
		`{"error":[{"msg":"bad"}]}`:         `[{"msg":"bad"}]`,
		`{"error":false}`:                   "No summary returned from Kagi",
		`{"error":null}`:                    "No summary returned from Kagi",
		`{}`:                                "No summary returned from Kagi",
		`{"output_data":{}}`:                "No summary returned from Kagi",
		`{"output_data":{"markdown":null}}`: "No summary returned from Kagi",
	} {
		result, err := summarizeAt(t, kagiServer(t, 200, body, nil), config)
		if err != nil || result.Success || text(result.Error) != want {
			t.Errorf("%s: %+v %v", body, result, err)
		}
	}
}

func TestStatusMessages(t *testing.T) {
	config := KagiConfig{SessionToken: "t"}
	for _, c := range []struct {
		status int
		body   string
		want   string
	}{
		{401, "", "Invalid session token"},
		{403, "", "Access forbidden - check your Kagi subscription"},
		{429, "", "Rate limit exceeded - please try again later"},
		{500, "boom", "Kagi error (500 Internal Server Error): boom"},
		{404, "gone", "Kagi error (404 Not Found): gone"},
	} {
		result, err := summarizeAt(t, kagiServer(t, c.status, c.body, nil), config)
		if err != nil || result.Success || text(result.Error) != c.want {
			t.Errorf("%d: %+v %v", c.status, result, err)
		}
	}
}

func TestMalformedJSONIsAnError(t *testing.T) {
	_, err := summarizeAt(t, kagiServer(t, 200, "{not json", nil), KagiConfig{SessionToken: "t"})
	var target *Error
	if !errors.As(err, &target) || !strings.HasPrefix(err.Error(), "Failed to parse Kagi response: ") {
		t.Fatalf("err = %v", err)
	}
}

func TestConnectionFailureIsAnError(t *testing.T) {
	server := kagiServer(t, 200, "{}", nil)
	server.Close()
	_, err := summarizeAt(t, server, KagiConfig{SessionToken: "t"})
	if err == nil || !strings.HasPrefix(err.Error(), "Failed to connect to Kagi: ") || errors.Unwrap(err) == nil {
		t.Fatalf("err = %v", err)
	}
}

func TestBaseComesFromTheEnvironment(t *testing.T) {
	var seen http.Request
	server := kagiServer(t, 200, `{"output_data":{"markdown":"ok"}}`, &seen)
	t.Setenv("RDRS_KAGI_API_BASE", server.URL)
	result, err := Summarize(context.Background(), Request{
		Config: KagiConfig{SessionToken: "t"}, URL: "https://x", Client: server.Client(), Retry: noRetry,
	})
	if err != nil || !result.Success || seen.URL.Path != "/mother/summary_labs" {
		t.Fatalf("%+v %v", result, err)
	}
}

func TestDefaultBaseIsKagi(t *testing.T) {
	t.Setenv("RDRS_KAGI_API_BASE", "")
	if got := summaryURL(Request{URL: "u"}); !strings.HasPrefix(got, "https://kagi.com/mother/summary_labs?") {
		t.Fatalf("url = %s", got)
	}
}

func TestTransientStatusIsNotRetried(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls++
		w.WriteHeader(500)
	}))
	defer server.Close()
	retry := fetch.RetryConfig{MaxRetries: 3, InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond}
	_, err := Summarize(context.Background(), Request{
		Config: KagiConfig{SessionToken: "t"}, URL: "u", Base: server.URL, Client: server.Client(), Retry: retry,
	})
	if err != nil || calls != 1 {
		t.Fatalf("calls = %d, err = %v (Rust retries errors only, not statuses)", calls, err)
	}
}
