package entry_test

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// explain is the concatenated EXPLAIN QUERY PLAN details of a statement that
// runs with the given binds.
func explain(t *testing.T, database *db.DB, sqlText string, args []any) string {
	t.Helper()
	rows, err := database.QueryContext(context.Background(), "EXPLAIN QUERY PLAN "+sqlText, args...)
	must(t, err)
	defer func() { _ = rows.Close() }()
	var details []string
	for rows.Next() {
		var id, parent, unused int64
		var detail string
		must(t, rows.Scan(&id, &parent, &unused, &detail))
		details = append(details, detail)
	}
	must(t, rows.Err())
	return strings.Join(details, " | ")
}

func sqliteFixture(t *testing.T) fixture {
	t.Helper()
	return newFixture(t, open(t, filepath.Join(t.TempDir(), "plan.sqlite3"), config.SQLite))
}

func TestListPlansUseTheHintedIndex(t *testing.T) {
	f := sqliteFixture(t)
	f.add(t, "a", "A", day(1))
	tests := []struct {
		name   string
		filter entry.Filter
		index  string
	}{
		{"starred", entry.Filter{StarredOnly: true}, "idx_entry_starred_sort"},
		{"read", entry.Filter{ReadOnly: true}, "idx_entry_read_sort"},
		{"unread", entry.Filter{UnreadOnly: true}, "idx_entry_unread_sort"},
		{"no predicate", entry.Filter{}, "idx_entry_sort_ts"},
	}
	for _, tt := range tests {
		params := &entry.ContinuationParams{Limit: 51, SortOrder: entry.SortPublishedAt}
		sqlText, args := entry.ListSQL(db.DialectSQLite, f.userID, &tt.filter, params)
		plan := explain(t, f.database, sqlText, args)
		if !strings.Contains(plan, tt.index) {
			t.Errorf("%s: plan misses %s: %s", tt.name, tt.index, plan)
		}
		if tt.name == "no predicate" && strings.Contains(plan, "USE TEMP B-TREE FOR ORDER BY") {
			t.Errorf("%s: plan sorts in a temp b-tree: %s", tt.name, plan)
		}
	}
}

// The snapshot-widened unread neighbours query must not fall back to an
// O(table) MULTI-INDEX OR.
func TestNeighborSnapshotPlanPinsSortIndex(t *testing.T) {
	f := sqliteFixture(t)
	id := f.add(t, "a", "A", day(1))
	snapshot := "2025-01-01 00:00:00"
	filter := &entry.Filter{UnreadOnly: true, ReadAfter: &snapshot}
	for _, next := range []bool{false, true} {
		sqlText, args := entry.NeighborSQL(db.DialectSQLite, f.userID, id, "2025-01-01 12:00:00", next, filter)
		plan := explain(t, f.database, sqlText, args)
		if !strings.Contains(plan, "idx_entry_sort_ts") {
			t.Errorf("next=%v: plan must pin idx_entry_sort_ts: %s", next, plan)
		}
		if strings.Contains(plan, "MULTI-INDEX OR") || strings.Contains(plan, "idx_entry_read_at") {
			t.Errorf("next=%v: plan fans out or scans read_at: %s", next, plan)
		}
	}
}

// seedMostlyRead stores 3000 entries over 20 feeds, 85% of them read, then
// runs ANALYZE: the planner picks between the unread indexes on statistics, so
// an empty schema says nothing about which one a real list gets.
func seedMostlyRead(t *testing.T) fixture {
	t.Helper()
	f := sqliteFixture(t)
	ctx := context.Background()
	for i := 1; i < 20; i++ {
		_, err := models.CreateFeed(ctx, f.database, models.CreateFeedParams{CategoryID: 1, URL: fmt.Sprintf("https://example.com/%d.xml", i)})
		must(t, err)
	}
	_, err := f.database.ExecContext(ctx,
		`WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3000)
		 INSERT INTO entry(feed_id, guid, published_at, read_at, created_at)
		 SELECT (SELECT MIN(id) FROM feed) + (i % 20), 'g' || i,
		        strftime('%Y-%m-%dT%H:%M:%SZ', 1700000000 + i * 300, 'unixepoch'),
		        CASE WHEN (i * 7919) % 100 < 85 THEN '2024-01-01 00:00:00' END,
		        strftime('%Y-%m-%d %H:%M:%S', 1700000000 + i * 300, 'unixepoch')
		 FROM n`)
	must(t, err)
	_, err = f.database.ExecContext(ctx, `ANALYZE`)
	must(t, err)
	return f
}

const rangeCmp = " AND COALESCE(e.published_at, e.created_at) > " +
	"(SELECT COALESCE(s.published_at, s.created_at) FROM entry s WHERE s.id = $2)"

func positionPlan(t *testing.T, f fixture, filter *entry.Filter, slice int) string {
	t.Helper()
	sqlText, args := entry.PositionSliceSQL(f.userID, 1, filter, slice, rangeCmp, " INDEXED BY idx_entry_unread_feed_sort")
	return explain(t, f.database, sqlText, args)
}

// Every pinned index must be usable for its query: SQLite rejects an INDEXED
// BY it cannot use at prepare time, which no plan check would show.
func TestFindPositionRunsWithPinnedUnreadIndexes(t *testing.T) {
	f := seedMostlyRead(t)
	var entryID int64
	must(t, f.database.QueryRowContext(context.Background(),
		`SELECT MIN(id) FROM entry WHERE read_at IS NULL`).Scan(&entryID))
	// After the seeded read_at, so no read entry counts as read since.
	snapshot := "2024-06-01 00:00:00"
	for _, filter := range []*entry.Filter{{UnreadOnly: true}, {UnreadOnly: true, ReadAfter: &snapshot}} {
		got, err := entry.FindPosition(context.Background(), f.database, f.userID, entryID, filter)
		must(t, err)
		if got.Total != 450 || got.Position != 450 {
			t.Errorf("%+v: position = %d of %d, want 450 of 450", filter, got.Position, got.Total)
		}
	}
}

func TestPositionUnreadCountIsIndexCovered(t *testing.T) {
	f := seedMostlyRead(t)
	plan := positionPlan(t, f, &entry.Filter{UnreadOnly: true}, 0)
	if !strings.Contains(plan, "idx_entry_unread_feed_sort") {
		t.Errorf("plan misses idx_entry_unread_feed_sort: %s", plan)
	}
}

// The snapshot predicate as one OR is O(table), so each half is counted on its
// own index.
func TestPositionSnapshotCountIsSplitIntoIndexedHalves(t *testing.T) {
	f := seedMostlyRead(t)
	snapshot := "2024-01-01 00:00:00"
	filter := &entry.Filter{UnreadOnly: true, ReadAfter: &snapshot}
	if plan := positionPlan(t, f, filter, 0); !strings.Contains(plan, "idx_entry_unread_feed_sort") {
		t.Errorf("unread half misses idx_entry_unread_feed_sort: %s", plan)
	}
	if plan := positionPlan(t, f, filter, 1); !strings.Contains(plan, "idx_entry_read_at") {
		t.Errorf("read-since half misses idx_entry_read_at: %s", plan)
	}
}
