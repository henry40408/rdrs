package main

import (
	"fmt"
	"io"
	"log/slog"
	"strings"
)

// logFormat keeps the four names the Rust build accepted. slog has only a text
// and a JSON layout, so full, compact and pretty all print text.
type logFormat string

const (
	formatFull    logFormat = "full"
	formatCompact logFormat = "compact"
	formatPretty  logFormat = "pretty"
	formatJSON    logFormat = "json"
)

func parseLogFormat(raw string) (logFormat, error) {
	switch f := logFormat(raw); f {
	case formatFull, formatCompact, formatPretty, formatJSON:
		return f, nil
	}
	return "", fmt.Errorf("invalid log format %q: expected one of full, compact, pretty, json", raw)
}

// parseLogLevel accepts only the four level names. Unlike RUST_LOG it has no
// per-module filter syntax: the Go build has no third-party crates to silence.
func parseLogLevel(raw string) (slog.Level, error) {
	switch strings.ToLower(strings.TrimSpace(raw)) {
	case "", "info":
		return slog.LevelInfo, nil
	case "debug":
		return slog.LevelDebug, nil
	case "warn":
		return slog.LevelWarn, nil
	case "error":
		return slog.LevelError, nil
	}
	return 0, fmt.Errorf("invalid RDRS_LOG_LEVEL %q: expected one of debug, info, warn, error", raw)
}

// loggerSettings are the inputs of newLogger.
type loggerSettings struct {
	format logFormat
	level  slog.Level
}

func newLogger(settings loggerSettings, w io.Writer) *slog.Logger {
	options := &slog.HandlerOptions{Level: settings.level}
	if settings.format == formatJSON {
		return slog.New(slog.NewJSONHandler(w, options))
	}
	return slog.New(slog.NewTextHandler(w, options))
}
