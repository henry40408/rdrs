package server

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/sanitize"
	"github.com/henry40408/rdrs/internal/search"
	"github.com/henry40408/rdrs/internal/textutil"
	"github.com/henry40408/rdrs/internal/views"
)

const untitledEntry = "(no title)"

// snapshotNow is the render-time snapshot in the UTC form datetime('now')
// writes to entry.read_at; it is echoed back as the "snapshot" parameter.
func (a *App) snapshotNow() string {
	return a.Now().UTC().Format(entry.SnapshotFormat)
}

// entryTitle is the title shown for an entry, entities decoded.
func entryTitle(title *string) string {
	if title == nil {
		return untitledEntry
	}
	return textutil.DecodeHTMLEntities(*title)
}

// cleanLink strips tracking parameters from an entry link.
func cleanLink(link *string) *string {
	if link == nil {
		return nil
	}
	cleaned := sanitize.StripTrackingParams(*link)
	return &cleaned
}

// publishedAt is when the entry was published, else when it arrived.
func publishedAt(e *entry.WithFeed) time.Time {
	if e.PublishedAt != nil {
		return *e.PublishedAt
	}
	return e.CreatedAt
}

// rowViewFrom is one row of an entry list; status is empty without a summary.
func (a *App) rowViewFrom(e *entry.WithFeed, status models.SummaryStatus) views.EntryRow {
	published := publishedAt(e)
	feedTitle := "(no feed)"
	if e.FeedTitle != nil {
		feedTitle = *e.FeedTitle
	}
	return views.EntryRow{
		ID: e.ID, FeedID: e.FeedID, FeedTitle: feedTitle, FeedHasIcon: e.FeedHasIcon,
		CategoryID: e.CategoryID, Title: entryTitle(e.Title), Link: cleanLink(e.Link),
		PublishedISO: rfc3339Offset(published), PublishedRelative: relativeTimeCompact(&published, a.Now()),
		IsRead: e.ReadAt != nil, IsStarred: e.StarredAt != nil, SummaryStatus: string(status),
	}
}

// entriesPageSize is the rows per list page. It is read per request, not
// cached with the chrome, so a saved change applies at once; clamped because
// the value is used as a slice length.
func (a *App) entriesPageSize(ctx context.Context, userID int64) int64 {
	size, err := models.EntriesPerPage(ctx, a.DB, userID)
	if err != nil {
		size = models.DefaultEntriesPerPage
	}
	return min(max(size, models.MinEntriesPerPage), models.MaxEntriesPerPage)
}

// scopedSearch is a list page's ?q= run through the search-syntax parser.
type scopedSearch struct {
	// raw is the query as typed; nil when blank.
	raw   *string
	query search.Node
	// failure is set when raw does not parse; the page must then list nothing
	// rather than fall back to an unfiltered list.
	failure *string
}

func parseScopedSearch(q *string) scopedSearch {
	if q == nil || strings.TrimSpace(*q) == "" {
		return scopedSearch{}
	}
	node, err := search.Parse(*q)
	if err == nil {
		return scopedSearch{raw: q, query: node}
	}
	message := searchErrorMessage(*q, err)
	return scopedSearch{raw: q, failure: &message}
}

// searchErrorMessage is "Search syntax error (near character N): ...", shared
// by /search and the list pages so the two cannot drift.
func searchErrorMessage(q string, err error) string {
	var parseErr *search.ParseError
	if !errors.As(err, &parseErr) {
		return "Search syntax error: " + err.Error()
	}
	// Byte offset to 1-based character position; an offset inside a character
	// counts as the start, as the Rust server did.
	position, offset := 1, parseErr.Position
	if offset == len(q) || (offset < len(q) && utf8.RuneStart(q[offset])) {
		position = utf8.RuneCountInString(q[:offset]) + 1
	}
	return fmt.Sprintf("Search syntax error (near character %d): %s", position, parseErr.Message)
}

// countMatchingUnread is what "Mark N matching as Read" would mark: scope
// narrowed by the search and by the render snapshot, like markReadScoped. It
// is nil without a valid search.
func (a *App) countMatchingUnread(ctx context.Context, userID int64, scope entry.Filter, found scopedSearch, snapshotAt string) *int64 {
	if found.query == nil {
		return nil
	}
	storedBefore, err := entry.ParseSnapshot(&snapshotAt)
	if err != nil {
		return nil
	}
	filter := scope
	filter.Query, filter.UnreadOnly, filter.StoredBefore = found.query, true, storedBefore
	count, err := entry.CountByUser(ctx, a.DB, userID, &filter)
	if err != nil {
		return nil
	}
	return &count
}

// pageRequest is one page of a list: who asks, what to list and where from.
type pageRequest struct {
	userID   int64
	filter   entry.Filter
	found    scopedSearch
	sort     entry.SortOrder
	pageSize int64
	cursor   entry.ContinuationCursor
}

// buildSearchedPage is buildPage, but an unparseable search lists nothing.
func (a *App) buildSearchedPage(ctx context.Context, request pageRequest) ([]views.EntryRow, *string) {
	if request.found.failure != nil {
		return nil, nil
	}
	request.filter.Query = request.found.query
	return a.buildPage(ctx, request)
}

// buildPage fetches a page of rows plus the next cursor, if more exist. A
// failing query shows an empty list rather than an error page.
func (a *App) buildPage(ctx context.Context, request pageRequest) ([]views.EntryRow, *string) {
	rows, next, err := a.fetchPage(ctx, request)
	if err != nil {
		return nil, nil
	}
	ids := make([]int64, len(rows))
	for i := range rows {
		ids[i] = rows[i].ID
	}
	statuses, err := models.SummaryStatusesFor(ctx, a.DB, request.userID, ids)
	if err != nil {
		return nil, nil
	}
	list := make([]views.EntryRow, len(rows))
	for i := range rows {
		list[i] = a.rowViewFrom(&rows[i], statuses[rows[i].ID])
	}
	return list, next
}

// fetchPage reads one more row than the page holds: its presence says another
// page exists, and the cursor comes from the last kept row.
func (a *App) fetchPage(ctx context.Context, request pageRequest) ([]entry.WithFeed, *string, error) {
	filter := request.filter
	found, err := entry.ListByUserWithContinuationTS(ctx, a.DB, request.userID, &filter, &entry.ContinuationParams{
		Limit: request.pageSize + 1, Continuation: request.cursor, SortOrder: request.sort,
	})
	if err != nil {
		return nil, nil, err
	}
	next := entry.NextContinuation(found, int(request.pageSize))
	rows := make([]entry.WithFeed, 0, min(len(found), int(request.pageSize)))
	for i := 0; i < len(found) && i < int(request.pageSize); i++ {
		rows = append(rows, found[i].WithFeed)
	}
	return rows, next, nil
}
