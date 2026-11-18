package middleware

import (
	"log/slog"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

func TestHeaderTokenAcceptsExactlyTheVisibleAsciiRange(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name  string
		value string
		want  bool
	}{
		{"space is the lowest visible byte", " ", true},
		{"tilde is the highest visible byte", "~", true},
		{"tab is allowed", "a\tb", true},
		{"unit separator is a control byte", "a\x1fb", false},
		{"nul is a control byte", "\x00", false},
		{"delete is past the visible range", "a\x7fb", false},
		{"non ascii", "caf\xc3\xa9", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			request := httptest.NewRequest(http.MethodPost, "/", nil)
			request.Header.Set(csrfHeader, tc.value)
			got, present := headerToken(request)
			if present != tc.want || (tc.want && got != tc.value) {
				t.Errorf("headerToken(%q) = %q, %v; want present %v", tc.value, got, present, tc.want)
			}
		})
	}
}

func TestSlowRequestLogReportsMillisecondsAndTheThreshold(t *testing.T) {
	records := captureLogs(t)
	logCompleted(completedRequest{method: "GET", route: "/x", status: 200, elapsed: 2500 * time.Millisecond})
	logCompleted(completedRequest{method: "GET", route: "/x", status: 200, elapsed: 250 * time.Millisecond})
	got := *records
	if len(got) != 2 {
		t.Fatalf("%d records", len(got))
	}
	if got[0].level != slog.LevelWarn || got[0].attrs["elapsed_ms"] != 2500.0 || got[0].attrs["threshold_ms"] != 1000.0 {
		t.Errorf("slow record = %+v", got[0])
	}
	if got[1].attrs["elapsed_ms"] != 250.0 {
		t.Errorf("fast record = %+v", got[1])
	}
}

// statusLog keeps every status the wrapped writer was given.
type statusLog struct {
	mu       sync.Mutex
	header   http.Header
	statuses []int
}

func newStatusLog() *statusLog { return &statusLog{header: http.Header{}} }

func (r *statusLog) Header() http.Header { return r.header }
func (r *statusLog) Write(data []byte) (int, error) {
	return len(data), nil
}
func (r *statusLog) WriteHeader(status int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.statuses = append(r.statuses, status)
}
func (r *statusLog) written() []int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]int(nil), r.statuses...)
}

func TestTimeoutIgnoresInformationalHeadsButHonoursTheFinalOne(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name    string
		status  int
		linger  time.Duration
		release bool
		want    []int
	}{
		// A 1xx head is not the response: the timer still answers 408.
		{"early hints", http.StatusEarlyHints, 0, true, []int{http.StatusEarlyHints, http.StatusRequestTimeout}},
		// A 200 head ends the race even when the handler then outlasts the limit.
		{"final 200", http.StatusOK, 100 * time.Millisecond, false, []int{http.StatusOK}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			recorder := newStatusLog()
			handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(tc.status)
				if tc.release {
					select {
					case <-r.Context().Done():
					case <-time.After(5 * time.Second):
					}
					return
				}
				time.Sleep(tc.linger)
			})
			Timeout(20*time.Millisecond)(handler).ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/", nil))
			got := recorder.written()
			if len(got) != len(tc.want) {
				t.Fatalf("statuses = %v, want %v", got, tc.want)
			}
			for i := range got {
				if got[i] != tc.want[i] {
					t.Fatalf("statuses = %v, want %v", got, tc.want)
				}
			}
		})
	}
}

func TestHeadEditsRunForAnExplicit200Head(t *testing.T) {
	t.Parallel()
	handler := HSTS("max-age=1")(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/", nil))
	// Result snapshots the header as it stood when the head was written.
	if got := recorder.Result().Header.Get("Strict-Transport-Security"); got != "max-age=1" {
		t.Errorf("HSTS = %q, an edit after the head is lost", got)
	}
}

func TestTaggableStatusesStopBelow300(t *testing.T) {
	t.Parallel()
	html := http.Header{"Content-Type": {"text/html"}}
	for status, want := range map[int]bool{199: false, 200: true, 299: true, 300: false} {
		if got := isTaggable(status, html); got != want {
			t.Errorf("isTaggable(%d) = %v, want %v", status, got, want)
		}
	}
}

func TestQValueAllowsThreeDecimalsAndNoMore(t *testing.T) {
	t.Parallel()
	cases := []struct {
		text   string
		want   int
		wantOK bool
	}{
		{"q=0.123", 123, true},
		{"q=0.1234", 0, false},
		{"q=1.000", 1000, true},
		{"q=1.001", 0, false},
	}
	for _, tc := range cases {
		got, ok := parseQValue(tc.text)
		if ok != tc.wantOK || (ok && got != tc.want) {
			t.Errorf("parseQValue(%q) = %d, %v; want %d, %v", tc.text, got, ok, tc.want, tc.wantOK)
		}
	}
}

func TestDeferredWriterTreatsZeroMaxBodyAsNoLimit(t *testing.T) {
	t.Parallel()
	for _, spill := range []bool{false, true} {
		recorder := httptest.NewRecorder()
		writer := &deferredWriter{
			ResponseWriter: recorder, spill: spill,
			shouldBuffer: func(int, http.Header) bool { return true },
		}
		writer.WriteHeader(http.StatusOK)
		if _, err := writer.Write([]byte("body")); err != nil {
			t.Fatal(err)
		}
		if writer.overflow || writer.body.String() != "body" || recorder.Body.Len() != 0 {
			t.Errorf("spill %v: overflow %v, buffered %q, passed through %q",
				spill, writer.overflow, writer.body.String(), recorder.Body.String())
		}
	}
}

func TestDeferredWriterBuffersAFinal200Head(t *testing.T) {
	t.Parallel()
	recorder := httptest.NewRecorder()
	writer := &deferredWriter{ResponseWriter: recorder, shouldBuffer: func(int, http.Header) bool { return true }}
	writer.WriteHeader(http.StatusOK)
	if !writer.decided || !writer.buffering || writer.status != http.StatusOK {
		t.Errorf("a 200 head was treated as informational: decided %v buffering %v status %d",
			writer.decided, writer.buffering, writer.status)
	}
}

func TestFlashValueWithAnEmptyPayloadStillVerifies(t *testing.T) {
	t.Parallel()
	root := []byte("0123456789abcdef0123456789abcdef")
	value := flashSignatureSeparator + flashSignature(root, "")
	payload, ok := verifyFlashValue(value, root)
	if !ok || payload != "" {
		t.Errorf("verifyFlashValue(%q) = %q, %v", value, payload, ok)
	}
	if _, ok := verifyFlashValue("nosignature", root); ok {
		t.Error("a value without a separator verified")
	}
}
