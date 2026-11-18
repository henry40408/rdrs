package server

import (
	"context"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/pixel"
	"github.com/henry40408/rdrs/internal/sanitize"
)

const (
	defaultStreamCount     = 20
	maxStreamContentsCount = 1000
	maxStreamItemIDsCount  = 10000
)

// streamQuery is the query string of stream/contents and stream/items/ids;
// the latter ignores the extensions (Q, HasSummary, NoContent).
type streamQuery struct {
	Stream     *string
	Count      *int64
	Cursor     *string
	ExcludeTag *string
	IncludeTag *string
	OldestTS   *int64
	NewestTS   *int64
	Order      *string
	Q          *string
	HasSummary *string
	NoContent  bool
}

// parseStreamQuery reads the query as axum's Query does, answering 400 for an
// unusable value. It reports false after answering.
func parseStreamQuery(w http.ResponseWriter, r *http.Request) (streamQuery, bool) {
	reader := queryReader{values: r.URL.Query()}
	query := streamQuery{
		Stream: reader.text("s"), Count: reader.id("n"), Cursor: reader.text("c"),
		ExcludeTag: reader.text("xt"), IncludeTag: reader.text("it"),
		OldestTS: reader.id("ot"), NewestTS: reader.id("nt"), Order: reader.text("r"),
		Q: reader.text("q"), HasSummary: reader.text("has_summary"), NoContent: reader.flag("no_content"),
	}
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return streamQuery{}, false
	}
	return query, true
}

// clampedCount is n bounded to [0, limit]: a negative n would make LIMIT
// unbounded and return everything.
func (q streamQuery) clampedCount(limit int64) int64 {
	count := int64(defaultStreamCount)
	if q.Count != nil {
		count = *q.Count
	}
	return min(max(count, 0), limit)
}

func sortOrderOf(stream greader.Stream) entry.SortOrder {
	switch stream.Kind {
	case greader.Read:
		return entry.SortReadAt
	case greader.Starred:
		return entry.SortStarredAt
	}
	return entry.SortPublishedAt
}

// pagination is the cursor, bounds and order of a page of count entries; it
// asks for one more, to know whether a next page exists.
func (q streamQuery) pagination(stream greader.Stream, count int64) *entry.ContinuationParams {
	params := &entry.ContinuationParams{
		OldestFirst: q.Order != nil && *q.Order == "o",
		Limit:       count + 1,
		OldestTS:    q.OldestTS, NewestTS: q.NewestTS,
		SortOrder: sortOrderOf(stream),
	}
	if q.Cursor != nil {
		if cursor, ok := entry.ParseCursor(*q.Cursor); ok {
			params.Continuation = cursor
		}
	}
	return params
}

// streamFilter is the filter the stream and the xt/it tags describe, before
// a label or feed is resolved to an id.
func streamFilter(stream greader.Stream, excludeTag, includeTag *string) *entry.Filter {
	filter := &entry.Filter{}
	switch stream.Kind {
	case greader.Read:
		filter.ReadOnly = true
	case greader.Starred:
		filter.StarredOnly = true
	case greader.KeptUnread:
		filter.UnreadOnly = true
	}
	// Only excluding "read" maps to a filter; other tags are ignored.
	if tag, ok := parseOptionalStream(excludeTag); ok && tag.Kind == greader.Read {
		filter.UnreadOnly = true
	}
	if tag, ok := parseOptionalStream(includeTag); ok {
		switch tag.Kind {
		case greader.Read:
			filter.ReadOnly = true
		case greader.Starred:
			filter.StarredOnly = true
		}
	}
	return filter
}

func parseOptionalStream(raw *string) (greader.Stream, bool) {
	if raw == nil {
		return greader.Stream{}, false
	}
	stream, err := greader.ParseStream(*raw)
	return stream, err == nil
}

// scopeToStream narrows filter to the feed or label a stream names; a name the
// user does not own is a 404.
func (a *App) scopeToStream(ctx context.Context, userID int64, stream greader.Stream, filter *entry.Filter) error {
	switch stream.Kind {
	case greader.Feed:
		found, err := models.FindFeedByURLForUser(ctx, a.DB, stream.Name, userID)
		if err != nil {
			return DatabaseError(err)
		}
		if found == nil {
			return ErrFeedNotFound
		}
		filter.FeedID = &found.ID
	case greader.Label:
		found, err := models.FindCategoryByNameAndUser(ctx, a.DB, stream.Name, userID)
		if err != nil {
			return DatabaseError(err)
		}
		if found == nil {
			return ErrCategoryNotFound
		}
		filter.CategoryID = &found.ID
	}
	return nil
}

// parseStreamParam parses a stream id, answering 400 when it is malformed.
func parseStreamParam(w http.ResponseWriter, raw string) (greader.Stream, bool) {
	stream, err := greader.ParseStream(raw)
	if err != nil {
		writeError(w, appError(err))
		return greader.Stream{}, false
	}
	return stream, true
}

// greaderStreamContents is GET /reader/api/0/stream/contents/{*stream}.
func (a *App) greaderStreamContents(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	query, ok := parseStreamQuery(w, r)
	if !ok {
		return
	}
	stream, ok := parseStreamParam(w, r.PathValue("stream"))
	if !ok {
		return
	}
	response, err := a.streamContents(r.Context(), user.user.ID, stream, query)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, response)
}

func (a *App) streamContents(ctx context.Context, userID int64, stream greader.Stream, query streamQuery) (*greader.StreamContents, error) {
	count := query.clampedCount(maxStreamContentsCount)
	filter := streamFilter(stream, query.ExcludeTag, query.IncludeTag)
	filter.Search = query.Q
	if query.HasSummary != nil {
		filter.HasSummary = ptr(*query.HasSummary == "true")
	}
	if err := a.scopeToStream(ctx, userID, stream, filter); err != nil {
		return nil, err
	}
	rows, err := entry.ListByUserWithContinuationTS(ctx, a.DB, userID, filter, query.pagination(stream, count))
	if err != nil {
		return nil, DatabaseError(err)
	}
	continuation := entry.NextContinuation(rows, int(count))
	entries := make([]entry.WithFeed, 0, count)
	for _, row := range rows[:min(int(count), len(rows))] {
		entries = append(entries, row.WithFeed)
	}
	items, err := a.greaderItems(ctx, userID, entries, query.NoContent)
	if err != nil {
		return nil, err
	}
	return &greader.StreamContents{
		ID: stream.String(), Updated: firstUpdated(entries), Continuation: continuation, Items: items,
	}, nil
}

// firstUpdated is the update time of the first entry, 0 for none.
func firstUpdated(entries []entry.WithFeed) int64 {
	if len(entries) == 0 {
		return 0
	}
	return entries[0].UpdatedAt.Unix()
}

// greaderItems converts entries to items with one settings read and one status
// read for the whole page.
func (a *App) greaderItems(ctx context.Context, userID int64, entries []entry.WithFeed, noContent bool) ([]greader.Item, error) {
	ids := make([]int64, len(entries))
	for i := range entries {
		ids[i] = entries[i].ID
	}
	stored, err := models.SummaryStatusesFor(ctx, a.DB, userID, ids)
	if err != nil {
		return nil, DatabaseError(err)
	}
	enabledAt, err := models.PixelTrackingEnabledAt(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	options := itemOptions{
		noContent: noContent,
		// Absolute: rendered inside another app with a different host.
		pixel: pixel.Context{UserID: userID, EnabledAt: enabledAt, Secret: a.Config.Secret, BaseURL: a.Config.PublicBaseURL},
	}
	statuses := mergeSummaryStatuses(stored, a.Summaries, userID, entries)
	items := make([]greader.Item, len(entries))
	for i := range entries {
		var status *string
		if value, ok := statuses[entries[i].ID]; ok {
			status = ptr(string(value))
		}
		items[i] = a.greaderItem(&entries[i], status, options)
	}
	return items, nil
}

// mergeSummaryStatuses lets an in-flight cache status win over the stored one.
func mergeSummaryStatuses(stored map[int64]models.SummaryStatus, inFlight *cache.Summary, userID int64, entries []entry.WithFeed) map[int64]models.SummaryStatus {
	merged := make(map[int64]models.SummaryStatus, len(stored))
	for id, status := range stored {
		merged[id] = status
	}
	for i := range entries {
		if status, ok := inFlight.GetStatus(userID, entries[i].ID); ok {
			merged[entries[i].ID] = status
		}
	}
	return merged
}

// itemOptions are the settings shared by every item of one response.
type itemOptions struct {
	// noContent skips sanitising for metadata-only list views.
	noContent bool
	pixel     pixel.Context
}

// rfc3339Chrono renders t as chrono's DateTime::to_rfc3339 does: "+00:00" for
// UTC, and a fraction of 3, 6 or 9 digits only when non-zero.
func rfc3339Chrono(t time.Time) string {
	return strings.TrimSuffix(chronoUTC(t), "Z") + "+00:00"
}

func optionalRFC3339(t *time.Time) *string {
	if t == nil {
		return nil
	}
	return ptr(rfc3339Chrono(*t))
}

// greaderItem converts an entry to the Google Reader item shape.
func (a *App) greaderItem(e *entry.WithFeed, summaryStatus *string, options itemOptions) greader.Item {
	categories := []string{greader.ReadingListID}
	if e.ReadAt != nil {
		categories = append(categories, greader.ReadID)
	}
	if e.StarredAt != nil {
		categories = append(categories, greader.StarredID)
	}
	categories = append(categories, "user/-/label/"+e.CategoryName)

	publishedAt := e.CreatedAt
	if e.PublishedAt != nil {
		publishedAt = *e.PublishedAt
	}
	published := publishedAt.Unix()
	link := deref(e.Link)
	fullContent, summary := a.itemBodies(e, options)
	return greader.Item{
		ID: greader.ItemID(e.ID), Published: published, Updated: e.UpdatedAt.Unix(),
		CrawlTimeMsec: strconv.FormatInt(e.CreatedAt.UnixMilli(), 10),
		TimestampUsec: strconv.FormatInt(published*1_000_000, 10),
		Title:         deref(e.Title), Categories: categories,
		Summary:   greader.Content{Content: summary},
		Canonical: []greader.Link{{Href: link}},
		Alternate: []greader.AlternateLink{{Href: link, Type: "text/html"}},
		Author:    deref(e.Author),
		Origin:    greader.Origin{StreamID: "feed/" + e.FeedURL, Title: deref(e.FeedTitle), HTMLURL: deref(e.SiteURL)},

		EntryID: e.ID, FeedID: e.FeedID, CategoryID: e.CategoryID, CategoryName: e.CategoryName,
		FeedHasIcon: e.FeedHasIcon,
		ReadAt:      optionalRFC3339(e.ReadAt), StarredAt: optionalRFC3339(e.StarredAt),
		PublishedAt: optionalRFC3339(e.PublishedAt),
		FullContent: fullContent, SummaryStatus: summaryStatus,
	}
}

// itemBodies sanitises once for both "summary" and "_content". The pixel goes
// on after sanitising, which would otherwise strip it.
func (a *App) itemBodies(e *entry.WithFeed, options itemOptions) (content *string, summary string) {
	if options.noContent {
		return nil, ""
	}
	sanitizeContext := sanitize.Context{
		Secret: a.Config.Secret, BaseURL: e.ContentBaseURL(), ProxyBaseURL: a.Config.PublicBaseURL,
	}
	if e.CustomReferrer != nil {
		sanitizeContext.Referrer = *e.CustomReferrer
	}
	render := func(raw string) string {
		return options.pixel.MaybeInject(sanitize.HTML(raw, sanitizeContext), e.ID, e.CreatedAt)
	}
	if e.Content != nil {
		rendered := render(*e.Content)
		return &rendered, rendered
	}
	return nil, render(deref(e.Summary))
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// greaderStreamItemIDs is GET /reader/api/0/stream/items/ids.
func (a *App) greaderStreamItemIDs(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	query, ok := parseStreamQuery(w, r)
	if !ok {
		return
	}
	stream, ok := parseStreamParam(w, streamOrDefault(query.Stream))
	if !ok {
		return
	}
	response, err := a.streamItemIDs(r.Context(), user.user.ID, stream, query)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, response)
}

func streamOrDefault(raw *string) string {
	if raw == nil {
		return greader.ReadingListID
	}
	return *raw
}

func (a *App) streamItemIDs(ctx context.Context, userID int64, stream greader.Stream, query streamQuery) (*greader.StreamItemIDs, error) {
	count := query.clampedCount(maxStreamItemIDsCount)
	filter := streamFilter(stream, query.ExcludeTag, query.IncludeTag)
	if err := a.scopeToStream(ctx, userID, stream, filter); err != nil {
		return nil, err
	}
	pagination := query.pagination(stream, count)
	rows, err := entry.ListIDsByUser(ctx, a.DB, userID, filter, pagination)
	if err != nil {
		return nil, DatabaseError(err)
	}
	hasMore := int64(len(rows)) > count
	rows = rows[:min(int(count), len(rows))]
	response := &greader.StreamItemIDs{ItemRefs: make([]greader.ItemRef, len(rows))}
	for i, row := range rows {
		response.ItemRefs[i] = greader.ItemRef{
			ID: strconv.FormatInt(row.ID, 10), TimestampUsec: strconv.FormatInt(row.TimestampUS, 10),
		}
	}
	if !hasMore || len(rows) == 0 {
		return response, nil
	}
	last := rows[len(rows)-1].ID
	sortTS, err := entry.FetchSortTS(ctx, a.DB, last, pagination.SortOrder)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if sortTS != nil {
		response.Continuation = ptr(entry.EncodeCursor(*sortTS, last))
	}
	return response, nil
}

// greaderStreamItemCount is GET /reader/api/0/stream/items/count.
func (a *App) greaderStreamItemCount(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	reader := queryReader{values: r.URL.Query()}
	raw := reader.text("s")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return
	}
	stream, ok := parseStreamParam(w, streamOrDefault(raw))
	if !ok {
		return
	}
	filter := streamFilter(stream, nil, nil)
	if err := a.scopeToStream(r.Context(), user.user.ID, stream, filter); err != nil {
		writeError(w, err)
		return
	}
	count, err := entry.CountByUser(r.Context(), a.DB, user.user.ID, filter)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	writeText(w, strconv.FormatInt(count, 10))
}

// greaderItemsContentsGet is GET /reader/api/0/stream/items/contents.
func (a *App) greaderItemsContentsGet(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	a.writeItemsByIDs(w, r, user, r.URL.Query()["i"])
}

// greaderItemsContentsPost is POST /reader/api/0/stream/items/contents.
func (a *App) greaderItemsContentsPost(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	a.writeItemsByIDs(w, r, user, form["i"])
}

// writeItemsByIDs answers stream/items/contents for the item ids sent; an id
// that is missing or not the user's is silently left out.
func (a *App) writeItemsByIDs(w http.ResponseWriter, r *http.Request, user greaderUser, itemIDs []string) {
	response := greader.StreamContents{ID: greader.ReadingListID, Items: []greader.Item{}}
	if len(itemIDs) == 0 {
		writeJSON(w, http.StatusOK, response)
		return
	}
	entryIDs, err := entryIDsOf(itemIDs)
	if err != nil {
		writeError(w, appError(err))
		return
	}
	entries, err := entry.FindByIDsWithFeed(r.Context(), a.DB, user.user.ID, entryIDs)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	items, err := a.greaderItems(r.Context(), user.user.ID, entries, false)
	if err != nil {
		writeError(w, err)
		return
	}
	response.Updated, response.Items = firstUpdated(entries), items
	writeJSON(w, http.StatusOK, response)
}

// entryIDsOf converts item ids, failing on the first malformed one.
func entryIDsOf(itemIDs []string) ([]int64, error) {
	ids := make([]int64, len(itemIDs))
	for i, itemID := range itemIDs {
		id, err := greader.EntryID(itemID)
		if err != nil {
			return nil, err
		}
		ids[i] = id
	}
	return ids, nil
}
