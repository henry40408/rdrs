package middleware

import (
	"context"
	"fmt"
	"net/http"
	"sync"
	"time"
)

// RequestTimeout is how long a handler has to produce a response head.
const RequestTimeout = 120 * time.Second

// Timeout answers 408 with an empty body when the handler has not started its
// response within limit. http.TimeoutHandler is no substitute: it answers
// 503 and buffers every body. Once the head is out the timeout no longer
// applies, as with tower-http's TimeoutLayer, so long downloads finish.
func Timeout(limit time.Duration) Middleware {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Only an expiry cancels the handler: after the head is out, the
			// body may legitimately take longer than limit.
			ctx, cancel := context.WithCancel(r.Context())
			defer cancel()
			tw := &timeoutWriter{ResponseWriter: w}
			panicked := make(chan any, 1)
			done := make(chan struct{})
			go func() {
				defer close(done)
				defer func() {
					if p := recover(); p != nil {
						panicked <- p
					}
				}()
				next.ServeHTTP(tw, r.WithContext(ctx))
			}()
			timer := time.NewTimer(limit)
			defer timer.Stop()
			select {
			case <-done:
			case <-timer.C:
				if tw.expire() {
					// The handler goroutine is abandoned: its context is cancelled
					// by the deferred cancel and every write it still makes is
					// dropped.
					return
				}
				<-done
			}
			select {
			case p := <-panicked:
				panic(fmt.Sprint(p))
			default:
			}
		})
	}
}

// timeoutWriter lets the timer and the handler race for the head; the loser's
// writes are dropped.
type timeoutWriter struct {
	http.ResponseWriter
	mu       sync.Mutex
	headSent bool
	timedOut bool
}

// expire answers 408 unless the handler already started its response, and
// reports whether it did.
func (w *timeoutWriter) expire() bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.headSent {
		return false
	}
	w.timedOut = true
	w.headSent = true
	w.ResponseWriter.WriteHeader(http.StatusRequestTimeout)
	return true
}

func (w *timeoutWriter) WriteHeader(status int) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.timedOut {
		return
	}
	if status >= 200 {
		w.headSent = true
	}
	w.ResponseWriter.WriteHeader(status)
}

func (w *timeoutWriter) Write(data []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.timedOut {
		return len(data), nil
	}
	w.headSent = true
	return w.ResponseWriter.Write(data)
}

func (w *timeoutWriter) Flush() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if !w.timedOut {
		w.headSent = true
		_ = http.NewResponseController(w.ResponseWriter).Flush()
	}
}

// Header is shared with the real writer; after a timeout the handler may still
// edit it, which would race with the server, so it gets a private copy.
func (w *timeoutWriter) Header() http.Header {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.timedOut {
		return http.Header{}
	}
	return w.ResponseWriter.Header()
}

func (w *timeoutWriter) Unwrap() http.ResponseWriter { return w.ResponseWriter }
