package server

import (
	"errors"
	"net/http"
	"net/url"
	"strconv"
)

// entriesQuery is the query string of the list pages, read as axum's
// Query<EntriesQuery> does: an absent key is nil, an empty one is a value.
type entriesQuery struct {
	// Fragment is 1 for the Load More and search refresh responses.
	Fragment *uint8
	After    *string
	// Status is unread, read or starred on feed and category pages.
	Status *string
	// Entry is the deep-link entry to pre-open.
	Entry *int64
	Q     *string
	// Snapshot is the render time echoed back by Load More.
	Snapshot *string
	// Pane is 1 on category pages for the pane swap.
	Pane *uint8
}

func (q entriesQuery) isFragment() bool { return q.Fragment != nil && *q.Fragment == 1 }
func (q entriesQuery) isPane() bool     { return q.Pane != nil && *q.Pane == 1 }

// loadsMore is a Load More request: a fragment with a cursor key.
func (q entriesQuery) loadsMore() bool { return q.isFragment() && q.After != nil }

func deserializeFailure(w http.ResponseWriter, reason string) {
	rejection(w, http.StatusBadRequest, "Failed to deserialize query string: "+reason)
}

// parseEntriesQuery reads the query, answering 400 like axum when a value or
// key is unusable. It reports false after answering.
func parseEntriesQuery(w http.ResponseWriter, r *http.Request) (entriesQuery, bool) {
	values := r.URL.Query()
	var query entriesQuery
	reader := queryReader{values: values}
	query.Fragment = reader.small("fragment")
	query.After = reader.text("after")
	query.Status = reader.text("status")
	query.Entry = reader.id("entry")
	query.Q = reader.text("q")
	query.Snapshot = reader.text("snapshot")
	query.Pane = reader.small("pane")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return entriesQuery{}, false
	}
	return query, true
}

// queryReader keeps the first failure, so the caller reads fields in a row.
type queryReader struct {
	values url.Values
	err    error
}

// lookup returns the single value of name, or false when it is absent or
// the key is repeated (serde rejects a repeated field).
func (r *queryReader) lookup(name string) (string, bool) {
	list, present := r.values[name]
	if !present || r.err != nil {
		return "", false
	}
	if len(list) > 1 {
		r.err = errors.New("duplicate field `" + name + "`")
		return "", false
	}
	return list[0], true
}

func (r *queryReader) text(name string) *string {
	value, ok := r.lookup(name)
	if !ok {
		return nil
	}
	return &value
}

func (r *queryReader) small(name string) *uint8 {
	value, ok := r.lookup(name)
	if !ok {
		return nil
	}
	parsed, err := strconv.ParseUint(value, 10, 8)
	if err != nil {
		r.err = errors.New(name + ": " + integerFailure(value, err))
		return nil
	}
	narrow := uint8(parsed)
	return &narrow
}

func (r *queryReader) id(name string) *int64 {
	value, ok := r.lookup(name)
	if !ok {
		return nil
	}
	parsed, err := strconv.ParseInt(value, 10, 64)
	if err != nil {
		r.err = errors.New(name + ": " + integerFailure(value, err))
		return nil
	}
	return &parsed
}

// integerFailure words a parse failure as Rust's ParseIntError does.
func integerFailure(value string, err error) string {
	switch {
	case value == "":
		return "cannot parse integer from empty string"
	case errors.Is(err, strconv.ErrRange) && value[0] == '-':
		return "number too small to fit in target type"
	case errors.Is(err, strconv.ErrRange):
		return "number too large to fit in target type"
	}
	return "invalid digit found in string"
}
