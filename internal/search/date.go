package search

import (
	"strconv"
	"strings"
	"time"
	"unicode"
)

// Year bounds of chrono's NaiveDate, which the Rust build parses with.
const (
	minYear = -262144
	maxYear = 262143
)

// parseChronoDate accepts what chrono's parse_from_str(value, "%Y-%m-%d")
// accepts: leading whitespace before each number, an unsigned year of 1 to 4
// digits or a signed year of any length, and a 1 or 2 digit month and day.
// The result is midnight UTC.
func parseChronoDate(value string) (time.Time, bool) {
	year, rest, ok := scanYear(value)
	if !ok {
		return time.Time{}, false
	}
	rest, ok = strings.CutPrefix(rest, "-")
	if !ok {
		return time.Time{}, false
	}
	month, rest, ok := scanNumber(rest, 2)
	if !ok {
		return time.Time{}, false
	}
	rest, ok = strings.CutPrefix(rest, "-")
	if !ok {
		return time.Time{}, false
	}
	day, rest, ok := scanNumber(rest, 2)
	if !ok || rest != "" || year < minYear || year > maxYear {
		return time.Time{}, false
	}
	date := time.Date(year, time.Month(month), day, 0, 0, 0, 0, time.UTC)
	if date.Year() != year || int(date.Month()) != month || date.Day() != day {
		return time.Time{}, false
	}
	return date, true
}

func scanYear(s string) (year int, rest string, ok bool) {
	s = strings.TrimLeftFunc(s, unicode.IsSpace)
	negative := strings.HasPrefix(s, "-")
	if negative || strings.HasPrefix(s, "+") {
		year, rest, ok = scanNumber(s[1:], -1)
		if negative {
			year = -year
		}
		return year, rest, ok
	}
	return scanNumber(s, 4)
}

// scanNumber reads one or more ASCII digits, at most maxDigits of them when
// maxDigits is positive. It skips leading whitespace first.
func scanNumber(s string, maxDigits int) (value int, rest string, ok bool) {
	s = strings.TrimLeftFunc(s, unicode.IsSpace)
	end := 0
	for end < len(s) && s[end] >= '0' && s[end] <= '9' && (maxDigits < 0 || end < maxDigits) {
		end++
	}
	if end == 0 {
		return 0, s, false
	}
	value, err := strconv.Atoi(s[:end])
	if err != nil {
		return 0, s, false
	}
	return value, s[end:], true
}
