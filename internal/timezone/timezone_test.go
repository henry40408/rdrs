package timezone

import (
	"encoding/json"
	"os"
	"testing"
	"time"
)

func mustParse(t *testing.T, name string) *time.Location {
	t.Helper()
	loc, ok := Parse(name)
	if !ok {
		t.Fatalf("Parse(%q) failed", name)
	}
	return loc
}

func ymd(y int, m time.Month, d int) Date { return Date{Year: y, Month: m, Day: d} }

func utc(y int, m time.Month, d, h, min int) time.Time {
	return time.Date(y, m, d, h, min, 0, 0, time.UTC)
}

func TestDayBoundsSpanTheLocalDay(t *testing.T) {
	start, end := DayBoundsUTC(ymd(2026, 5, 1), mustParse(t, "Asia/Taipei"))
	if !start.Equal(utc(2026, 4, 30, 16, 0)) || !end.Equal(utc(2026, 5, 1, 16, 0)) {
		t.Errorf("got [%v, %v)", start, end)
	}
}

func TestDayBoundsAreNot24HoursAcrossDST(t *testing.T) {
	newYork := mustParse(t, "America/New_York")
	hours := func(date Date) time.Duration {
		start, end := DayBoundsUTC(date, newYork)
		return end.Sub(start) / time.Hour
	}
	if got := hours(ymd(2026, 3, 8)); got != 23 {
		t.Errorf("spring forward = %d hours", got)
	}
	if got := hours(ymd(2026, 11, 1)); got != 25 {
		t.Errorf("fall back = %d hours", got)
	}
	if got := hours(ymd(2026, 6, 1)); got != 24 {
		t.Errorf("ordinary day = %d hours", got)
	}
}

func TestSameRules(t *testing.T) {
	now := utc(2026, 11, 4, 0, 0)
	cases := []struct {
		name string
		a, b string
		want bool
	}{
		{"alias", "Asia/Calcutta", "Asia/Kolkata", true},
		{"alias etc", "Etc/UTC", "UTC", true},
		{"same offset all year", "Asia/Taipei", "Asia/Shanghai", true},
		{"one hour apart", "Asia/Taipei", "Asia/Tokyo", false},
		{"equal in winter only", "Europe/London", "Africa/Abidjan", false},
		{"differ for a few weeks", "Africa/Cairo", "Asia/Jerusalem", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := SameRules(mustParse(t, tc.a), mustParse(t, tc.b), now); got != tc.want {
				t.Errorf("SameRules(%s, %s) = %v", tc.a, tc.b, got)
			}
		})
	}
}

func TestParseAcceptsIANANamesOnly(t *testing.T) {
	for _, name := range []string{"Asia/Taipei", "UTC"} {
		if _, ok := Parse(name); !ok {
			t.Errorf("Parse(%q) should succeed", name)
		}
	}
	for _, name := range []string{"", "Local", "Mars/Olympus_Mons", "CST-8", " UTC", "utc", "../etc/passwd"} {
		if _, ok := Parse(name); ok {
			t.Errorf("Parse(%q) should fail", name)
		}
	}
}

func TestEverySuggestionParses(t *testing.T) {
	found := false
	for _, name := range Suggestions {
		mustParse(t, name)
		found = found || name == Default
	}
	if !found {
		t.Error("Default is not among the suggestions")
	}
}

func TestParseOrUTCFallsBackForAStaleValue(t *testing.T) {
	if got := ParseOrUTC("Asia/Taipei").String(); got != "Asia/Taipei" {
		t.Errorf("got %s", got)
	}
	if got := ParseOrUTC("Not/AZone"); got != time.UTC {
		t.Errorf("got %v", got)
	}
}

func TestDateAtFollowsTheZoneAcrossMidnight(t *testing.T) {
	lateEvening := utc(2026, 1, 5, 20, 0)
	cases := map[string]Date{
		"UTC":                 ymd(2026, 1, 5),
		"Asia/Taipei":         ymd(2026, 1, 6),
		"America/Los_Angeles": ymd(2026, 1, 5),
	}
	for name, want := range cases {
		if got := DateAt(lateEvening, mustParse(t, name)); got != want {
			t.Errorf("%s: got %v, want %v", name, got, want)
		}
	}
}

func TestStartOfDay(t *testing.T) {
	cases := []struct {
		name string
		zone string
		date Date
		want time.Time
	}{
		{"east of Greenwich", "Asia/Taipei", ymd(2026, 1, 6), utc(2026, 1, 5, 16, 0)},
		{"west of Greenwich", "America/Los_Angeles", ymd(2026, 1, 6), utc(2026, 1, 6, 8, 0)},
		{"UTC", "UTC", ymd(2026, 1, 6), utc(2026, 1, 6, 0, 0)},
		{"winter", "America/New_York", ymd(2026, 1, 15), utc(2026, 1, 15, 5, 0)},
		{"summer", "America/New_York", ymd(2026, 7, 15), utc(2026, 7, 15, 4, 0)},
		// Sao Paulo moved its clocks from 00:00 to 01:00, so the day began at
		// 01:00 local (UTC-2).
		{"skipped midnight", "America/Sao_Paulo", ymd(2018, 11, 4), utc(2018, 11, 4, 3, 0)},
		// Samoa jumped from 2011-12-29 to 2011-12-31.
		{"skipped day", "Pacific/Apia", ymd(2011, 12, 31), utc(2011, 12, 30, 10, 0)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := StartOfDayUTC(tc.date, mustParse(t, tc.zone)); !got.Equal(tc.want) {
				t.Errorf("got %v, want %v", got, tc.want)
			}
		})
	}
}

func TestASkippedCalendarDayBeginsWhereTheNextOneDoes(t *testing.T) {
	apia := mustParse(t, "Pacific/Apia")
	skipped := StartOfDayUTC(ymd(2011, 12, 30), apia)
	next := StartOfDayUTC(ymd(2011, 12, 31), apia)
	if !skipped.Equal(next) {
		t.Errorf("skipped %v != next %v", skipped, next)
	}
}

func TestStartOfDayPicksTheFirstRepeatedHour(t *testing.T) {
	// Clocks in Havana go back from 01:00 to 00:00, so local midnight happens
	// twice on 2026-11-01; the day starts at the first one (UTC-4).
	havana := mustParse(t, "America/Havana")
	got := StartOfDayUTC(ymd(2026, 11, 1), havana)
	if want := utc(2026, 11, 1, 4, 0); !got.Equal(want) {
		t.Errorf("got %v, want %v", got, want)
	}
}

func TestDateArithmetic(t *testing.T) {
	d := ymd(2024, 2, 28)
	if got := d.AddDays(2); got != ymd(2024, 3, 1) {
		t.Errorf("leap day: %v", got)
	}
	if got := d.DaysUntil(ymd(2024, 3, 1)); got != 2 {
		t.Errorf("DaysUntil = %d", got)
	}
	if !d.Before(ymd(2024, 2, 29)) || d.Before(d) {
		t.Error("Before")
	}
	if got := ymd(2026, 5, 1).String(); got != "2026-05-01" {
		t.Errorf("String = %s", got)
	}
	if _, ok := ParseDate("2026-13-01"); ok {
		t.Error("month 13 parsed")
	}
}

type vectors struct {
	Default string `json:"default"`
	DateAt  []struct {
		At   string `json:"at"`
		Date string `json:"date"`
		Tz   string `json:"tz"`
	} `json:"date_at"`
	DayBounds []struct {
		Date       string `json:"date"`
		End        string `json:"end"`
		Start      string `json:"start"`
		StartOfDay string `json:"start_of_day"`
		Tz         string `json:"tz"`
	} `json:"day_bounds"`
	Parse []struct {
		Name   string  `json:"name"`
		OrUTC  string  `json:"or_utc"`
		Parsed *string `json:"parsed"`
	} `json:"parse"`
	SameRules []struct {
		A    string `json:"a"`
		B    string `json:"b"`
		Now  string `json:"now"`
		Same bool   `json:"same"`
	} `json:"same_rules"`
	Suggestions []string `json:"suggestions"`
}

func loadVectors(t *testing.T) vectors {
	t.Helper()
	raw, err := os.ReadFile("../../testdata/vectors/timezone.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func mustTime(t *testing.T, s string) time.Time {
	t.Helper()
	got, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func TestRustVectors(t *testing.T) {
	v := loadVectors(t)
	if v.Default != Default {
		t.Errorf("default %q", v.Default)
	}
	if len(v.Suggestions) != len(Suggestions) {
		t.Fatalf("suggestions: %v", v.Suggestions)
	}
	for i, name := range v.Suggestions {
		if Suggestions[i] != name {
			t.Errorf("suggestion %d: %q != %q", i, Suggestions[i], name)
		}
	}
	for _, c := range v.DateAt {
		got := DateAt(mustTime(t, c.At), mustParse(t, c.Tz)).String()
		if got != c.Date {
			t.Errorf("DateAt(%s, %s) = %s, want %s", c.At, c.Tz, got, c.Date)
		}
	}
	for _, c := range v.DayBounds {
		date, _ := ParseDate(c.Date)
		loc := mustParse(t, c.Tz)
		start, end := DayBoundsUTC(date, loc)
		if !start.Equal(mustTime(t, c.Start)) || !end.Equal(mustTime(t, c.End)) {
			t.Errorf("DayBounds(%s, %s) = [%v, %v), want [%s, %s)", c.Date, c.Tz, start, end, c.Start, c.End)
		}
		if got := start.Format("2006-01-02 15:04:05"); got != c.StartOfDay {
			t.Errorf("StartOfDay(%s, %s) = %s, want %s", c.Date, c.Tz, got, c.StartOfDay)
		}
	}
	for _, c := range v.Parse {
		loc, ok := Parse(c.Name)
		if ok != (c.Parsed != nil) || (ok && loc.String() != *c.Parsed) {
			t.Errorf("Parse(%q) = %v, %v; want %v", c.Name, loc, ok, c.Parsed)
		}
		if got := ParseOrUTC(c.Name).String(); got != c.OrUTC {
			t.Errorf("ParseOrUTC(%q) = %s, want %s", c.Name, got, c.OrUTC)
		}
	}
	for _, c := range v.SameRules {
		got := SameRules(mustParse(t, c.A), mustParse(t, c.B), mustTime(t, c.Now))
		if got != c.Same {
			t.Errorf("SameRules(%s, %s, %s) = %v", c.A, c.B, c.Now, got)
		}
	}
}

// DaysUntil is the number of calendar days from d to other.
func (d Date) DaysUntil(other Date) int {
	return int(other.utcMidnight().Sub(d.utcMidnight()) / (24 * time.Hour))
}
