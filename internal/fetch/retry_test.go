package fetch

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"
)

var instantRetry = RetryConfig{MaxRetries: 2, InitialBackoff: time.Millisecond, MaxBackoff: 2 * time.Millisecond}

func TestIsTransientStatus(t *testing.T) {
	for _, status := range []int{500, 502, 503, 504, 429, 408} {
		if !IsTransientStatus(status) {
			t.Errorf("%d should be transient", status)
		}
	}
	for _, status := range []int{200, 301, 400, 401, 403, 404} {
		if IsTransientStatus(status) {
			t.Errorf("%d should not be transient", status)
		}
	}
}

func TestRetryConfigs(t *testing.T) {
	if got := DefaultRetry(); got != (RetryConfig{3, time.Second, 8 * time.Second}) {
		t.Errorf("default = %+v", got)
	}
	if got := IconRetry(); got != (RetryConfig{2, 500 * time.Millisecond, 4 * time.Second}) {
		t.Errorf("icon = %+v", got)
	}
}

func runRetry(failures int, transient bool) (attempts int, value string, err error) {
	value, err = Retry(context.Background(), instantRetry, func() Attempt[string] {
		attempts++
		if attempts <= failures {
			return Attempt[string]{Err: errors.New("failing"), Transient: transient}
		}
		return Attempt[string]{Value: "ok"}
	})
	return attempts, value, err
}

func TestRetryImmediateSuccess(t *testing.T) {
	if attempts, value, err := runRetry(0, true); attempts != 1 || value != "ok" || err != nil {
		t.Errorf("%d %q %v", attempts, value, err)
	}
}

func TestRetryTransientThenSuccess(t *testing.T) {
	if attempts, value, err := runRetry(2, true); attempts != 3 || value != "ok" || err != nil {
		t.Errorf("%d %q %v", attempts, value, err)
	}
}

func TestRetryExhausted(t *testing.T) {
	if attempts, _, err := runRetry(10, true); attempts != 3 || err == nil {
		t.Errorf("%d %v", attempts, err)
	}
}

func TestRetryPermanentErrorShortCircuits(t *testing.T) {
	if attempts, _, err := runRetry(10, false); attempts != 1 || err == nil {
		t.Errorf("%d %v", attempts, err)
	}
}

func TestRetryStopsWhenTheContextEnds(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	slow := RetryConfig{MaxRetries: 5, InitialBackoff: time.Hour, MaxBackoff: time.Hour}
	_, err := Retry(ctx, slow, func() Attempt[int] { return Attempt[int]{Err: errors.New("x"), Transient: true} })
	if !errors.Is(err, context.Canceled) {
		t.Errorf("got %v", err)
	}
}

func sendParams(server *httptest.Server, retryStatus bool) SendParams {
	return SendParams{
		Client: server.Client(), Config: instantRetry, RetryStatus: retryStatus,
		Build: func(ctx context.Context) (*http.Request, error) {
			return http.NewRequestWithContext(ctx, http.MethodGet, server.URL, nil)
		},
	}
}

func flaky(failuresBeforeOK int32, status int) (*httptest.Server, *atomic.Int32) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		if calls.Add(1) <= failuresBeforeOK {
			w.WriteHeader(status)
			return
		}
		_, _ = w.Write([]byte("ok"))
	}))
	return server, &calls
}

func TestSendWithRetryRetriesAStatusOnlyWhenAsked(t *testing.T) {
	server, calls := flaky(1, http.StatusServiceUnavailable)
	defer server.Close()
	response, err := SendWithRetry(context.Background(), sendParams(server, false))
	if err != nil || response.StatusCode != 503 || calls.Load() != 1 {
		t.Fatalf("without RetryStatus: %v, calls %d", err, calls.Load())
	}
	_ = response.Body.Close()

	calls.Store(0)
	response, err = SendWithRetry(context.Background(), sendParams(server, true))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = response.Body.Close() }()
	body, _ := io.ReadAll(response.Body)
	if string(body) != "ok" || calls.Load() != 2 {
		t.Errorf("body %q, calls %d", body, calls.Load())
	}
}

func TestSendWithRetryReturnsTheLastErrorStatusWhenExhausted(t *testing.T) {
	server, calls := flaky(100, http.StatusBadGateway)
	defer server.Close()
	response, err := SendWithRetry(context.Background(), sendParams(server, true))
	if err != nil {
		t.Fatal(err)
	}
	_ = response.Body.Close()
	if response.StatusCode != 502 || calls.Load() != 3 {
		t.Errorf("status %d, calls %d", response.StatusCode, calls.Load())
	}
}

func TestSendWithRetryDoesNotRetryAPermanentStatus(t *testing.T) {
	server, calls := flaky(100, http.StatusNotFound)
	defer server.Close()
	response, err := SendWithRetry(context.Background(), sendParams(server, true))
	if err != nil {
		t.Fatal(err)
	}
	_ = response.Body.Close()
	if response.StatusCode != 404 || calls.Load() != 1 {
		t.Errorf("status %d, calls %d", response.StatusCode, calls.Load())
	}
}

func TestSendWithRetryRetriesATransportErrorThenGivesUp(t *testing.T) {
	server := httptest.NewServer(http.NotFoundHandler())
	params := sendParams(server, false)
	server.Close()
	var builds atomic.Int32
	build := params.Build
	params.Build = func(ctx context.Context) (*http.Request, error) { builds.Add(1); return build(ctx) }
	if _, err := SendWithRetry(context.Background(), params); err == nil {
		t.Fatal("expected an error")
	}
	if builds.Load() != 3 {
		t.Errorf("attempts = %d", builds.Load())
	}
}

func TestSendWithRetryDoesNotRetryABuildError(t *testing.T) {
	var builds int
	params := SendParams{Config: instantRetry, Build: func(context.Context) (*http.Request, error) {
		builds++
		return nil, errors.New("bad request")
	}}
	if _, err := SendWithRetry(context.Background(), params); err == nil || builds != 1 {
		t.Errorf("builds %d, err %v", builds, err)
	}
}
