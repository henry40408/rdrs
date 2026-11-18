package fetch

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestValidateURLBoundaries(t *testing.T) {
	cases := []struct {
		raw  string
		want error
	}{
		{"http://127.0.0.1./", ErrBlockedHost},
		{"http://10.0.0.1./", ErrPrivateIP},
		{"http://4294967296/", ErrBlockedHost},
		{"http://4294967295/", ErrPrivateIP},
		{"http://1.16777216/", ErrBlockedHost},
		{"http://1.16777215/", nil},
		{"http://1.2.65536/", ErrBlockedHost},
		{"http://1.2.65535/", nil},
		{"http://1.2.3.256/", ErrBlockedHost},
		{"http://1.2.3.255/", nil},
		{"http://example.0xzz/", nil},
	}
	for _, tc := range cases {
		if err := ValidateURL(mustURL(t, tc.raw)); !errors.Is(err, tc.want) {
			t.Errorf("%s: got %v, want %v", tc.raw, err, tc.want)
		}
	}
}

func TestRedirectCapIsReachedAtTheLimit(t *testing.T) {
	request := func() *http.Request { return httptest.NewRequest(http.MethodGet, "http://a.example/", nil) }
	via := func(n int) []*http.Request {
		hops := make([]*http.Request, n)
		for i := range hops {
			hops[i] = request()
		}
		return hops
	}
	if err := DropAuthorizationOnOriginChange(request(), via(maxRedirects-1)); err != nil {
		t.Errorf("below the cap: %v", err)
	}
	if err := DropAuthorizationOnOriginChange(request(), via(maxRedirects)); err == nil {
		t.Error("at the cap: no error")
	}
}

func TestTransportTimeouts(t *testing.T) {
	transport := NewFetcher(Policy{}).Client(false).Transport.(*http.Transport)
	if transport.IdleConnTimeout != 90*time.Second || transport.TLSHandshakeTimeout != 10*time.Second {
		t.Errorf("idle %v, handshake %v", transport.IdleConnTimeout, transport.TLSHandshakeTimeout)
	}
}

func captureLogs(t *testing.T) *bytes.Buffer {
	t.Helper()
	var buf bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&buf, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return &buf
}

func assertRetryLog(t *testing.T, logs string) {
	t.Helper()
	for _, want := range []string{
		"attempt=1 attempts=4 backoff_ms=10", "attempt=2 attempts=4 backoff_ms=20", "attempt=3 attempts=4 backoff_ms=40",
	} {
		if !strings.Contains(logs, want) {
			t.Errorf("missing %q in %s", want, logs)
		}
	}
}

var slowRetry = RetryConfig{MaxRetries: 3, InitialBackoff: 10 * time.Millisecond, MaxBackoff: 40 * time.Millisecond}

func TestRetryLogsAttemptsAndDoublingBackoff(t *testing.T) {
	logs := captureLogs(t)
	_, _ = Retry(context.Background(), slowRetry, func() Attempt[string] {
		return Attempt[string]{Err: errors.New("failing"), Transient: true}
	})
	assertRetryLog(t, logs.String())
}

func TestSendWithRetryLogsAttemptsAndDoublingBackoff(t *testing.T) {
	logs := captureLogs(t)
	server, _ := flaky(100, http.StatusServiceUnavailable)
	defer server.Close()
	params := sendParams(server, true)
	params.Config = slowRetry
	resp, err := SendWithRetry(context.Background(), params)
	if err == nil && resp != nil {
		_ = resp.Body.Close()
	}
	assertRetryLog(t, logs.String())
}
