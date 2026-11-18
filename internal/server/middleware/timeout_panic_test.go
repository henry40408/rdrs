package middleware

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

type headRecorder struct {
	*httptest.ResponseRecorder
	headWritten bool
}

func (r *headRecorder) WriteHeader(status int) {
	r.headWritten = true
	r.ResponseRecorder.WriteHeader(status)
}

// A panicking handler must not get a 200 head from Timeout: the outer request
// log would record a success for a connection that is aborted.
func TestTimeoutWritesNoHeadForAPanickingHandler(t *testing.T) {
	recorder := &headRecorder{ResponseRecorder: httptest.NewRecorder()}
	handler := Timeout(time.Second)(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { panic("boom") }))
	func() {
		defer func() { _ = recover() }()
		handler.ServeHTTP(recorder, httptest.NewRequest("GET", "/", nil))
	}()
	if recorder.headWritten {
		t.Errorf("a head was written (status %d) before the panic propagated", recorder.Code)
	}
}
