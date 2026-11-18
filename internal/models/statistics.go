package models

import (
	"context"
	"database/sql"
	"time"

	"github.com/henry40408/rdrs/internal/datetime"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/timezone"
)

const boundLayout = "2006-01-02 15:04:05"

// parseBound reads a range bound: "YYYY-MM-DD" (midnight UTC) or the
// "YYYY-MM-DD HH:MM:SS" that UTCBound returns. It falls back to today so a bad
// form value still renders a page.
func parseBound(s string) time.Time {
	if t, err := time.Parse(boundLayout, s); err == nil {
		return t
	}
	if t, err := time.Parse(timezone.DateLayout, s); err == nil {
		return t
	}
	return time.Now().UTC().Truncate(24 * time.Hour)
}

// bound is the query argument for the range bound s.
func bound(e db.Executor, s string) string {
	return e.Dialect().TimeBound(parseBound(s))
}

// UTCBound is the UTC instant at which the local date (YYYY-MM-DD) begins in
// loc, as a range bound for the queries below. Unparseable input is passed
// through, so parseBound applies its own fallback.
func UTCBound(date string, loc *time.Location) string {
	parsed, ok := timezone.ParseDate(date)
	if !ok {
		return date
	}
	return timezone.StartOfDayUTC(parsed, loc).Format(boundLayout)
}

// PersonalOverview holds a user's metrics for a date range.
type PersonalOverview struct {
	TotalEntries   int64
	ReadEntries    int64
	StarredEntries int64
	Summaries      int64
}

// UnreadEntries counts period-published entries not yet read. It is never
// negative, since read is a subset of the same cohort; no clamp, so a
// regression surfaces.
func (o PersonalOverview) UnreadEntries() int64 {
	return o.TotalEntries - o.ReadEntries
}

// ReadRate is the percentage (0-100) of period-published entries read.
func (o PersonalOverview) ReadRate() float64 {
	return readRate(o.ReadEntries, o.TotalEntries)
}

func readRate(read, total int64) float64 {
	if total == 0 {
		return 0
	}
	return float64(read) / float64(total) * 100
}

// DailyReadCount is a single day's read count.
type DailyReadCount struct {
	Date  timezone.Date
	Count int64
}

// DailyBucket is a span of days [Start, End] collapsed into one bar.
type DailyBucket struct {
	Start, End timezone.Date
	Count      int64
}

// BucketDailyCounts collapses chronologically ordered per-day counts into at
// most maxBars buckets of ceil(len / maxBars) days each.
func BucketDailyCounts(daily []DailyReadCount, maxBars int) []DailyBucket {
	if len(daily) == 0 {
		return nil
	}
	size := (len(daily) + max(maxBars, 1) - 1) / max(maxBars, 1)
	var buckets []DailyBucket
	for start := 0; start < len(daily); start += size {
		chunk := daily[start:min(start+size, len(daily))]
		bucket := DailyBucket{Start: chunk[0].Date, End: chunk[len(chunk)-1].Date}
		for _, day := range chunk {
			bucket.Count += day.Count
		}
		buckets = append(buckets, bucket)
	}
	return buckets
}

// CategoryCount is a category with its entry count.
type CategoryCount struct {
	Name  string
	Count int64
}

// FeedCount is a feed with its entry count.
type FeedCount struct {
	Title string
	Count int64
}

// AdminCounts are site-wide counts that do not depend on the period.
type AdminCounts struct {
	TotalUsers int64
	TotalFeeds int64
}

// AdminEntryStats are site-wide entry stats for a period.
type AdminEntryStats struct {
	TotalEntries int64
	ReadEntries  int64
}

// ReadRate is the percentage (0-100) of period-published entries read.
func (s AdminEntryStats) ReadRate() float64 {
	return readRate(s.ReadEntries, s.TotalEntries)
}

// ReclaimableSpace is the free space a VACUUM would reclaim; SQLite only.
type ReclaimableSpace struct {
	Bytes int64
	// FragmentationRatio is Bytes over the database size, in 0..1.
	FragmentationRatio float64
}

// AdminDatabaseStats are the storage and record stats of the database. The
// page omits the card when Reclaimable is nil rather than showing a misleading
// zero.
type AdminDatabaseStats struct {
	DBSizeBytes         int64
	Reclaimable         *ReclaimableSpace
	TotalEntries        int64
	AvgNewEntriesPerDay float64
	CoverageDays        float64
	TombstoneCount      int64
}

// The count halves of GetPersonalOverview, hoisted for the query-plan test.
//
// The read count needs idx_entry_feed_read_sort (migration 0011). The starred
// count uses feed_id IN (SELECT ...) on purpose: a join makes SQLite pick
// idx_entry_starred_sort and pay a table lookup per row, instead of
// idx_entry_feed_starred_sort (0013).
const (
	totalEntriesSQL = `
		SELECT COUNT(e.id)
		FROM entry e
		INNER JOIN feed f ON e.feed_id = f.id
		INNER JOIN category c ON f.category_id = c.id
		WHERE c.user_id = $1
		  AND COALESCE(e.published_at, e.created_at) >= $2
		  AND COALESCE(e.published_at, e.created_at) < $3`

	readEntriesSQL = totalEntriesSQL + `
		  AND e.read_at IS NOT NULL`

	starredEntriesSQL = `
		SELECT COUNT(e.id)
		FROM entry e
		WHERE e.feed_id IN (
				SELECT f.id
				FROM feed f
				INNER JOIN category c ON f.category_id = c.id
				WHERE c.user_id = $1
			  )
		  AND COALESCE(e.published_at, e.created_at) >= $2
		  AND COALESCE(e.published_at, e.created_at) < $3
		  AND e.starred_at IS NOT NULL`

	completedSummariesSQL = `
		SELECT COUNT(es.id)
		FROM entry_summary es
		WHERE es.user_id = $1
		  AND es.status = 'completed'
		  AND es.created_at >= $2
		  AND es.created_at < $3`
)

// GetPersonalOverview counts a user's entries in [from, to), both YYYY-MM-DD.
// Read and starred are subsets of the same publish cohort as the total, not
// period activity, so read never exceeds total.
func GetPersonalOverview(ctx context.Context, e db.Executor, userID int64, from, to string) (PersonalOverview, error) {
	args := []any{userID, bound(e, from), bound(e, to)}
	var overview PersonalOverview
	counts := []struct {
		query string
		into  *int64
	}{
		{totalEntriesSQL, &overview.TotalEntries},
		{readEntriesSQL, &overview.ReadEntries},
		{starredEntriesSQL, &overview.StarredEntries},
		{completedSummariesSQL, &overview.Summaries},
	}
	for _, count := range counts {
		n, err := countScalar(ctx, e, count.query, args...)
		if err != nil {
			return PersonalOverview{}, err
		}
		*count.into = n
	}
	return overview, nil
}

// readInstantsSQL returns each read instant rather than a per-day count,
// because which day an instant belongs to depends on the user's timezone,
// which SQLite cannot apply. It needs idx_entry_feed_read_at to stay covering
// (see 0012). The instants are selected as text so one unreadable row is
// skipped instead of failing the whole chart.
func readInstantsSQL(d db.Dialect) string {
	return `SELECT ` + d.CursorTS("e.read_at") + `
		FROM entry e
		INNER JOIN feed f ON e.feed_id = f.id
		INNER JOIN category c ON f.category_id = c.id
		WHERE c.user_id = $1
		  AND e.read_at >= $2
		  AND e.read_at < $3`
}

// DailyReadQuery is the range of a GetDailyReadCounts lookup. From and To are
// local dates (YYYY-MM-DD) in Loc; the range is [From, To).
type DailyReadQuery struct {
	UserID   int64
	From, To string
	Loc      *time.Location
}

// GetDailyReadCounts counts reads per local day; days with no reads are
// zero-filled.
func GetDailyReadCounts(ctx context.Context, e db.Executor, query DailyReadQuery) ([]DailyReadCount, error) {
	if query.Loc == nil {
		query.Loc = time.UTC
	}
	rows, err := e.QueryContext(ctx, readInstantsSQL(e.Dialect()), query.UserID,
		bound(e, UTCBound(query.From, query.Loc)), bound(e, UTCBound(query.To, query.Loc)))
	instants, err := collect(rows, err, func(row scanner) (string, error) {
		var text string
		return text, row.Scan(&text)
	})
	if err != nil {
		return nil, err
	}
	perDay := make(map[timezone.Date]int64)
	for _, text := range instants {
		if readAt, ok := datetime.TryParse(text); ok {
			perDay[timezone.DateAt(readAt, query.Loc)]++
		}
	}
	return zeroFilledDays(perDay, query), nil
}

func zeroFilledDays(perDay map[timezone.Date]int64, query DailyReadQuery) []DailyReadCount {
	today := timezone.TodayIn(query.Loc)
	from, to := dateOr(query.From, today), dateOr(query.To, today)
	var days []DailyReadCount
	for day := from; day.Before(to); day = day.AddDays(1) {
		days = append(days, DailyReadCount{Date: day, Count: perDay[day]})
	}
	return days
}

func dateOr(s string, fallback timezone.Date) timezone.Date {
	if date, ok := timezone.ParseDate(s); ok {
		return date
	}
	return fallback
}

// GetEntriesByCategory counts entries per category in [from, to); only
// categories with at least one entry are returned, by count descending.
func GetEntriesByCategory(ctx context.Context, e db.Executor, userID int64, from, to string) ([]CategoryCount, error) {
	rows, err := e.QueryContext(ctx, `
		SELECT c.name, COUNT(e.id) AS count
		FROM category c
		LEFT JOIN feed f ON f.category_id = c.id
		LEFT JOIN entry e ON e.feed_id = f.id
			AND COALESCE(e.published_at, e.created_at) >= $2
			AND COALESCE(e.published_at, e.created_at) < $3
		WHERE c.user_id = $1
		GROUP BY c.id
		HAVING COUNT(e.id) > 0
		ORDER BY COUNT(e.id) DESC`, userID, bound(e, from), bound(e, to))
	return collect(rows, err, func(row scanner) (CategoryCount, error) {
		var c CategoryCount
		return c, row.Scan(&c.Name, &c.Count)
	})
}

// TopFeedsQuery is the range and size of a GetTopFeeds lookup.
type TopFeedsQuery struct {
	UserID   int64
	From, To string
	Limit    int64
}

// GetTopFeeds returns the top Limit feeds with at least one entry in
// [From, To), by count descending.
func GetTopFeeds(ctx context.Context, e db.Executor, query TopFeedsQuery) ([]FeedCount, error) {
	rows, err := e.QueryContext(ctx, `
		SELECT f.title, COUNT(e.id) AS count
		FROM feed f
		INNER JOIN category c ON f.category_id = c.id
		LEFT JOIN entry e ON e.feed_id = f.id
			AND COALESCE(e.published_at, e.created_at) >= $2
			AND COALESCE(e.published_at, e.created_at) < $3
		WHERE c.user_id = $1
		GROUP BY f.id
		HAVING COUNT(e.id) > 0
		ORDER BY COUNT(e.id) DESC
		LIMIT $4`, query.UserID, bound(e, query.From), bound(e, query.To), query.Limit)
	return collect(rows, err, func(row scanner) (FeedCount, error) {
		var f FeedCount
		return f, row.Scan(&f.Title, &f.Count)
	})
}

// GetAdminCounts is period-independent, unlike the other admin stats.
func GetAdminCounts(ctx context.Context, e db.Executor) (AdminCounts, error) {
	users, err := countScalar(ctx, e, `SELECT COUNT(*) FROM "user"`)
	if err != nil {
		return AdminCounts{}, err
	}
	feeds, err := countScalar(ctx, e, `SELECT COUNT(*) FROM feed`)
	if err != nil {
		return AdminCounts{}, err
	}
	return AdminCounts{TotalUsers: users, TotalFeeds: feeds}, nil
}

// GetAdminEntryStats counts entries site-wide published in [from, to). Read
// entries are the same publish cohort as the total, so the rate stays within
// 0-100%.
func GetAdminEntryStats(ctx context.Context, e db.Executor, from, to string) (AdminEntryStats, error) {
	const cohort = `SELECT COUNT(id) FROM entry
		WHERE COALESCE(published_at, created_at) >= $1
		  AND COALESCE(published_at, created_at) < $2`
	args := []any{bound(e, from), bound(e, to)}
	total, err := countScalar(ctx, e, cohort, args...)
	if err != nil {
		return AdminEntryStats{}, err
	}
	read, err := countScalar(ctx, e, cohort+` AND read_at IS NOT NULL`, args...)
	if err != nil {
		return AdminEntryStats{}, err
	}
	return AdminEntryStats{TotalEntries: total, ReadEntries: read}, nil
}

// GetAdminDatabaseStats is period-independent; the storage figures are
// dialect-specific.
func GetAdminDatabaseStats(ctx context.Context, e db.Executor) (AdminDatabaseStats, error) {
	size, reclaimable, err := storageSizes(ctx, e)
	if err != nil {
		return AdminDatabaseStats{}, err
	}
	total, err := countScalar(ctx, e, `SELECT COUNT(*) FROM entry`)
	if err != nil {
		return AdminDatabaseStats{}, err
	}
	tombstones, err := countScalar(ctx, e, `SELECT COUNT(*) FROM entry_tombstone`)
	if err != nil {
		return AdminDatabaseStats{}, err
	}
	coverage, spanKnown, err := createdCoverageDays(ctx, e)
	if err != nil {
		return AdminDatabaseStats{}, err
	}
	stats := AdminDatabaseStats{
		DBSizeBytes: size, Reclaimable: reclaimable, TotalEntries: total,
		CoverageDays: coverage, TombstoneCount: tombstones,
	}
	if spanKnown {
		// Average over the retained span, not the age of the oldest entry,
		// since retention prunes read entries. A span under a day counts as one.
		stats.AvgNewEntriesPerDay = float64(total) / max(coverage, 1)
	}
	return stats, nil
}

// storageSizes returns the database size and, on SQLite, the free space a
// VACUUM would reclaim. PostgreSQL has no reliable free-space figure without
// pgstattuple (n_dead_tup excludes TOAST, counts tuples not bytes, and resets
// on autovacuum), so it reports nil and the page drops the card.
func storageSizes(ctx context.Context, e db.Executor) (int64, *ReclaimableSpace, error) {
	if e.Dialect() == db.DialectPostgres {
		size, err := countScalar(ctx, e, `SELECT pg_database_size(current_database())`)
		return size, nil, err
	}
	var pragmas [3]int64
	for i, name := range []string{"page_count", "page_size", "freelist_count"} {
		n, err := countScalar(ctx, e, "PRAGMA "+name)
		if err != nil {
			return 0, nil, err
		}
		pragmas[i] = n
	}
	pageCount, pageSize, freelist := pragmas[0], pragmas[1], pragmas[2]
	size, free := pageCount*pageSize, freelist*pageSize
	reclaimable := &ReclaimableSpace{Bytes: free}
	if size > 0 {
		reclaimable.FragmentationRatio = float64(free) / float64(size)
	}
	return size, reclaimable, nil
}

// createdCoverageDays is the span in days between the oldest and newest entry,
// or unknown when either is missing or unreadable. It uses the fallible parser, since
// ParseOrNow's fallback would corrupt aggregates. The bare MIN and MAX let
// SQLite use the idx_entry_created_at endpoint optimisation.
func createdCoverageDays(ctx context.Context, e db.Executor) (days float64, known bool, err error) {
	oldest, err := createdEndpoint(ctx, e, "MIN")
	if err != nil || oldest == nil {
		return 0, false, err
	}
	newest, err := createdEndpoint(ctx, e, "MAX")
	if err != nil || newest == nil {
		return 0, false, err
	}
	return newest.Sub(*oldest).Truncate(time.Second).Seconds() / 86_400, true, nil
}

func createdEndpoint(ctx context.Context, e db.Executor, aggregate string) (*time.Time, error) {
	var text sql.NullString
	query := `SELECT ` + e.Dialect().CursorTS(aggregate+"(created_at)") + ` FROM entry`
	if err := e.QueryRowContext(ctx, query).Scan(&text); err != nil {
		return nil, err
	}
	if !text.Valid {
		return nil, nil
	}
	parsed, ok := datetime.TryParse(text.String)
	if !ok {
		return nil, nil
	}
	return &parsed, nil
}
