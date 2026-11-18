package models

import (
	"context"
	"math"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/timezone"
)

type statsFixture struct {
	t        *testing.T
	database *db.DB
	userID   int64
	feedID   int64
}

func newStatsFixture(t *testing.T, database *db.DB) statsFixture {
	t.Helper()
	user := mustUser(t, database, "testuser")
	category := mustCategory(t, database, user.ID, "Tech")
	feed, err := CreateFeed(context.Background(), database, CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/feed", Title: ptr("Test Feed"),
	})
	if err != nil {
		t.Fatal(err)
	}
	return statsFixture{t: t, database: database, userID: user.ID, feedID: feed.ID}
}

// insertEntry stores an entry; a bare date gets a midnight time, since a stored
// value without one sorts before its own midnight.
func (f statsFixture) insertEntry(feedID int64, guid, publishedAt string) int64 {
	f.t.Helper()
	if len(publishedAt) == 10 {
		publishedAt += " 00:00:00"
	}
	var id int64
	err := f.database.QueryRowContext(context.Background(),
		`INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3) RETURNING id`,
		feedID, guid, publishedAt).Scan(&id)
	if err != nil {
		f.t.Fatal(err)
	}
	return id
}

func (f statsFixture) insertEntryCreatedAt(guid, createdAt string) {
	f.t.Helper()
	mustExec(f.t, f.database, `INSERT INTO entry (feed_id, guid, created_at) VALUES ($1, $2, $3)`,
		f.feedID, guid, createdAt)
}

func (f statsFixture) markRead(entryID int64, readAt string) {
	f.t.Helper()
	mustExec(f.t, f.database, `UPDATE entry SET read_at = $1 WHERE id = $2`, readAt, entryID)
}

func (f statsFixture) markStarred(entryID int64, starredAt string) {
	f.t.Helper()
	mustExec(f.t, f.database, `UPDATE entry SET starred_at = $1 WHERE id = $2`, starredAt, entryID)
}

func (f statsFixture) overview(from, to string) PersonalOverview {
	f.t.Helper()
	overview, err := GetPersonalOverview(context.Background(), f.database, f.userID, from, to)
	if err != nil {
		f.t.Fatal(err)
	}
	return overview
}

func (f statsFixture) dailyCounts(from, to string, loc *time.Location) []int64 {
	f.t.Helper()
	days, err := GetDailyReadCounts(context.Background(), f.database,
		DailyReadQuery{UserID: f.userID, From: from, To: to, Loc: loc})
	if err != nil {
		f.t.Fatal(err)
	}
	counts := make([]int64, len(days))
	for i, day := range days {
		counts[i] = day.Count
	}
	return counts
}

func mustZone(t *testing.T, name string) *time.Location {
	t.Helper()
	loc, ok := timezone.Parse(name)
	if !ok {
		t.Fatalf("no zone %q", name)
	}
	return loc
}

func near(a, b float64) bool { return math.Abs(a-b) < 1e-6 }

func TestPersonalOverviewEmpty(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		overview := newStatsFixture(t, database).overview("2024-01-01", "2024-02-01")
		if overview != (PersonalOverview{}) || overview.UnreadEntries() != 0 || overview.ReadRate() != 0 {
			t.Errorf("got %+v", overview)
		}
	})
}

func TestPersonalOverviewWithData(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		e1 := f.insertEntry(f.feedID, "g1", "2024-01-05")
		e2 := f.insertEntry(f.feedID, "g2", "2024-01-10")
		e3 := f.insertEntry(f.feedID, "g3", "2024-01-15")
		f.markRead(e1, "2024-01-06 00:00:00")
		f.markRead(e2, "2024-01-11 00:00:00")
		f.markStarred(e3, "2024-01-16 00:00:00")
		overview := f.overview("2024-01-01", "2024-02-01")
		if overview.TotalEntries != 3 || overview.ReadEntries != 2 || overview.StarredEntries != 1 ||
			overview.UnreadEntries() != 1 {
			t.Errorf("got %+v", overview)
		}
	})
}

func TestPersonalOverviewUsesPublishCohort(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		// Published before the period but read and starred during it: excluded.
		old := f.insertEntry(f.feedID, "old", "2023-12-01")
		f.markRead(old, "2024-01-15 00:00:00")
		f.markStarred(old, "2024-01-16 00:00:00")
		e1 := f.insertEntry(f.feedID, "e1", "2024-01-05")
		f.markRead(e1, "2024-01-06 00:00:00")
		f.markStarred(e1, "2024-01-07 00:00:00")
		f.insertEntry(f.feedID, "e2", "2024-01-10")
		// Read after the period ends is still read.
		e3 := f.insertEntry(f.feedID, "e3", "2024-01-20")
		f.markRead(e3, "2024-03-01 00:00:00")

		overview := f.overview("2024-01-01", "2024-02-01")
		if overview.TotalEntries != 3 || overview.ReadEntries != 2 || overview.StarredEntries != 1 ||
			overview.UnreadEntries() != 1 || !near(overview.ReadRate(), 2.0/3.0*100) {
			t.Errorf("got %+v, rate %v", overview, overview.ReadRate())
		}
	})
}

func TestPersonalOverviewRangeIsHalfOpen(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		f.insertEntry(f.feedID, "in", "2024-01-15")
		f.insertEntry(f.feedID, "before", "2023-12-31")
		f.insertEntry(f.feedID, "from-edge", "2024-01-01")
		f.insertEntry(f.feedID, "on-to", "2024-02-01")
		if got := f.overview("2024-01-01", "2024-02-01").TotalEntries; got != 2 {
			t.Errorf("total = %d", got)
		}
	})
}

func TestPersonalOverviewCountsCompletedSummariesInRange(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		for i, row := range []struct{ status, createdAt string }{
			{"completed", "2024-01-10 00:00:00"},
			{"completed", "2024-02-01 00:00:00"},
			{"failed", "2024-01-10 00:00:00"},
		} {
			other := f.insertEntry(f.feedID, "s"+string(rune('a'+i)), "2024-01-06")
			mustExec(t, database,
				`INSERT INTO entry_summary (entry_id, user_id, status, created_at) VALUES ($1, $2, $3, $4)`,
				other, f.userID, row.status, row.createdAt)
		}
		if got := f.overview("2024-01-01", "2024-02-01").Summaries; got != 1 {
			t.Errorf("summaries = %d", got)
		}
	})
}

func TestStarredCountIsScopedToTheUserAndWindow(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		inside := f.insertEntry(f.feedID, "inside", "2024-01-05")
		f.markStarred(inside, "2024-01-06 00:00:00")
		outside := f.insertEntry(f.feedID, "outside", "2024-03-05")
		f.markStarred(outside, "2024-03-06 00:00:00")
		f.insertEntry(f.feedID, "unstarred", "2024-01-07")

		other := mustUser(t, database, "otheruser")
		otherFeed := mustFeed(t, database, mustCategory(t, database, other.ID, "Theirs").ID, "https://other.example.com/feed")
		theirs := f.insertEntry(otherFeed.ID, "theirs", "2024-01-05")
		f.markStarred(theirs, "2024-01-06 00:00:00")

		if got := f.overview("2024-01-01", "2024-02-01").StarredEntries; got != 1 {
			t.Errorf("starred = %d", got)
		}
		theirOverview, err := GetPersonalOverview(context.Background(), database, other.ID, "2024-01-01", "2024-02-01")
		if err != nil || theirOverview.StarredEntries != 1 {
			t.Errorf("theirs = %+v, %v", theirOverview, err)
		}
	})
}

func TestDailyReadCountsEmptyAndFilled(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		if got := f.dailyCounts("2024-01-01", "2024-01-04", time.UTC); !slices.Equal(got, []int64{0, 0, 0}) {
			t.Errorf("empty = %v", got)
		}
		if got := f.dailyCounts("2024-01-01", "2024-01-04", nil); !slices.Equal(got, []int64{0, 0, 0}) {
			t.Errorf("nil zone = %v", got)
		}
		for i, day := range []string{"2024-01-01", "2024-01-01", "2024-01-03"} {
			f.markRead(f.insertEntry(f.feedID, "g"+string(rune('0'+i)), "2023-12-01"), day+" 09:00:00")
		}
		f.markRead(f.insertEntry(f.feedID, "outside", "2023-12-01"), "2024-02-10 09:00:00")
		days, err := GetDailyReadCounts(context.Background(), database,
			DailyReadQuery{UserID: f.userID, From: "2024-01-01", To: "2024-01-04", Loc: time.UTC})
		if err != nil {
			t.Fatal(err)
		}
		if len(days) != 3 || days[0].Date != (timezone.Date{Year: 2024, Month: 1, Day: 1}) ||
			days[0].Count != 2 || days[1].Count != 0 || days[2].Count != 1 {
			t.Errorf("days = %+v", days)
		}
	})
}

// The same instants fall on different days depending on the reader's zone.
func TestDailyReadCountsFollowTheUserTimezone(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		for i, readAt := range []string{"2024-01-02 20:00:00", "2024-01-02 03:00:00"} {
			f.markRead(f.insertEntry(f.feedID, "g"+string(rune('0'+i)), "2023-12-01"), readAt)
		}
		cases := []struct {
			zone string
			want []int64
		}{
			{"UTC", []int64{0, 2, 0, 0}},
			{"Asia/Taipei", []int64{0, 1, 1, 0}},
			{"America/Los_Angeles", []int64{1, 1, 0, 0}},
		}
		for _, tc := range cases {
			if got := f.dailyCounts("2024-01-01", "2024-01-05", mustZone(t, tc.zone)); !slices.Equal(got, tc.want) {
				t.Errorf("%s: got %v, want %v", tc.zone, got, tc.want)
			}
		}
	})
}

// The Taipei day Jan 1 starts at 16:00 UTC on Dec 31; a read just inside the
// local range must not be lost to the UTC cut-off.
func TestDailyReadCountsIncludeTheLocalDayThatStartsInUTCYesterday(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		f.markRead(f.insertEntry(f.feedID, "g", "2023-12-01"), "2023-12-31 16:30:00")
		if got := f.dailyCounts("2024-01-01", "2024-01-02", mustZone(t, "Asia/Taipei")); !slices.Equal(got, []int64{1}) {
			t.Errorf("got %v", got)
		}
	})
}

func TestDailyReadCountsSkipAnUnreadableInstantAndFallBackOnBadDates(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		if database.Dialect() != db.DialectSQLite {
			t.Skip("PostgreSQL cannot store an unreadable timestamp")
		}
		f := newStatsFixture(t, database)
		f.markRead(f.insertEntry(f.feedID, "ok", "2023-12-01"), "2024-01-02 09:00:00")
		f.markRead(f.insertEntry(f.feedID, "bad", "2023-12-01"), "2024-01-02 not a time")
		if got := f.dailyCounts("2024-01-01", "2024-01-03", time.UTC); !slices.Equal(got, []int64{0, 1}) {
			t.Errorf("got %v", got)
		}
		if got := f.dailyCounts("garbage", "garbage", time.UTC); len(got) != 0 {
			t.Errorf("bad dates gave %v", got)
		}
	})
}

// The overview windows use the same cut-off: an entry published at 20:00 UTC
// belongs to the next day in Taipei.
func TestOverviewWindowFollowsTheUserTimezone(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		f.insertEntry(f.feedID, "late", "2024-01-02 20:00:00")
		window := func(zone string) PersonalOverview {
			loc := mustZone(t, zone)
			return f.overview(UTCBound("2024-01-03", loc), UTCBound("2024-01-04", loc))
		}
		if got := window("UTC").TotalEntries; got != 0 {
			t.Errorf("utc = %d", got)
		}
		if got := window("Asia/Taipei").TotalEntries; got != 1 {
			t.Errorf("taipei = %d", got)
		}
	})
}

func TestUTCBound(t *testing.T) {
	taipei := mustZone(t, "Asia/Taipei")
	if got := UTCBound("2024-01-01", taipei); got != "2023-12-31 16:00:00" {
		t.Errorf("got %q", got)
	}
	if got := UTCBound("not a date", taipei); got != "not a date" {
		t.Errorf("got %q", got)
	}
}

func TestParseBound(t *testing.T) {
	want := time.Date(2024, 1, 2, 3, 4, 5, 0, time.UTC)
	if got := parseBound("2024-01-02 03:04:05"); !got.Equal(want) {
		t.Errorf("got %v", got)
	}
	if got := parseBound("2024-01-02"); !got.Equal(time.Date(2024, 1, 2, 0, 0, 0, 0, time.UTC)) {
		t.Errorf("got %v", got)
	}
	today := time.Now().UTC().Truncate(24 * time.Hour)
	if got := parseBound("junk"); !got.Equal(today) {
		t.Errorf("fallback %v, want %v", got, today)
	}
}

func dailyRun(counts ...int64) []DailyReadCount {
	start := timezone.Date{Year: 2024, Month: 1, Day: 1}
	days := make([]DailyReadCount, len(counts))
	for i, count := range counts {
		days[i] = DailyReadCount{Date: start.AddDays(i), Count: count}
	}
	return days
}

func TestBucketDailyCountsEmpty(t *testing.T) {
	if got := BucketDailyCounts(nil, 14); got != nil {
		t.Errorf("got %v", got)
	}
}

func TestBucketDailyCountsWithinMaxKeepsEachDay(t *testing.T) {
	daily := dailyRun(0, 2, 1, 0)
	buckets := BucketDailyCounts(daily, 14)
	if len(buckets) != 4 {
		t.Fatalf("len = %d", len(buckets))
	}
	for i, bucket := range buckets {
		if bucket.Start != daily[i].Date || bucket.End != daily[i].Date || bucket.Count != daily[i].Count {
			t.Errorf("bucket %d = %+v", i, bucket)
		}
	}
}

func TestBucketDailyCountsOverMaxAggregates(t *testing.T) {
	ones := make([]int64, 15)
	for i := range ones {
		ones[i] = 1
	}
	buckets := BucketDailyCounts(dailyRun(ones...), 14)
	if len(buckets) != 8 {
		t.Fatalf("len = %d", len(buckets))
	}
	first, last := buckets[0], buckets[len(buckets)-1]
	if first.Start != (timezone.Date{Year: 2024, Month: 1, Day: 1}) ||
		first.End != (timezone.Date{Year: 2024, Month: 1, Day: 2}) || first.Count != 2 {
		t.Errorf("first = %+v", first)
	}
	// The last bucket is the single leftover day.
	jan15 := timezone.Date{Year: 2024, Month: 1, Day: 15}
	if last.Start != jan15 || last.End != jan15 || last.Count != 1 {
		t.Errorf("last = %+v", last)
	}
}

func TestBucketDailyCountsSumsWithinBucket(t *testing.T) {
	counts := make([]int64, 28)
	for i := range counts {
		counts[i] = int64(i + 1)
	}
	buckets := BucketDailyCounts(dailyRun(counts...), 14)
	if len(buckets) != 14 || buckets[0].Count != 3 || buckets[13].Count != 55 {
		t.Errorf("got %+v", buckets)
	}
}

func TestBucketDailyCountsTreatsZeroBarsAsOne(t *testing.T) {
	if got := BucketDailyCounts(dailyRun(1, 2, 3), 0); len(got) != 1 || got[0].Count != 6 {
		t.Errorf("got %+v", got)
	}
}

func TestEntriesByCategoryAndTopFeeds(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		science := mustCategory(t, database, f.userID, "Science")
		scienceFeed, err := CreateFeed(context.Background(), database, CreateFeedParams{
			CategoryID: science.ID, URL: "https://science.com/feed", Title: ptr("Science Feed"),
		})
		if err != nil {
			t.Fatal(err)
		}
		f.insertEntry(f.feedID, "t1", "2024-01-05")
		f.insertEntry(f.feedID, "t2", "2024-01-10")
		f.insertEntry(scienceFeed.ID, "s1", "2024-01-07")
		f.insertEntry(scienceFeed.ID, "s-out", "2024-03-07")

		categories, err := GetEntriesByCategory(context.Background(), database, f.userID, "2024-01-01", "2024-02-01")
		if err != nil || len(categories) != 2 ||
			categories[0] != (CategoryCount{"Tech", 2}) || categories[1] != (CategoryCount{"Science", 1}) {
			t.Errorf("categories = %+v, %v", categories, err)
		}
		feeds, err := GetTopFeeds(context.Background(), database,
			TopFeedsQuery{UserID: f.userID, From: "2024-01-01", To: "2024-02-01", Limit: 1})
		if err != nil || len(feeds) != 1 || feeds[0] != (FeedCount{"Test Feed", 2}) {
			t.Errorf("feeds = %+v, %v", feeds, err)
		}
		none, err := GetTopFeeds(context.Background(), database,
			TopFeedsQuery{UserID: f.userID, From: "2025-01-01", To: "2025-02-01", Limit: 10})
		if err != nil || len(none) != 0 {
			t.Errorf("none = %+v, %v", none, err)
		}
	})
}

func TestAdminCountsAndEntryStats(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		counts, err := GetAdminCounts(context.Background(), database)
		if err != nil || counts != (AdminCounts{TotalUsers: 1, TotalFeeds: 1}) {
			t.Errorf("counts = %+v, %v", counts, err)
		}
		e1 := f.insertEntry(f.feedID, "g1", "2024-01-05")
		f.insertEntry(f.feedID, "g2", "2024-01-10")
		f.markRead(e1, "2024-01-06 00:00:00")
		old := f.insertEntry(f.feedID, "g-old", "2023-12-01")
		f.markRead(old, "2024-01-07 00:00:00")

		stats, err := GetAdminEntryStats(context.Background(), database, "2024-01-01", "2024-02-01")
		if err != nil || stats.TotalEntries != 2 || stats.ReadEntries != 1 || !near(stats.ReadRate(), 50) {
			t.Errorf("stats = %+v, %v", stats, err)
		}
	})
}

func databaseStats(t *testing.T, database *db.DB) AdminDatabaseStats {
	t.Helper()
	stats, err := GetAdminDatabaseStats(context.Background(), database)
	if err != nil {
		t.Fatal(err)
	}
	return stats
}

func TestAdminDatabaseStatsEmpty(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		newStatsFixture(t, database)
		stats := databaseStats(t, database)
		if stats.DBSizeBytes <= 0 || stats.TotalEntries != 0 || stats.CoverageDays != 0 ||
			stats.AvgNewEntriesPerDay != 0 || stats.TombstoneCount != 0 {
			t.Errorf("stats = %+v", stats)
		}
		if database.Dialect() == db.DialectPostgres {
			if stats.Reclaimable != nil {
				t.Errorf("postgres reclaimable = %+v", stats.Reclaimable)
			}
			return
		}
		if stats.Reclaimable == nil || stats.Reclaimable.Bytes < 0 ||
			stats.Reclaimable.FragmentationRatio < 0 || stats.Reclaimable.FragmentationRatio > 1 {
			t.Errorf("reclaimable = %+v", stats.Reclaimable)
		}
	})
}

func TestAdminDatabaseStatsWithData(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		for i, day := range []string{"01", "02", "03", "04"} {
			f.insertEntryCreatedAt("e"+string(rune('a'+i)), "2024-01-"+day+" 00:00:00")
		}
		mustExec(t, database, `INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, 'dead-1')`, f.feedID)
		mustExec(t, database, `INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, 'dead-2')`, f.feedID)
		stats := databaseStats(t, database)
		if stats.TotalEntries != 4 || stats.TombstoneCount != 2 || !near(stats.CoverageDays, 3) ||
			!near(stats.AvgNewEntriesPerDay, 4.0/3.0) {
			t.Errorf("stats = %+v", stats)
		}
	})
}

func TestAdminDatabaseStatsAverageGuardsASubDaySpan(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		f.insertEntryCreatedAt("only", "2024-01-01 12:00:00")
		stats := databaseStats(t, database)
		if stats.TotalEntries != 1 || stats.CoverageDays != 0 || !near(stats.AvgNewEntriesPerDay, 1) {
			t.Errorf("stats = %+v", stats)
		}
	})
}

func TestAdminDatabaseStatsParseRFC3339CreatedAt(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newStatsFixture(t, database)
		f.insertEntryCreatedAt("a", "2024-01-01T00:00:00Z")
		f.insertEntryCreatedAt("b", "2024-01-03T00:00:00Z")
		stats := databaseStats(t, database)
		if stats.TotalEntries != 2 || !near(stats.CoverageDays, 2) || !near(stats.AvgNewEntriesPerDay, 1) {
			t.Errorf("stats = %+v", stats)
		}
	})
}

func TestStatisticsQueriesAreIndexCovered(t *testing.T) {
	database := openAndMigrate(t, t.TempDir()+"/plan.sqlite3", config.SQLite)
	cases := []struct {
		name, query string
		want, deny  string
	}{
		{"read count", readEntriesSQL, "idx_entry_feed_read_sort", "idx_entry_feed_sort"},
		{"daily reads", readInstantsSQL(db.DialectSQLite), "COVERING INDEX idx_entry_feed_read_at", ""},
		{"starred count", starredEntriesSQL, "idx_entry_feed_starred_sort", "idx_entry_starred_sort ("},
	}
	for _, tc := range cases {
		plan := explainRangeQuery(t, database, tc.query)
		if !strings.Contains(plan, tc.want) {
			t.Errorf("%s: plan lacks %s:\n%s", tc.name, tc.want, plan)
		}
		if tc.deny != "" && strings.Contains(plan, tc.deny) {
			t.Errorf("%s: plan uses %s:\n%s", tc.name, tc.deny, plan)
		}
	}
}

// explainRangeQuery is the query plan of a statistics query, bound as
// (user, from, to).
func explainRangeQuery(t *testing.T, database *db.DB, query string) string {
	t.Helper()
	rows, err := database.QueryContext(context.Background(), "EXPLAIN QUERY PLAN "+query,
		int64(1), "2024-01-01 00:00:00", "2024-02-01 00:00:00")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rows.Close() }()
	var details []string
	for rows.Next() {
		var id, parent, unused int64
		var detail string
		if err := rows.Scan(&id, &parent, &unused, &detail); err != nil {
			t.Fatal(err)
		}
		details = append(details, detail)
	}
	return strings.Join(details, "\n")
}
