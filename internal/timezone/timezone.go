// Package timezone maps calendar dates to and from UTC instants for a user's
// timezone, which decides where the statistics page cuts one day from the
// next. Instants are stored in UTC everywhere.
package timezone

import (
	"time"
	// Embedded so a name parses the same on a host without zoneinfo.
	_ "time/tzdata"
)

// Default is what a user who never chose a timezone gets.
const Default = "UTC"

// Suggestions are the common choices offered in the settings form; any IANA
// name is accepted.
var Suggestions = []string{
	"UTC",
	"America/Los_Angeles",
	"America/New_York",
	"America/Sao_Paulo",
	"Europe/London",
	"Europe/Berlin",
	"Asia/Dubai",
	"Asia/Kolkata",
	"Asia/Shanghai",
	"Asia/Taipei",
	"Asia/Tokyo",
	"Australia/Sydney",
}

// Parse returns the zone for an IANA name such as "Asia/Taipei".
func Parse(name string) (*time.Location, bool) {
	// LoadLocation maps "" to UTC and "Local" to the host zone; neither is an
	// IANA name.
	if name == "" || name == "Local" || hasLowercaseSegment(name) || !hasExactCase(name) {
		return nil, false
	}
	loc, err := time.LoadLocation(name)
	if err != nil {
		return nil, false
	}
	return loc, true
}

// hasLowercaseSegment rejects "utc" and "asia/taipei". Every IANA name segment
// starts with a capital, but LoadLocation reads the host's zoneinfo first, and
// a case-insensitive file system (macOS) would accept the wrong case.
func hasLowercaseSegment(name string) bool {
	start := true
	for i := 0; i < len(name); i++ {
		if start && name[i] >= 'a' && name[i] <= 'z' {
			return true
		}
		start = name[i] == '/'
	}
	return false
}

// ParseOrUTC falls back to UTC for a stored value that no longer parses, so a
// display preference can never break a page render.
func ParseOrUTC(name string) *time.Location {
	if loc, ok := Parse(name); ok {
		return loc
	}
	return time.UTC
}

// sameRulesHorizonDays is how far ahead SameRules compares offsets: a year,
// so daylight-saving rules that differ for only a few weeks still count.
const sameRulesHorizonDays = 366

// SameRules reports whether a and b cut days at the same instants for the next
// year. It compares behaviour because the database lists aliases as separate
// zones (Asia/Calcutta and Asia/Kolkata), and browsers report either spelling.
func SameRules(a, b *time.Location, now time.Time) bool {
	if a.String() == b.String() {
		return true
	}
	for day := 0; day <= sameRulesHorizonDays; day++ {
		at := now.AddDate(0, 0, day)
		if offsetAt(a, at) != offsetAt(b, at) {
			return false
		}
	}
	return true
}

func offsetAt(loc *time.Location, at time.Time) int {
	_, offset := at.In(loc).Zone()
	return offset
}

// DateAt is the calendar date instant falls on in loc.
func DateAt(instant time.Time, loc *time.Location) Date {
	year, month, day := instant.In(loc).Date()
	return Date{Year: year, Month: month, Day: day}
}

// TodayIn is the current calendar date in loc.
func TodayIn(loc *time.Location) Date {
	return DateAt(time.Now(), loc)
}

// StartOfDayUTC is the UTC instant at which date begins in loc.
//
// Some zones skip midnight on a DST change (Sao Paulo before 2019); the day
// then begins at its first hour that exists. A few skipped a whole day (Apia,
// 2011-12-30); that date then begins where the next one does.
func StartOfDayUTC(date Date, loc *time.Location) time.Time {
	for ; ; date = date.AddDays(1) {
		if start, ok := firstExistingHour(date, loc); ok {
			return start.UTC()
		}
	}
}

// firstExistingHour is the earliest instant of date at an on-the-hour local
// time that exists in loc.
func firstExistingHour(date Date, loc *time.Location) (time.Time, bool) {
	for hour := 0; hour < 24; hour++ {
		wall := time.Date(date.Year, date.Month, date.Day, hour, 0, 0, 0, time.UTC)
		if instant, ok := earliestInstant(wall, loc); ok {
			return instant, true
		}
	}
	return time.Time{}, false
}

// earliestInstant is the first instant whose clock in loc reads wall (given as
// a UTC time carrying the clock reading). It is absent when a DST gap skips
// that reading, and the first of two when clocks go back. time.Date makes
// neither choice reliably, so every offset in effect nearby is tried.
func earliestInstant(wall time.Time, loc *time.Location) (time.Time, bool) {
	var best time.Time
	found := false
	for _, probe := range []time.Duration{-24 * time.Hour, 0, 24 * time.Hour} {
		offset := offsetAt(loc, wall.Add(probe))
		candidate := wall.Add(-time.Duration(offset) * time.Second)
		if offsetAt(loc, candidate) != offset {
			continue
		}
		if !found || candidate.Before(best) {
			best, found = candidate, true
		}
	}
	return best, found
}

// DayBoundsUTC is [start, end) of date in loc as UTC instants; a DST day is
// not 24 hours.
func DayBoundsUTC(date Date, loc *time.Location) (start, end time.Time) {
	return StartOfDayUTC(date, loc), StartOfDayUTC(date.AddDays(1), loc)
}
