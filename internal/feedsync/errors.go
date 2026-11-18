// Package feedsync refreshes feeds: a conditional fetch, a parse, and one
// transaction that stores every entry and the fetch result.
package feedsync

// ErrorKind says which stage of a refresh failed.
type ErrorKind int

// The failure stages. Their texts are shown to users, so they match Rust.
const (
	KindInvalidURL ErrorKind = iota
	KindFetch
	KindParse
)

// Error is a failed refresh. Detail is also what is stored in feed.fetch_error.
type Error struct {
	Kind   ErrorKind
	Detail string
}

func (e *Error) Error() string {
	switch e.Kind {
	case KindInvalidURL:
		return "Invalid URL"
	case KindFetch:
		return "Failed to fetch: " + e.Detail
	default:
		return "Failed to parse feed: " + e.Detail
	}
}
