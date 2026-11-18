package fetch

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"time"
)

// Timeouts shared by the callers of this package.
const (
	// DefaultTimeout is for general HTTP requests.
	DefaultTimeout = 30 * time.Second
	// IconTimeout is for icon fetching.
	IconTimeout = 10 * time.Second
	// ExternalAPITimeout is for calls to Linkding and Kagi.
	ExternalAPITimeout = 60 * time.Second
	// FeedSyncTimeout covers one feed's fetch, retries, icon and DB writes.
	FeedSyncTimeout = 90 * time.Second
)

// RetryConfig is how often and how patiently an operation is retried.
type RetryConfig struct {
	MaxRetries     int
	InitialBackoff time.Duration
	MaxBackoff     time.Duration
}

// DefaultRetry is three retries, backing off from 1 s to 8 s.
func DefaultRetry() RetryConfig {
	return RetryConfig{MaxRetries: 3, InitialBackoff: time.Second, MaxBackoff: 8 * time.Second}
}

// IconRetry is fewer, shorter retries: a missing favicon must not hold a sync
// open.
func IconRetry() RetryConfig {
	return RetryConfig{MaxRetries: 2, InitialBackoff: 500 * time.Millisecond, MaxBackoff: 4 * time.Second}
}

// Attempt is the outcome of one try. Transient marks a failure that is safe to
// replay.
type Attempt[T any] struct {
	Value     T
	Err       error
	Transient bool
}

// Retry runs operation with exponential backoff while it reports a transient
// failure, and returns the error of a permanent one or of the last attempt.
func Retry[T any](ctx context.Context, config RetryConfig, operation func() Attempt[T]) (T, error) {
	backoff := config.InitialBackoff
	for attempt := 0; ; attempt++ {
		result := operation()
		if result.Err == nil {
			return result.Value, nil
		}
		if !result.Transient || attempt >= config.MaxRetries {
			return result.Value, result.Err
		}
		slog.Warn("transient error, retrying", "event", "http.retry", "reason", "error",
			"attempt", attempt+1, "attempts", config.MaxRetries+1, "backoff_ms", backoff.Milliseconds())
		if err := sleep(ctx, backoff); err != nil {
			return result.Value, err
		}
		backoff = min(backoff*2, config.MaxBackoff)
	}
}

func sleep(ctx context.Context, d time.Duration) error {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}

// IsTransientError is whether a failed send is safe to replay: the request
// never reached a decision. A refused redirect or a cancelled context is a
// decision, so it is not.
func IsTransientError(err error) bool {
	var refused *RedirectRefusedError
	return !errors.As(err, &refused) && !errors.Is(err, context.Canceled)
}

// IsTransientStatus is a status worth retrying: 5xx, 429 and 408.
func IsTransientStatus(status int) bool {
	return status >= 500 || status == http.StatusTooManyRequests || status == http.StatusRequestTimeout
}

// SendParams describes a SendWithRetry call.
type SendParams struct {
	Client *http.Client
	Config RetryConfig
	// Build makes a fresh request per attempt, as a body is consumed by a send.
	Build func(ctx context.Context) (*http.Request, error)
	// RetryStatus also retries a transient status; the final response may
	// still carry an error status.
	RetryStatus bool
}

// SendWithRetry sends the request, retrying transient errors and, when asked,
// transient statuses. A non-transient status comes back as a response. The
// caller closes the response body.
func SendWithRetry(ctx context.Context, params SendParams) (*http.Response, error) {
	backoff := params.Config.InitialBackoff
	for attempt := 0; ; attempt++ {
		result := params.sendOnce(ctx)
		if !result.retry || attempt >= params.Config.MaxRetries {
			return result.response, result.err
		}
		reason := "error"
		if result.response != nil {
			reason = "status"
			_ = result.response.Body.Close()
		}
		slog.Warn("transient failure, retrying", "event", "http.retry", "reason", reason,
			"attempt", attempt+1, "attempts", params.Config.MaxRetries+1, "backoff_ms", backoff.Milliseconds())
		if err := sleep(ctx, backoff); err != nil {
			return nil, err
		}
		backoff = min(backoff*2, params.Config.MaxBackoff)
	}
}

// sendResult is one attempt's outcome and whether it is worth retrying.
type sendResult struct {
	response *http.Response
	err      error
	retry    bool
}

func (p SendParams) sendOnce(ctx context.Context) sendResult {
	request, err := p.Build(ctx)
	if err != nil {
		return sendResult{err: err}
	}
	response, err := p.Client.Do(request)
	if err != nil {
		return sendResult{err: err, retry: IsTransientError(err)}
	}
	return sendResult{response: response, retry: p.RetryStatus && IsTransientStatus(response.StatusCode)}
}
