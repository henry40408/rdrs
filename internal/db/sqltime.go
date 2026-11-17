package db

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// Never bind a time.Time directly: the driver picks its own text form, and
// SQLite compares these columns as text. Format first with one of the two
// functions below, matching what the Rust build wrote.

const spaceLayout = "2006-01-02 15:04:05"

// FormatSpace renders t as "YYYY-MM-DD HH:MM:SS" in UTC, the form entry
// timestamps and datetime('now') use.
func FormatSpace(t time.Time) string {
	return t.UTC().Format(spaceLayout)
}

// FormatRustRFC3339 renders t as chrono's RFC 3339 with the AutoSi fraction:
// "+00:00" for the zone, and a fraction of 3, 6 or 9 digits only when the
// sub-second part is non-zero.
func FormatRustRFC3339(t time.Time) string {
	t = t.UTC()
	nanos := t.Nanosecond()
	switch {
	case nanos == 0:
		return t.Format("2006-01-02T15:04:05+00:00")
	case nanos%1_000_000 == 0:
		return t.Format("2006-01-02T15:04:05.000+00:00")
	case nanos%1_000 == 0:
		return t.Format("2006-01-02T15:04:05.000000+00:00")
	default:
		return t.Format("2006-01-02T15:04:05.000000000+00:00")
	}
}

// storedLayouts are the textual forms found in existing databases. Go accepts a
// fraction after the seconds field even when the layout has none.
var storedLayouts = []string{
	time.RFC3339Nano,
	"2006-01-02T15:04:05",
	"2006-01-02 15:04:05Z07:00",
	spaceLayout,
	"2006-01-02 15:04",
	"2006-01-02T15:04",
}

const (
	julianUnixEpoch = 2440587.5
	secondsPerDay   = 86400
)

// ParseStored decodes a timestamp column value. It accepts every form SQLite
// may hold: RFC 3339 text, space-separated text, an INTEGER of unix seconds, a
// FLOAT Julian day, and the time.Time modernc returns for TIMESTAMP columns.
func ParseStored(v any) (time.Time, error) {
	switch x := v.(type) {
	case time.Time:
		return x.UTC(), nil
	case string:
		return parseStoredText(x)
	case []byte:
		return parseStoredText(string(x))
	case int64:
		return time.Unix(x, 0).UTC(), nil
	case float64:
		return julianToTime(x), nil
	default:
		return time.Time{}, fmt.Errorf("db: cannot parse %T as a timestamp", v)
	}
}

func parseStoredText(s string) (time.Time, error) {
	s = strings.TrimSpace(s)
	for _, layout := range storedLayouts {
		if t, err := time.Parse(layout, s); err == nil {
			return t.UTC(), nil
		}
	}
	// A numeric string is what a TEXT-affinity column holds for the same values.
	if n, err := strconv.ParseInt(s, 10, 64); err == nil {
		return time.Unix(n, 0).UTC(), nil
	}
	if f, err := strconv.ParseFloat(s, 64); err == nil {
		return julianToTime(f), nil
	}
	return time.Time{}, fmt.Errorf("db: unrecognized timestamp %q", s)
}

func julianToTime(day float64) time.Time {
	seconds := (day - julianUnixEpoch) * secondsPerDay
	whole := int64(seconds)
	nanos := int64((seconds - float64(whole)) * 1e9)
	return time.Unix(whole, nanos).UTC().Round(time.Millisecond)
}
