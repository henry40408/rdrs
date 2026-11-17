package server

import (
	"context"
	"errors"
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/readability"
	"github.com/henry40408/rdrs/internal/sanitize"
	"github.com/henry40408/rdrs/internal/search"
	"github.com/henry40408/rdrs/internal/summarize"
)

// entryAPIRoutes are the JSON endpoints behind the reading pane.
func (a *App) entryAPIRoutes() []Route {
	return []Route{
		{"GET", "/api/entries/{id}/neighbors", a.entryNeighbors},
		{"GET", "/api/entries/{id}/position", a.entryPosition},
		{"POST", "/api/entries/{id}/fetch-full-content", a.fetchFullContentAPI},
		{"POST", "/api/entries/{id}/save", a.saveEntryAPI},
		{"POST", "/api/entries/{id}/summarize", a.summarizeEntryAPI},
		{"GET", "/api/entries/{id}/summary", a.entrySummaryAPI},
		{"DELETE", "/api/entries/{id}/summary", a.deleteEntrySummaryAPI},
	}
}

// flag reads a bool the way serde does: only "true" or "false".
func (r *queryReader) flag(name string) bool {
	value, ok := r.lookup(name)
	if !ok {
		return false
	}
	switch value {
	case "true":
		return true
	case "false":
		return false
	}
	r.err = errors.New(name + ": provided string was not `true` or `false`")
	return false
}

// beginEntryAPI authenticates and reads {id}; it reports false after answering.
func (a *App) beginEntryAPI(w http.ResponseWriter, r *http.Request) (userID, entryID int64, ok bool) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return 0, 0, false
	}
	entryID, ok = pathID(w, r)
	if !ok {
		return 0, 0, false
	}
	return principal.User.ID, entryID, true
}

// ownedEntry loads the entry or answers 404, whether it is missing or another
// user's.
func (a *App) ownedEntry(w http.ResponseWriter, r *http.Request, userID, entryID int64) (*entry.WithFeed, bool) {
	found, err := entry.FindByIDForUser(r.Context(), a.DB, userID, entryID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return nil, false
	}
	if found == nil {
		writeError(w, ErrEntryNotFound)
		return nil, false
	}
	return found, true
}

// neighborsFilter is the ownership check plus the list filter that neighbors
// and position share. It reports false after answering.
func (a *App) neighborsFilter(w http.ResponseWriter, r *http.Request, userID, entryID int64) (*entry.Filter, bool) {
	owned, err := entry.IsOwnedBy(r.Context(), a.DB, userID, entryID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return nil, false
	}
	if !owned {
		writeError(w, ErrEntryNotFound)
		return nil, false
	}
	reader := queryReader{values: r.URL.Query()}
	filter := &entry.Filter{
		FeedID: reader.id("feed_id"), CategoryID: reader.id("category_id"),
		UnreadOnly: reader.flag("unread_only"), StarredOnly: reader.flag("starred_only"), ReadOnly: reader.flag("read_only"),
	}
	if reader.values.Has("has_summary") {
		filter.HasSummary = ptr(reader.flag("has_summary"))
	}
	filter.ReadAfter = reader.text("read_after")
	q, today := reader.text("q"), reader.flag("today")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return nil, false
	}
	// Blank means no search; an unparseable query is a 400, which leaves the
	// pane's prev/next disabled just as the list page shows no rows.
	if q != nil && strings.TrimSpace(*q) != "" {
		node, err := search.Parse(*q)
		if err != nil {
			writeError(w, ValidationError(searchParseMessage(err)))
			return nil, false
		}
		filter.Query = node
	}
	if today {
		today := entry.TodayUnreadFilter(a.userLocation(r.Context(), userID))
		filter.PublishedBetween = today.PublishedBetween
	}
	return filter, true
}

// searchParseMessage is the bare parser message, without the page wording.
func searchParseMessage(err error) string {
	var parseErr *search.ParseError
	if errors.As(err, &parseErr) {
		return parseErr.Message
	}
	return err.Error()
}

func (a *App) entryNeighbors(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	filter, ok := a.neighborsFilter(w, r, userID, entryID)
	if !ok {
		return
	}
	neighbors, err := entry.FindNeighbors(r.Context(), a.DB, userID, entryID, filter)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	writeJSON(w, http.StatusOK, struct {
		PrevID *int64 `json:"prev_id"`
		NextID *int64 `json:"next_id"`
	}{neighbors.PrevID, neighbors.NextID})
}

// entryPosition is "12 / 48" for the reading pane toolbar; kept apart from
// neighbors so the counts never delay prev/next.
func (a *App) entryPosition(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	filter, ok := a.neighborsFilter(w, r, userID, entryID)
	if !ok {
		return
	}
	position, err := entry.FindPosition(r.Context(), a.DB, userID, entryID, filter)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	writeJSON(w, http.StatusOK, struct {
		Position int64 `json:"position"`
		Total    int64 `json:"total"`
		Limit    int64 `json:"limit"`
	}{position.Position, position.Total, position.Limit})
}

// extractionError maps a readability failure to its API error.
func extractionError(err error) error {
	var failure *readability.Error
	if errors.As(err, &failure) && failure.Kind == readability.KindFetch {
		return FetchError(failure.Detail)
	}
	return ErrInvalidURL
}

func (a *App) fetchFullContentAPI(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	found, ok := a.ownedEntry(w, r, userID, entryID)
	if !ok {
		return
	}
	if found.Link == nil {
		writeError(w, ValidationError("Entry has no link"))
		return
	}
	extracted, err := readability.FetchAndExtract(r.Context(), readability.Params{
		URL: *found.Link, UserAgent: a.Config.UserAgent, Fetcher: a.Fetcher, Retry: fetch.DefaultRetry(),
	})
	if err != nil {
		writeError(w, extractionError(err))
		return
	}
	// The entry link is the base URL for relative images.
	sanitizeContext := sanitize.Context{Secret: a.Config.Secret, BaseURL: *found.Link}
	if found.CustomReferrer != nil {
		sanitizeContext.Referrer = *found.CustomReferrer
	}
	writeJSON(w, http.StatusOK, struct {
		Title            *string `json:"title"`
		Content          string  `json:"content"`
		SanitizedContent string  `json:"sanitized_content"`
	}{extracted.Title, extracted.Content, sanitize.HTML(extracted.Content, sanitizeContext)})
}

func (a *App) saveEntryAPI(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	found, ok := a.ownedEntry(w, r, userID, entryID)
	if !ok {
		return
	}
	if found.Link == nil {
		writeError(w, ValidationError("Entry has no link to save"))
		return
	}
	config, err := a.usableServices(r.Context(), userID)
	if err != nil {
		writeError(w, err)
		return
	}
	if !config.HasAnyService() {
		writeError(w, ValidationError("No save services configured"))
		return
	}
	outcomes := a.saveToServices(r.Context(), config, bookmarkOf(found))
	response := struct {
		Results    []any `json:"results"`
		AllSuccess bool  `json:"all_success"`
	}{Results: []any{}, AllSuccess: true}
	for _, outcome := range outcomes {
		if outcome.failure != nil {
			writeError(w, InternalError(outcome.failure))
			return
		}
		response.Results = append(response.Results, outcome.result)
		response.AllSuccess = response.AllSuccess && outcome.result.Success
	}
	writeJSON(w, http.StatusOK, response)
}

// summaryJSON is the body of the summary endpoints.
type summaryJSON struct {
	Status      models.SummaryStatus `json:"status"`
	SummaryText *string              `json:"summary_text"`
	Error       *string              `json:"error"`
	CreatedAt   string               `json:"created_at"`
}

func summaryFromCache(cached cache.SummaryEntry) summaryJSON {
	return summaryJSON{cached.Status, cached.SummaryText, cached.ErrorMessage, chronoUTC(cached.CreatedAt)}
}

func summaryFromRow(row *models.EntrySummary) summaryJSON {
	return summaryJSON{row.Status, row.SummaryText, row.ErrorMessage, chronoUTC(row.CreatedAt)}
}

// summarizeEntryAPI queues a summary, or returns the one already known.
func (a *App) summarizeEntryAPI(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	if cached, ok := a.Summaries.Get(userID, entryID); ok {
		writeJSON(w, http.StatusOK, summaryFromCache(cached))
		return
	}
	stored, err := models.FindEntrySummary(r.Context(), a.DB, userID, entryID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if stored != nil {
		writeJSON(w, http.StatusOK, summaryFromRow(stored))
		return
	}
	found, ok := a.ownedEntry(w, r, userID, entryID)
	if !ok {
		return
	}
	if err := a.queueSummary(r.Context(), userID, found); err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, summaryJSON{Status: models.SummaryPending, CreatedAt: chronoUTC(a.Now())})
}

// queueSummary checks the Kagi setting, records the pending row and enqueues.
func (a *App) queueSummary(ctx context.Context, userID int64, found *entry.WithFeed) error {
	if found.Link == nil {
		return ValidationError("Entry has no link to summarize")
	}
	config, err := a.usableServices(ctx, userID)
	if err != nil {
		return err
	}
	if config.Kagi == nil || !config.Kagi.IsConfigured() {
		return ValidationError("Kagi is not configured")
	}
	if _, err := models.UpsertPendingSummary(ctx, a.DB, userID, found.ID); err != nil {
		return DatabaseError(err)
	}
	a.Summaries.SetPending(userID, found.ID)
	if err := summarize.Enqueue(ctx, a.SummaryQueue, summarize.Job{UserID: userID, EntryID: found.ID, EntryLink: *found.Link}); err != nil {
		if errors.Is(err, summarize.ErrQueueFull) {
			return InternalError(errors.New("Summary queue is full, please try again later"))
		}
		return InternalError(errors.New("Failed to queue summary job: " + err.Error()))
	}
	return nil
}

func (a *App) entrySummaryAPI(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	if cached, ok := a.Summaries.Get(userID, entryID); ok {
		writeJSON(w, http.StatusOK, summaryFromCache(cached))
		return
	}
	if _, ok := a.ownedEntry(w, r, userID, entryID); !ok {
		return
	}
	stored, err := models.FindEntrySummary(r.Context(), a.DB, userID, entryID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if stored == nil {
		writeError(w, NotFoundError("No summary found"))
		return
	}
	writeJSON(w, http.StatusOK, summaryFromRow(stored))
}

func (a *App) deleteEntrySummaryAPI(w http.ResponseWriter, r *http.Request) {
	userID, entryID, ok := a.beginEntryAPI(w, r)
	if !ok {
		return
	}
	if _, ok := a.ownedEntry(w, r, userID, entryID); !ok {
		return
	}
	if _, err := models.DeleteEntrySummary(r.Context(), a.DB, userID, entryID); err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	a.Summaries.Remove(userID, entryID)
	// The completed-summary badge may have dropped.
	a.Sidebar.Bust(userID)
	a.Events.EmitSummary(userID, entryID, nil)
	a.Events.EmitSidebar(userID)
	writeJSON(w, http.StatusOK, map[string]bool{"success": true})
}
