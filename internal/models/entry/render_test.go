package entry

import (
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/search"
)

// render parses a /search query and renders it the way a filter does. The
// expected strings are the ones asserted in the Rust filters.rs tests.
func render(t *testing.T, input string, dialect db.Dialect) (string, []any) {
	t.Helper()
	node, err := search.Parse(input)
	if err != nil {
		t.Fatalf("parse %q: %v", input, err)
	}
	q := &query{dialect: dialect}
	return q.renderQuery(node), q.binds
}

func TestRenderQueryGolden(t *testing.T) {
	tests := []struct {
		name      string
		input     string
		dialect   db.Dialect
		want      string
		wantBinds []any
	}{
		{"status unread", "is:unread", db.DialectSQLite, "e.read_at IS NULL", nil},
		{
			"free text sqlite", "rust", db.DialectSQLite,
			`COALESCE((e.title LIKE $1 ESCAPE '\' OR e.content_text LIKE $1 ESCAPE '\'), 0)`,
			[]any{"%rust%"},
		},
		{
			"free text postgres", "rust", db.DialectPostgres,
			`COALESCE((e.title ILIKE $1 ESCAPE '\' OR e.content_text ILIKE $1 ESCAPE '\'), FALSE)`,
			[]any{"%rust%"},
		},
		{"wildcards escaped", "50%", db.DialectSQLite,
			`COALESCE((e.title LIKE $1 ESCAPE '\' OR e.content_text LIKE $1 ESCAPE '\'), 0)`,
			[]any{`%50\%%`}},
		{"feed column", "feed:news", db.DialectSQLite, `COALESCE(f.title LIKE $1 ESCAPE '\', 0)`, []any{"%news%"}},
		{"author column", "author:jane", db.DialectSQLite, `COALESCE(e.author LIKE $1 ESCAPE '\', 0)`, []any{"%jane%"}},
		{
			"nesting and bind numbering", "(rust OR go) AND is:unread", db.DialectSQLite,
			`((COALESCE((e.title LIKE $1 ESCAPE '\' OR e.content_text LIKE $1 ESCAPE '\'), 0) ` +
				`OR COALESCE((e.title LIKE $2 ESCAPE '\' OR e.content_text LIKE $2 ESCAPE '\'), 0)) AND e.read_at IS NULL)`,
			[]any{"%rust%", "%go%"},
		},
		{"not wraps", "-is:read", db.DialectSQLite, "(NOT e.read_at IS NOT NULL)", nil},
		{
			"after date sqlite", "after:2026-01-01", db.DialectSQLite,
			"CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) >= $1",
			[]any{int64(1767225600)},
		},
		{
			"before date postgres", "before:2026-01-01", db.DialectPostgres,
			"EXTRACT(EPOCH FROM COALESCE(e.published_at, e.created_at))::bigint < $1",
			[]any{int64(1767225600)},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, binds := render(t, tt.input, tt.dialect)
			if got != tt.want {
				t.Errorf("fragment\n got: %s\nwant: %s", got, tt.want)
			}
			if len(binds) != len(tt.wantBinds) {
				t.Fatalf("binds = %v, want %v", binds, tt.wantBinds)
			}
			for i := range binds {
				if binds[i] != tt.wantBinds[i] {
					t.Errorf("bind %d = %#v, want %#v", i+1, binds[i], tt.wantBinds[i])
				}
			}
		})
	}
}

func TestPublishedSortHint(t *testing.T) {
	snapshot := "2026-01-01 00:00:00"
	tests := []struct {
		name   string
		filter Filter
		want   string
	}{
		{"unread", Filter{UnreadOnly: true}, " INDEXED BY idx_entry_unread_sort"},
		{"unread snapshot has no unread hint", Filter{UnreadOnly: true, ReadAfter: &snapshot}, ""},
		{"starred", Filter{StarredOnly: true}, " INDEXED BY idx_entry_starred_sort"},
		{"read", Filter{ReadOnly: true}, " INDEXED BY idx_entry_read_sort"},
		{"no entry-side predicate", Filter{}, " INDEXED BY idx_entry_sort_ts"},
		{"feed scoped", Filter{FeedID: new(int64)}, ""},
	}
	for _, tt := range tests {
		if got := publishedSortHint(&tt.filter); got != tt.want {
			t.Errorf("%s: hint = %q, want %q", tt.name, got, tt.want)
		}
	}
}

func TestIsNoEntrySidePredicate(t *testing.T) {
	if !(&Filter{}).isNoEntrySidePredicate() {
		t.Error("zero filter must have no entry-side predicate")
	}
	yes := true
	text := "foo"
	id := int64(1)
	for _, f := range []Filter{
		{FeedID: &id}, {CategoryID: &id}, {UnreadOnly: true}, {StarredOnly: true}, {ReadOnly: true},
		{Search: &text}, {HasSummary: &yes}, {HasSummary: new(bool)},
	} {
		if f.isNoEntrySidePredicate() {
			t.Errorf("expected entry-side predicate for %+v", f)
		}
	}
}

func continuationCondition(cursor CompositeCursor, dialect db.Dialect) (string, []any) {
	q := newUserQuery(dialect, 1)
	q.applyContinuation(&ContinuationParams{Continuation: cursor})
	return q.conditions[len(q.conditions)-1], q.binds[1:]
}

// A valid PostgreSQL cursor must never use to_char, or the index is lost.
func TestPostgresCursorIsSargable(t *testing.T) {
	cond, binds := continuationCondition(CompositeCursor{SortTS: "2026-07-07 12:00:00", ID: 42}, db.DialectPostgres)
	if strings.Contains(cond, "to_char") || !strings.Contains(cond, "COALESCE(e.published_at, e.created_at)") {
		t.Errorf("cursor condition = %s", cond)
	}
	if binds[0] != "2026-07-07 12:00:00" || binds[1] != "2026-07-07 12:00:00" || binds[2] != int64(42) {
		t.Errorf("binds = %#v", binds)
	}
}

func TestPostgresCursorFallsBackToToCharWhenUnparseable(t *testing.T) {
	cond, binds := continuationCondition(CompositeCursor{SortTS: "not-a-timestamp", ID: 1}, db.DialectPostgres)
	if !strings.Contains(cond, "to_char") || binds[0] != "not-a-timestamp" {
		t.Errorf("cond = %s binds = %#v", cond, binds)
	}
}

func TestSQLiteCursorUsesRawText(t *testing.T) {
	cond, _ := continuationCondition(CompositeCursor{SortTS: "2026-07-07 12:00:00", ID: 1}, db.DialectSQLite)
	if strings.Contains(cond, "to_char") {
		t.Errorf("cond = %s", cond)
	}
}

// PostgreSQL's CROSS JOIN takes no ON and INDEXED BY does not exist there.
func TestPostgresCountSQLHasNoSQLiteOnlySyntax(t *testing.T) {
	snapshot := "2024-01-01 00:00:00"
	counter := positionCounter{userID: 1, entryID: 1, slices: countSlices(&Filter{UnreadOnly: true, ReadAfter: &snapshot})}
	if len(counter.slices) != 2 {
		t.Fatalf("slices = %d, want 2", len(counter.slices))
	}
	for _, slice := range counter.slices {
		sqlText, _ := counter.sliceSQL(db.DialectPostgres, " AND e.id > $2", slice, " INDEXED BY idx_x")
		if strings.Contains(sqlText, "CROSS JOIN") || strings.Contains(sqlText, "INDEXED BY") {
			t.Errorf("sql = %s", sqlText)
		}
	}
}

func TestPostgresNeighborSQLHasNoSQLiteOnlySyntax(t *testing.T) {
	snapshot := "2024-01-01 00:00:00"
	n := neighborQuery{userID: 1, entryID: 1, sortTime: "2024-01-01 00:00:00", next: true,
		filter: &Filter{UnreadOnly: true, ReadAfter: &snapshot}}
	sqlText, _ := n.build(db.DialectPostgres)
	if strings.Contains(sqlText, "CROSS JOIN") || strings.Contains(sqlText, "INDEXED BY") {
		t.Errorf("sql = %s", sqlText)
	}
}
