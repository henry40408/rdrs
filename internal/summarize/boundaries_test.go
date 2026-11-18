package summarize

import (
	"bytes"
	"context"
	"log/slog"
	"strings"
	"testing"
)

func TestKagiAcceptsStatus200And299(t *testing.T) {
	for _, status := range []int{200, 299} {
		server := kagiServer(t, status, `{"output_data":{"markdown":"Title: T\n\nbody"}}`, nil)
		result, err := summarizeAt(t, server, KagiConfig{SessionToken: "tok"})
		if err != nil || !result.Success {
			t.Errorf("status %d: %+v, %v", status, result, err)
		}
	}
}

func TestRecoverIncompleteIsSilentWhenNothingIsIncomplete(t *testing.T) {
	f := newWorkerFixture(t, kagiSettings, answer("x"))
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	count := RecoverIncomplete(context.Background(), f.worker.DB, NewQueue(), f.worker.Cache)

	if count != 0 || strings.Contains(logs.String(), "summary.recovering") {
		t.Errorf("count = %d, logs = %s", count, logs.String())
	}
}
