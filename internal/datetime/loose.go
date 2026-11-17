package datetime

import (
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// This file replaces the Rust dateparser crate for the shapes feeds and the
// database produce. Like the Rust build, a value with no offset reads as UTC
// and a date with no time reads as midnight UTC, whatever the server's zone.
// Time-only values and dates with no year are not accepted: dateparser fills
// those from the clock, which would make a stored value change between runs.

var (
	// offsetSuffix splits a trailing "+0800", "+08:00" or "-08".
	offsetSuffix = regexp.MustCompile(`^(.*?)\s*([+-])(\d{2})(?::?(\d{2}))?$`)
	// clockEnd is a clock reading that ends the text, so a "-08" after it is
	// an offset and not the day of "2021-02-21".
	clockEnd = regexp.MustCompile(`\d:\d{2}(?::\d{2}(?:\.\d+)?)?(?:\s*[AaPp]\.?[Mm]\.?)?$`)
	// nameSuffix splits a trailing zone abbreviation.
	nameSuffix = regexp.MustCompile(`^(.*?)(?:\s+|(\d))([A-Za-z]{1,4})$`)
	// meridiem is a trailing am/pm in any spelling.
	meridiem = regexp.MustCompile(`(?i)(\d)\s*([ap])\.?m\.?$`)
)

// zoneAbbreviations are offsets in seconds, the set RFC 5322 defines plus UTC.
var zoneAbbreviations = map[string]int{
	"Z": 0, "UTC": 0, "UT": 0, "GMT": 0,
	"EST": -5 * 3600, "EDT": -4 * 3600,
	"CST": -6 * 3600, "CDT": -5 * 3600,
	"MST": -7 * 3600, "MDT": -6 * 3600,
	"PST": -8 * 3600, "PDT": -7 * 3600,
}

// parseLoose reads text as a UTC instant, or reports false.
func parseLoose(text string) (time.Time, bool) {
	text = strings.TrimSpace(text)
	if text == "" {
		return time.Time{}, false
	}
	if t, ok := parseUnix(text); ok {
		return t, true
	}
	if t, err := time.Parse(time.RFC3339, text); err == nil {
		return t.UTC(), true
	}
	body, offset := splitZone(text)
	body = meridiem.ReplaceAllStringFunc(body, upperMeridiem)
	for _, layout := range naiveLayouts() {
		t, err := time.Parse(layout, body)
		if err == nil {
			return t.Add(-time.Duration(offset) * time.Second).UTC(), true
		}
	}
	return time.Time{}, false
}

// parseUnix reads a bare Unix timestamp in seconds, milliseconds or
// nanoseconds, told apart by digit count as dateparser does.
func parseUnix(text string) (time.Time, bool) {
	if len(text) != 10 && len(text) != 13 && len(text) != 19 {
		return time.Time{}, false
	}
	n, err := strconv.ParseInt(text, 10, 64)
	if err != nil || text[0] == '+' || text[0] == '-' {
		return time.Time{}, false
	}
	switch len(text) {
	case 10:
		return time.Unix(n, 0).UTC(), true
	case 13:
		return time.UnixMilli(n).UTC(), true
	}
	return time.Unix(0, n).UTC(), true
}

// splitZone removes a trailing offset or zone abbreviation, returning the rest
// and the offset in seconds (0 when there is none).
func splitZone(text string) (string, int) {
	if m := offsetSuffix.FindStringSubmatch(text); m != nil && plausibleOffset(m) {
		return m[1], offsetSeconds(m[2], m[3], m[4])
	}
	if m := nameSuffix.FindStringSubmatch(text); m != nil {
		if offset, ok := zoneAbbreviations[strings.ToUpper(m[3])]; ok {
			return m[1] + m[2], offset
		}
	}
	return text, 0
}

// plausibleOffset decides whether the sign and digits found at the end are an
// offset. After a clock reading they are; after a bare date only a "+" is,
// since "2021-02-21" must not lose its day.
func plausibleOffset(m []string) bool {
	hours, _ := strconv.Atoi(m[3])
	minutes, _ := strconv.Atoi(m[4])
	if hours > 23 || minutes > 59 {
		return false
	}
	return clockEnd.MatchString(strings.TrimSpace(m[1])) || m[2] == "+"
}

func offsetSeconds(sign, hours, minutes string) int {
	h, _ := strconv.Atoi(hours)
	m, _ := strconv.Atoi(minutes)
	seconds := h*3600 + m*60
	if sign == "-" {
		return -seconds
	}
	return seconds
}

// upperMeridiem rewrites "3:04pm" as "3:04 PM", the only spelling the layouts
// accept.
func upperMeridiem(match string) string {
	parts := meridiem.FindStringSubmatch(match)
	return parts[1] + " " + strings.ToUpper(parts[2]) + "M"
}

var naiveLayouts = sync.OnceValue(func() []string {
	dates := []string{
		"2006-1-2", "2006/1/2", "2006.1.2", "1/2/2006", "1/2/06", "1.2.2006", "1.2.06",
		"2 Jan 2006", "2 January 2006", "Jan 2, 2006", "Jan 2 2006", "Jan. 2, 2006",
		"January 2, 2006", "Jan 2, 06", "Mon, 2 Jan 2006", "Monday, 2 Jan 2006",
		"Mon, 2 January 2006", "Mon, Jan 2, 2006", "Monday, January 2, 2006",
		"2006年1月2日", "060102",
	}
	clocks := []string{
		"15:04:05", "15:04", "3:04:05 PM", "3:04 PM", "15时04分05秒", "15时04分",
	}
	separators := []string{" ", ", ", "T", " at ", ""}
	var layouts []string
	for _, date := range dates {
		layouts = append(layouts, date)
		for _, clock := range clocks {
			for _, sep := range separators {
				layouts = append(layouts, date+sep+clock)
			}
		}
	}
	return layouts
})
