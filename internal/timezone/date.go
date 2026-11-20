package timezone

import "time"

// DateLayout is the Y-M-D form dates take in forms and queries.
const DateLayout = "2006-01-02"

// Date is a calendar date with no time or zone.
type Date struct {
	Year  int
	Month time.Month
	Day   int
}

// ParseDate reads a "YYYY-MM-DD" date.
func ParseDate(s string) (Date, bool) {
	t, err := time.Parse(DateLayout, s)
	if err != nil {
		return Date{}, false
	}
	return dateOf(t), true
}

func dateOf(t time.Time) Date {
	year, month, day := t.Date()
	return Date{Year: year, Month: month, Day: day}
}

func (d Date) utcMidnight() time.Time {
	return time.Date(d.Year, d.Month, d.Day, 0, 0, 0, 0, time.UTC)
}

// AddDays moves the date by n calendar days, which may be negative.
func (d Date) AddDays(n int) Date {
	return dateOf(d.utcMidnight().AddDate(0, 0, n))
}

// Before reports whether d is earlier than other.
func (d Date) Before(other Date) bool {
	return d.utcMidnight().Before(other.utcMidnight())
}

// Weekday is the day of the week.
func (d Date) Weekday() time.Weekday {
	return d.utcMidnight().Weekday()
}

// String formats the date as "YYYY-MM-DD".
func (d Date) String() string {
	return d.utcMidnight().Format(DateLayout)
}
