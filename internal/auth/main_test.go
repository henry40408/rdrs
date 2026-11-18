package auth

import (
	"log/slog"
	"os"
	"testing"
)

func TestMain(m *testing.M) {
	// Every test migrates a database; the per-migration log lines are noise.
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}
