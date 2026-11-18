package server

import (
	"errors"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// writeError logs through the default logger, so this test swaps it and
// therefore must not run in parallel.
func TestWriteErrorLogsServerFailuresWithACause(t *testing.T) {
	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	cause := errors.New("disk on fire")

	cases := []struct {
		name   string
		err    error
		logged bool
	}{
		{"500 with a cause", &AppError{Status: http.StatusInternalServerError, Message: "x", Cause: cause}, true},
		{"503 with a cause", &AppError{Status: http.StatusServiceUnavailable, Message: "x", Cause: cause}, true},
		{"499 with a cause", &AppError{Status: 499, Message: "x", Cause: cause}, false},
		{"404 with a cause", &AppError{Status: http.StatusNotFound, Message: "x", Cause: cause}, false},
		{"500 without a cause", &AppError{Status: http.StatusInternalServerError, Message: "x"}, false},
		{"a plain error", cause, true},
	}
	for _, tc := range cases {
		before := strings.Count(logs.String(), "http.error")
		writeError(httptest.NewRecorder(), tc.err)
		logged := strings.Count(logs.String(), "http.error") - before
		if (logged == 1) != tc.logged || logged > 1 {
			t.Errorf("%s: %d log lines, want logged=%v", tc.name, logged, tc.logged)
		}
	}
}
