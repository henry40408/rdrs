package entry

import (
	"errors"
	"time"

	"github.com/henry40408/rdrs/internal/timezone"
)

// SnapshotLayout is the UTC form of a render-time snapshot, the one
// datetime('now') writes to entry.read_at. Single-letter fields accept one or
// two digits, as chrono does.
const SnapshotLayout = "2006-1-2 15:4:5"

// SnapshotFormat is the form a snapshot is written in.
const SnapshotFormat = "2006-01-02 15:04:05"

// ErrBadSnapshot is a snapshot that is not UTC YYYY-MM-DD HH:MM:SS. It is
// refused rather than ignored: dropping it would mark everything, which is
// what the caller asked to avoid.
var ErrBadSnapshot = errors.New("snapshot must be UTC YYYY-MM-DD HH:MM:SS")

// ParseSnapshot reads a snapshot; an absent one is nil.
func ParseSnapshot(raw *string) (*time.Time, error) {
	if raw == nil {
		return nil, nil
	}
	parsed, err := time.ParseInLocation(SnapshotLayout, *raw, time.UTC)
	if err != nil {
		return nil, ErrBadSnapshot
	}
	return &parsed, nil
}

// UnreadFilterOn is the unread entries published on a local day in loc: the
// one definition behind the /entries/today list, its sidebar badge,
// prev/next and mark-all. Follow-up requests take the day of the page's
// snapshot so they keep meaning the list the reader saw when midnight passes
// in between.
func UnreadFilterOn(day timezone.Date, loc *time.Location) Filter {
	start, end := timezone.DayBoundsUTC(day, loc)
	return Filter{UnreadOnly: true, PublishedBetween: &TimeRange{Start: start, End: end}}
}

// TodayUnreadFilter is UnreadFilterOn for the current day in loc.
func TodayUnreadFilter(loc *time.Location) Filter {
	return UnreadFilterOn(timezone.TodayIn(loc), loc)
}

// DayOfSnapshot is the local day of a snapshot taken at instant.
func DayOfSnapshot(instant time.Time, loc *time.Location) timezone.Date {
	return timezone.DateAt(instant, loc)
}
