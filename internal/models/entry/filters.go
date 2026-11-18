package entry

import (
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/search"
)

// query accumulates WHERE conditions and their positional binds ($1, $2, ...).
// Every caller binds the user id first, so $1 is the user id.
type query struct {
	dialect    db.Dialect
	conditions []string
	binds      []any
}

func newUserQuery(dialect db.Dialect, userID int64) *query {
	return &query{dialect: dialect, conditions: []string{"c.user_id = $1"}, binds: []any{userID}}
}

// bind appends a value and returns its placeholder, such as "$3".
func (q *query) bind(value any) string {
	q.binds = append(q.binds, value)
	return "$" + strconv.Itoa(len(q.binds))
}

func (q *query) where(condition string) {
	q.conditions = append(q.conditions, condition)
}

func (q *query) whereClause() string {
	return strings.Join(q.conditions, " AND ")
}

// cursorLayout is the entry timestamp text layout.
const cursorLayout = "2006-01-02 15:04:05"

// parseCursorTS parses cursor text from FetchSortTS; ok is false when it is
// malformed, so callers fall back to string comparison.
func parseCursorTS(text string) (time.Time, bool) {
	parsed, err := time.Parse(cursorLayout, text)
	return parsed, err == nil
}

// isNoEntrySidePredicate reports a filter with nothing on the entry table
// itself, only the ownership join.
func (f *Filter) isNoEntrySidePredicate() bool {
	return f.FeedID == nil && f.CategoryID == nil && !f.UnreadOnly && !f.StarredOnly &&
		!f.ReadOnly && f.Search == nil && f.HasSummary == nil && f.PublishedBetween == nil &&
		f.StoredBefore == nil && f.Query == nil
}

// publishedSortHint is the SQLite index hint for ORDER BY
// COALESCE(published_at, created_at) queries; without it the planner walks
// category -> feed -> entry over every row. Pass it through Dialect.IndexedBy.
func publishedSortHint(f *Filter) string {
	switch {
	case f.StarredOnly:
		return " INDEXED BY idx_entry_starred_sort"
	case f.ReadOnly:
		return " INDEXED BY idx_entry_read_sort"
	case f.UnreadOnly && f.ReadAfter == nil:
		// Not for snapshots: read_at >= ? is not covered by the partial index.
		return " INDEXED BY idx_entry_unread_sort"
	case f.isNoEntrySidePredicate():
		return " INDEXED BY idx_entry_sort_ts"
	}
	return ""
}

// readSince is "e.read_at" at or after readAfter; >= so a same-second read
// still counts.
func (q *query) readSince(readAfter string) string {
	if parsed, ok := parseCursorTS(readAfter); ok && q.dialect == db.DialectPostgres {
		return "e.read_at >= " + q.bind(db.FormatSpace(parsed))
	}
	return q.dialect.CursorTS("e.read_at") + " >= " + q.bind(readAfter)
}

func (q *query) applyFilter(f *Filter) {
	if f.FeedID != nil {
		q.where("e.feed_id = " + q.bind(*f.FeedID))
	}
	if f.CategoryID != nil {
		q.where("c.id = " + q.bind(*f.CategoryID))
	}
	q.applyReadState(f)
	if f.Search != nil {
		q.bind("%" + *f.Search + "%")
		index := len(q.binds)
		q.where(fmt.Sprintf("(%s OR %s)",
			q.dialect.LikeCI("e.title", index), q.dialect.LikeCI("e.content_text", index)))
	}
	if f.PublishedBetween != nil {
		start := q.bind(q.dialect.TimeBound(f.PublishedBetween.Start))
		end := q.bind(q.dialect.TimeBound(f.PublishedBetween.End))
		q.where(fmt.Sprintf("(%s >= %s AND %s < %s)", publishedSortColumn, start, publishedSortColumn, end))
	}
	if f.StoredBefore != nil {
		q.where("e.created_at < " + q.bind(q.dialect.TimeBound(*f.StoredBefore)))
	}
	if f.HasSummary != nil {
		q.where(summaryExists(*f.HasSummary))
	}
	if f.Query != nil {
		q.where(q.renderQuery(f.Query))
	}
}

func (q *query) applyReadState(f *Filter) {
	if f.UnreadOnly {
		if f.ReadAfter != nil {
			// Snapshot: entries read during this page view stay in the unread set.
			q.where("(e.read_at IS NULL OR " + q.readSince(*f.ReadAfter) + ")")
		} else {
			q.where("e.read_at IS NULL")
		}
	}
	if f.StarredOnly {
		q.where("e.starred_at IS NOT NULL")
	}
	if f.ReadOnly {
		q.where("e.read_at IS NOT NULL")
	}
}

// summaryExists relies on $1 being the user id.
func summaryExists(want bool) string {
	exists := "EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = $1 AND es.entry_id = e.id)"
	if want {
		return exists
	}
	return "NOT " + exists
}

// likeContains backslash-escapes LIKE metacharacters and wraps the value in
// %...% (it needs ESCAPE '\').
func likeContains(value string) string {
	var out strings.Builder
	out.WriteByte('%')
	for _, r := range value {
		if r == '\\' || r == '%' || r == '_' {
			out.WriteByte('\\')
		}
		out.WriteRune(r)
	}
	out.WriteByte('%')
	return out.String()
}

// renderQuery renders a query AST into a WHERE fragment, binding one value per
// valued leaf.
func (q *query) renderQuery(node search.Node) string {
	switch n := node.(type) {
	case search.And:
		return "(" + q.renderQuery(n.Left) + " AND " + q.renderQuery(n.Right) + ")"
	case search.Or:
		return "(" + q.renderQuery(n.Left) + " OR " + q.renderQuery(n.Right) + ")"
	case search.Not:
		return "(NOT " + q.renderQuery(n.Inner) + ")"
	case search.Text:
		// COALESCE to FALSE so negation (-foo) keeps NULL-column rows.
		q.bind(likeContains(n.Value))
		index := len(q.binds)
		return fmt.Sprintf("COALESCE((%s OR %s), %s)",
			q.dialect.LikeCIEscape("e.title", index),
			q.dialect.LikeCIEscape("e.content_text", index), q.dialect.BoolFalse())
	case search.Field:
		return q.renderLike(textFieldColumn(n.Field), n.Value)
	case search.Source:
		return q.renderLike(sourceColumn(n.Kind), n.Value)
	case search.StatusFilter:
		return statusCondition(n.Status)
	case search.Date:
		return q.renderDate(n)
	}
	panic(fmt.Sprintf("entry: unknown search node %T", node))
}

func (q *query) renderLike(column, value string) string {
	q.bind(likeContains(value))
	return fmt.Sprintf("COALESCE(%s, %s)", q.dialect.LikeCIEscape(column, len(q.binds)), q.dialect.BoolFalse())
}

func textFieldColumn(field search.TextField) string {
	if field == search.FieldAuthor {
		return "e.author"
	}
	return "e.title"
}

func sourceColumn(kind search.SourceKind) string {
	if kind == search.SourceCategory {
		return "c.name"
	}
	return "f.title"
}

func statusCondition(status search.Status) string {
	switch status {
	case search.StatusRead:
		return "e.read_at IS NOT NULL"
	case search.StatusStarred:
		return "e.starred_at IS NOT NULL"
	}
	return "e.read_at IS NULL"
}

func (q *query) renderDate(n search.Date) string {
	comparison := "<"
	if n.Bound == search.After {
		comparison = ">="
	}
	epoch := q.dialect.Epoch(publishedSortColumn)
	return epoch + " " + comparison + " " + q.bind(n.Date.Unix())
}

// applyTimeBounds applies the ot / nt (oldest / newest epoch seconds) bounds.
func (q *query) applyTimeBounds(p *ContinuationParams) {
	epoch := q.dialect.Epoch(publishedSortColumn)
	if p.OldestTS != nil {
		q.where(epoch + " >= " + q.bind(*p.OldestTS))
	}
	if p.NewestTS != nil {
		q.where(epoch + " <= " + q.bind(*p.NewestTS))
	}
}

// applyContinuation applies the continuation cursor. The bounded-OR form lets
// SQLite use an indexed range scan even on a COALESCE sort key.
func (q *query) applyContinuation(p *ContinuationParams) {
	switch cursor := p.Continuation.(type) {
	case CompositeCursor:
		q.applyCompositeCursor(p, cursor)
	case LegacyIDCursor:
		comparison := "<"
		if p.OldestFirst {
			comparison = ">"
		}
		q.where("e.id " + comparison + " " + q.bind(int64(cursor)))
	}
}

func (q *query) applyCompositeCursor(p *ContinuationParams, cursor CompositeCursor) {
	sortColumn := sortColumn(p.SortOrder)
	outer, inner := "<=", "<"
	if p.OldestFirst {
		outer, inner = ">=", ">"
	}
	// PostgreSQL compares the raw column (sargable) unless the cursor text is
	// unparseable, when to_char is the fallback.
	expr := q.dialect.CursorTS(sortColumn)
	bound := cursor.SortTS
	if parsed, ok := parseCursorTS(cursor.SortTS); ok && q.dialect == db.DialectPostgres {
		expr = sortColumn
		bound = db.FormatSpace(parsed)
	}
	first, second, id := q.bind(bound), q.bind(bound), q.bind(cursor.ID)
	q.where(fmt.Sprintf("%s %s %s AND (%s %s %s OR e.id %s %s)",
		expr, outer, first, expr, inner, second, inner, id))
}

// placeholders is "$from, $from+1, ..." for count values.
func placeholders(from, count int) string {
	parts := make([]string, count)
	for i := range parts {
		parts[i] = "$" + strconv.Itoa(from+i)
	}
	return strings.Join(parts, ", ")
}

func int64Args(ids []int64) []any {
	args := make([]any, len(ids))
	for i, id := range ids {
		args[i] = id
	}
	return args
}
