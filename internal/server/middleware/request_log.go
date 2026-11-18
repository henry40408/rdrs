package middleware

import (
	"context"
	"log/slog"
	"net/http"
	"sync/atomic"
	"time"
)

// SlowRequestThreshold is when a request logs at WARN as http.slow_request.
const SlowRequestThreshold = time.Second

// UnmatchedRoute labels requests no route matched; never the request path,
// which an attacker controls.
const UnmatchedRoute = "<unmatched>"

type routeSlotKey struct{}

// RequestLog logs one event per request when its response head is ready, so
// SSE streams read as fast. The route label is the matched template, never
// the path, because paths can carry credentials (/invite/{token}). Install it
// outermost so every response is timed exactly once.
func RequestLog(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		started := time.Now()
		// Atomic because a timed-out handler may still call SetRoute.
		var route atomic.Value
		route.Store(UnmatchedRoute)
		r = r.WithContext(context.WithValue(r.Context(), routeSlotKey{}, &route))
		logHead := func(_ http.Header, status int) {
			logCompleted(completedRequest{method: r.Method, route: route.Load().(string), status: status, elapsed: time.Since(started)})
		}
		withHead(next, logHead).ServeHTTP(w, r)
	})
}

// SetRoute records the matched route template for RequestLog. Without a
// RequestLog above it, it does nothing.
func SetRoute(r *http.Request, template string) {
	if slot, ok := r.Context().Value(routeSlotKey{}).(*atomic.Value); ok {
		slot.Store(template)
	}
}

type completedRequest struct {
	method  string
	route   string
	status  int
	elapsed time.Duration
}

func logCompleted(req completedRequest) {
	elapsedMS := float64(req.elapsed) / float64(time.Millisecond)
	if req.elapsed >= SlowRequestThreshold {
		slog.Warn("request exceeded the slow-request threshold", "event", "http.slow_request",
			"method", req.method, "route", req.route, "status", req.status,
			"elapsed", req.elapsed, "elapsed_ms", elapsedMS,
			"threshold_ms", float64(SlowRequestThreshold)/float64(time.Millisecond))
		return
	}
	slog.Debug("request completed", "event", "http.request",
		"method", req.method, "route", req.route, "status", req.status,
		"elapsed", req.elapsed, "elapsed_ms", elapsedMS)
}
