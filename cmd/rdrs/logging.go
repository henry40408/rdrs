package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"strconv"
	"strings"
	"sync"
	"time"
)

type logFormat string

const (
	formatFull    logFormat = "full"
	formatCompact logFormat = "compact"
	formatPretty  logFormat = "pretty"
	formatJSON    logFormat = "json"
)

// defaultTarget labels records that carry no "target" attribute, mirroring the
// Rust crate name tracing printed.
const defaultTarget = "rdrs"

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
	// ansi colours the level in the text formats.
	ansi bool
}

func newLogger(settings loggerSettings, w io.Writer) *slog.Logger {
	return slog.New(&handler{settings: settings, w: w, mu: &sync.Mutex{}})
}

// handler renders records in the four formats the Rust build offered through
// tracing-subscriber. The json format keeps tracing's field names: timestamp,
// level, fields (with the message inside) and target.
type handler struct {
	settings loggerSettings
	w        io.Writer
	mu       *sync.Mutex
	attrs    []slog.Attr
	prefix   string
}

func (h *handler) Enabled(_ context.Context, level slog.Level) bool {
	return level >= h.settings.level
}

func (h *handler) WithAttrs(attrs []slog.Attr) slog.Handler {
	clone := *h
	clone.attrs = append(append([]slog.Attr(nil), h.attrs...), qualify(h.prefix, attrs)...)
	return &clone
}

func (h *handler) WithGroup(name string) slog.Handler {
	if name == "" {
		return h
	}
	clone := *h
	clone.prefix = h.prefix + name + "."
	return &clone
}

func qualify(prefix string, attrs []slog.Attr) []slog.Attr {
	out := make([]slog.Attr, 0, len(attrs))
	for _, a := range attrs {
		a.Key = prefix + a.Key
		out = append(out, a)
	}
	return out
}

// field is one rendered key/value pair of a record.
type field struct {
	key   string
	value slog.Value
}

func (h *handler) Handle(_ context.Context, r slog.Record) error {
	target := defaultTarget
	var fields []field
	collect := func(a slog.Attr) {
		if a.Key == "target" {
			target = a.Value.String()
			return
		}
		fields = append(fields, field{a.Key, a.Value.Resolve()})
	}
	for _, a := range h.attrs {
		collect(a)
	}
	r.Attrs(func(a slog.Attr) bool {
		a.Key = h.prefix + a.Key
		collect(a)
		return true
	})

	line := h.render(r, target, fields)
	h.mu.Lock()
	defer h.mu.Unlock()
	_, err := io.WriteString(h.w, line)
	return err
}

func (h *handler) render(r slog.Record, target string, fields []field) string {
	ts := r.Time.UTC().Format("2006-01-02T15:04:05.000000Z")
	switch h.settings.format {
	case formatJSON:
		return renderJSON(ts, r, target, fields)
	case formatPretty:
		return h.renderPretty(ts, r, target, fields)
	default:
		return h.renderLine(ts, r, target, fields)
	}
}

func (h *handler) levelLabel(level slog.Level) string {
	label := fmt.Sprintf("%5s", level.String())
	if !h.settings.ansi {
		return label
	}
	return "\x1b[" + levelColor(level) + "m" + label + "\x1b[0m"
}

func levelColor(level slog.Level) string {
	switch {
	case level >= slog.LevelError:
		return "31"
	case level >= slog.LevelWarn:
		return "33"
	case level >= slog.LevelInfo:
		return "32"
	}
	return "34"
}

func (h *handler) renderLine(ts string, r slog.Record, target string, fields []field) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s %s %s: %s", ts, h.levelLabel(r.Level), target, r.Message)
	for _, f := range fields {
		b.WriteString(" " + f.key + "=" + textValue(f.value))
	}
	b.WriteString("\n")
	return b.String()
}

func (h *handler) renderPretty(ts string, r slog.Record, target string, fields []field) string {
	var b strings.Builder
	fmt.Fprintf(&b, "  %s %s %s: %s\n", ts, h.levelLabel(r.Level), target, r.Message)
	for _, f := range fields {
		b.WriteString("    " + f.key + ": " + textValue(f.value) + "\n")
	}
	b.WriteString("\n")
	return b.String()
}

func textValue(v slog.Value) string {
	s := fmt.Sprint(v.Any())
	if s == "" || strings.ContainsAny(s, " \t\n\"=") {
		return strconv.Quote(s)
	}
	return s
}

func renderJSON(ts string, r slog.Record, target string, fields []field) string {
	var b strings.Builder
	b.WriteString(`{"timestamp":` + quote(ts) + `,"level":` + quote(r.Level.String()))
	b.WriteString(`,"fields":{"message":` + quote(r.Message))
	for _, f := range fields {
		b.WriteString("," + quote(f.key) + ":" + jsonValue(f.value))
	}
	b.WriteString(`},"target":` + quote(target) + "}\n")
	return b.String()
}

func quote(s string) string {
	encoded, _ := json.Marshal(s)
	return string(encoded)
}

func jsonValue(v slog.Value) string {
	switch v.Kind() {
	case slog.KindInt64, slog.KindUint64, slog.KindFloat64, slog.KindBool:
		return fmt.Sprint(v.Any())
	case slog.KindDuration:
		return quote(v.Duration().String())
	case slog.KindTime:
		return quote(v.Time().UTC().Format(time.RFC3339Nano))
	}
	// Errors and Stringers marshal to {} or private fields; print them instead.
	return quote(fmt.Sprint(v.Any()))
}
