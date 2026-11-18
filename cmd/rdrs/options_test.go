package main

import (
	"bytes"
	"log/slog"
	"testing"
)

func TestFlagBeatsEnvBeatsDefault(t *testing.T) {
	var sink bytes.Buffer
	cases := []struct {
		name string
		args []string
		env  map[string]string
		want logFormat
	}{
		{"default", nil, nil, formatFull},
		{"env", nil, map[string]string{"RDRS_LOG_FORMAT": "json"}, formatJSON},
		{"blank env is unset", nil, map[string]string{"RDRS_LOG_FORMAT": "  "}, formatFull},
		{"flag", []string{"--log-format", "pretty"}, nil, formatPretty},
		{"flag beats env", []string{"--log-format=compact"}, map[string]string{"RDRS_LOG_FORMAT": "json"}, formatCompact},
	}
	for _, tc := range cases {
		opts, err := parseOptions(tc.args, envOf(tc.env), &sink)
		if err != nil || opts.logFormat != tc.want {
			t.Errorf("%s: got %q, %v", tc.name, opts.logFormat, err)
		}
	}
}

func TestInvalidLogFormatFails(t *testing.T) {
	var sink bytes.Buffer
	if _, err := parseOptions([]string{"--log-format", "xml"}, envOf(nil), &sink); err == nil {
		t.Fatal("flag: want error")
	}
	if _, err := parseOptions(nil, envOf(map[string]string{"RDRS_LOG_FORMAT": "xml"}), &sink); err == nil {
		t.Fatal("env: want error")
	}
}

func TestUnknownFlagFails(t *testing.T) {
	var sink bytes.Buffer
	if _, err := parseOptions([]string{"--nope"}, envOf(nil), &sink); err == nil {
		t.Fatal("want error")
	}
}

func TestLogLevel(t *testing.T) {
	var sink bytes.Buffer
	for raw, want := range map[string]slog.Level{
		"": slog.LevelInfo, "info": slog.LevelInfo, "debug": slog.LevelDebug,
		"warn": slog.LevelWarn, "error": slog.LevelError, "DEBUG": slog.LevelDebug,
	} {
		opts, err := parseOptions(nil, envOf(map[string]string{"RDRS_LOG_LEVEL": raw}), &sink)
		if err != nil || opts.logLevel != want {
			t.Errorf("%q: got %v, %v", raw, opts.logLevel, err)
		}
	}
	// Rust's filter syntax and tracing's trace level are not accepted.
	for _, raw := range []string{"trace", "error,rdrs=info", "verbose"} {
		if _, err := parseOptions(nil, envOf(map[string]string{"RDRS_LOG_LEVEL": raw}), &sink); err == nil {
			t.Errorf("%q: want error", raw)
		}
	}
}
