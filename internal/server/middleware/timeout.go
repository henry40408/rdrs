package middleware

import (
	"context"
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
			tw := &timeoutWriter{ResponseWriter: w, header: http.Header{}}
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
				tw.finish()
			case <-timer.C:
				if tw.expire() {
					// The handler goroutine is abandoned: its context is cancelled
					// by the deferred cancel and every write it still makes is
					// dropped.
					return
				}
				<-done
				tw.finish()
			}
			select {
			case p := <-panicked:
				// Re-panic with the original value so http.ErrAbortHandler still
				// aborts the connection silently.
				panic(p)
			default:
			}
		})
	}
}

// timeoutWriter lets the timer and the handler race for the head; the loser's
// writes are dropped. The handler works on a private header map that is copied
// to the real writer under mu when the head goes out, as http.TimeoutHandler
// does: the handler may keep editing headers after the timer fired, and the
// outer layers edit the real map concurrently.
type timeoutWriter struct {
	http.ResponseWriter
	header   http.Header
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

// finish answers 200 for a handler that returned without starting a
// response, so the headers it set (a cookie removal) are not lost.
func (w *timeoutWriter) finish() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.headSent || w.timedOut {
		return
	}
	w.headSent = true
	w.publishHeader()
	w.ResponseWriter.WriteHeader(http.StatusOK)
}

// publishHeader copies the handler's headers to the real writer. The caller
// holds mu.
func (w *timeoutWriter) publishHeader() {
	dst := w.ResponseWriter.Header()
	for name, values := range w.header {
		dst[name] = append([]string(nil), values...)
	}
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
	w.publishHeader()
	w.ResponseWriter.WriteHeader(status)
}

func (w *timeoutWriter) Write(data []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.timedOut {
		return len(data), nil
	}
	if !w.headSent {
		w.publishHeader()
	}
	w.headSent = true
	return w.ResponseWriter.Write(data)
}

func (w *timeoutWriter) Flush() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.timedOut {
		return
	}
	if !w.headSent {
		w.publishHeader()
	}
	w.headSent = true
	_ = http.NewResponseController(w.ResponseWriter).Flush()
}

// Header is the handler's private map; only the handler goroutine touches it.
func (w *timeoutWriter) Header() http.Header { return w.header }
