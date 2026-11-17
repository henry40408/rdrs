// Package datetime parses the timestamps feeds and the database hold, and
// formats them for display.
package datetime

import (
	"strconv"
	"strings"
	"time"
)

// displayLayout is how a UTC timestamp is shown to the user. It matches the
// database text format but is independent of it: cursor and read_after values
// must keep matching datetime('now'), so do not reuse this for them.
const displayLayout = "2006-01-02 15:04:05"

const secondsPerDay = 24 * 3600

var chineseMonths = map[string]int{
	"一月": 1, "二月": 2, "三月": 3, "四月": 4, "五月": 5, "六月": 6,
	"七月": 7, "八月": 8, "九月": 9, "十月": 10, "十一月": 11, "十二月": 12,
}

// FormatDisplay shows dt in UTC as "2006-01-02 15:04:05".
func FormatDisplay(dt time.Time) string {
	return dt.UTC().Format(displayLayout)
}

// TryParse reads a stored datetime (RFC 3339, SQL datetime, loose formats,
// Chinese), or reports false. Use it where falling back to now would corrupt
// the result.
func TryParse(s string) (time.Time, bool) {
	if t, err := time.Parse(time.RFC3339, s); err == nil {
		return t.UTC(), true
	}
	if t, err := time.Parse(displayLayout, s); err == nil {
		return t, true
	}
	if t, ok := parseLoose(s); ok {
		return t, true
	}
	return parseChinese(s)
}

// ParseOrNow is TryParse falling back to the current time.
func ParseOrNow(s string) time.Time {
	if t, ok := TryParse(s); ok {
		return t
	}
	return time.Now().UTC()
}

// ParseTimestamp is the feed timestamp parser: loose formats, plus "+08:00"
// offsets in RFC 2822 dates and Chinese dates such as
// "週二, 6 一月 2026 14:28:00 +0000".
func ParseTimestamp(text string) (time.Time, bool) {
	if t, ok := parseLoose(text); ok {
		return t, true
	}
	if normalized := NormalizeTimezoneFormat(text); normalized != text {
		if t, ok := parseLoose(normalized); ok {
			return t, true
		}
	}
	return parseChinese(text)
}

// NormalizeTimezoneFormat turns a "+08:00" suffix into "+0800"; some feeds put
// ISO 8601 offsets in RFC 2822 dates.
func NormalizeTimezoneFormat(text string) string {
	text = strings.TrimSpace(text)
	cut := len(text) - 6
	if cut < 0 {
		return text
	}
	tail := text[cut:]
	if (tail[0] != '+' && tail[0] != '-') || tail[3] != ':' {
		return text
	}
	digits := tail[1:3] + tail[4:6]
	if !allASCIIDigits(digits) {
		return text
	}
	return text[:cut] + tail[:1] + tail[1:3] + tail[4:6]
}

func allASCIIDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

// parseChinese reads "週二, 6 一月 2026 14:28:00 +0000", weekday and offset
// optional.
func parseChinese(s string) (time.Time, bool) {
	s = strings.TrimSpace(s)
	if _, rest, found := strings.Cut(s, ", "); found {
		s = rest
	}
	parts := strings.SplitN(s, " ", 4)
	if len(parts) < 4 {
		return time.Time{}, false
	}
	day, dayErr := strconv.Atoi(parts[0])
	month, monthOK := chineseMonths[parts[1]]
	year, yearErr := strconv.Atoi(parts[2])
	if dayErr != nil || !monthOK || yearErr != nil {
		return time.Time{}, false
	}
	clock, offsetText, _ := strings.Cut(parts[3], " ")
	hour, minute, second, ok := parseClock(clock)
	if !ok {
		return time.Time{}, false
	}
	t := time.Date(year, time.Month(month), day, hour, minute, second, 0, time.UTC)
	if t.Day() != day {
		return time.Time{}, false
	}
	if offset, ok := parseOffset(offsetText); ok {
		// A whole-day offset is no zone at all; the Rust build gave up on the
		// date rather than reading it as UTC.
		if offset <= -secondsPerDay || offset >= secondsPerDay {
			return time.Time{}, false
		}
		t = t.Add(-time.Duration(offset) * time.Second)
	}
	return t, true
}

// parseClock reads "H:M:S" with one or two digits per field.
func parseClock(s string) (hour, minute, second int, ok bool) {
	fields := strings.Split(s, ":")
	if len(fields) != 3 {
		return 0, 0, 0, false
	}
	var values [3]int
	for i, field := range fields {
		if len(field) == 0 || len(field) > 2 || !allASCIIDigits(field) {
			return 0, 0, 0, false
		}
		values[i], _ = strconv.Atoi(field)
	}
	if values[0] > 23 || values[1] > 59 || values[2] > 59 {
		return 0, 0, 0, false
	}
	return values[0], values[1], values[2], true
}

// parseOffset reads an offset such as "+0800" into seconds.
func parseOffset(s string) (int, bool) {
	s = strings.TrimSpace(s)
	if len(s) < 5 || (s[0] != '+' && s[0] != '-') {
		return 0, false
	}
	hours, minutes := s[1:3], s[3:5]
	if !allASCIIDigits(hours) || !allASCIIDigits(minutes) {
		return 0, false
	}
	return offsetSeconds(s[:1], hours, minutes), true
}
