package entry

import "github.com/henry40408/rdrs/internal/db"

// NeighborSQL exposes the neighbour lookup SQL so plan tests can run it.
func NeighborSQL(dialect db.Dialect, userID, entryID int64, sortTime string, next bool, f *Filter) (string, []any) {
	return neighborQuery{userID: userID, entryID: entryID, sortTime: sortTime, filter: f, next: next}.build(dialect)
}

// PositionSliceSQL exposes one position count SQL over slice of f.
func PositionSliceSQL(userID, entryID int64, f *Filter, slice int, cmp, unreadIndex string) (string, []any) {
	counter := positionCounter{userID: userID, entryID: entryID, slices: countSlices(f)}
	return counter.sliceSQL(db.DialectSQLite, cmp, counter.slices[slice], unreadIndex)
}

// ListSQL exposes the page SQL of a continuation list.
func ListSQL(dialect db.Dialect, userID int64, f *Filter, p *ContinuationParams) (string, []any) {
	return continuationSQL(dialect, userID, f, p, "")
}
