package server

import (
	"context"
	"log/slog"
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/readability"
	"github.com/henry40408/rdrs/internal/sanitize"
	"github.com/henry40408/rdrs/internal/save"
	"github.com/henry40408/rdrs/internal/summarize"
	"github.com/henry40408/rdrs/internal/views"
)

// entryActionRoutes are the form actions of the reading pane and entry rows.
func (a *App) entryActionRoutes() []Route {
	return []Route{
		{"POST", "/entries/{id}/star", a.starEntryForm},
		{"POST", "/entries/{id}/unstar", a.unstarEntryForm},
		{"POST", "/entries/{id}/read", a.readEntryForm},
		{"POST", "/entries/{id}/unread", a.unreadEntryForm},
		{"POST", "/entries/{id}/summarize", a.summarizeEntryForm},
		{"POST", "/entries/{id}/summarize/cancel", a.summarizeCancelForm},
		{"POST", "/entries/{id}/fetch-full-content", a.fetchFullContentForm},
		{"POST", "/entries/{id}/save", a.saveEntryForm},
	}
}

// toast is a message shown as a <template data-flash> block, or as a flash
// cookie when the browser sent a plain form post.
type toast struct {
	level flash.Level
	text  string
}

func (t *toast) payload() *views.Flash {
	if t == nil {
		return nil
	}
	return &views.Flash{Level: string(t.level), Message: t.text}
}

func (t toast) message() flash.Message {
	switch t.level {
	case flash.Success:
		return flash.NewSuccess(t.text)
	case flash.Error:
		return flash.NewError(t.text)
	case flash.Warning:
		return flash.NewWarning(t.text)
	}
	return flash.NewInfo(t.text)
}

// respondToAction answers an entry action: the swap fragment for a fetch(),
// a 303 for a scriptless form post, which carries the toast as a flash cookie.
func respondToAction(w http.ResponseWriter, r *http.Request, entryID int64, fragment string, message *toast) {
	if !isDocumentNavigation(r) {
		writeHTML(w, http.StatusOK, fragment)
		return
	}
	location := actionDocumentRedirect(r, entryID)
	if message == nil {
		redirect(w, location)
		return
	}
	flash.Redirect(w, location, message.message())
}

// entryActionRequest is an authenticated action on one of the user's entries.
type entryActionRequest struct {
	principal *auth.Principal
	entry     *entry.WithFeed
}

// beginEntryAction authenticates, reads {id} and loads the entry; it reports
// false after answering.
func (a *App) beginEntryAction(w http.ResponseWriter, r *http.Request) (entryActionRequest, bool) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return entryActionRequest{}, false
	}
	id, ok := pathID(w, r)
	if !ok {
		return entryActionRequest{}, false
	}
	found, err := entry.FindByIDForUser(r.Context(), a.DB, principal.User.ID, id)
	if err != nil {
		writeError(w, DatabaseError(err))
		return entryActionRequest{}, false
	}
	if found == nil {
		writeError(w, ErrEntryNotFound)
		return entryActionRequest{}, false
	}
	return entryActionRequest{principal: principal, entry: found}, true
}

func (a *App) starEntryForm(w http.ResponseWriter, r *http.Request)   { a.setStarred(w, r, true) }
func (a *App) unstarEntryForm(w http.ResponseWriter, r *http.Request) { a.setStarred(w, r, false) }
func (a *App) readEntryForm(w http.ResponseWriter, r *http.Request)   { a.setRead(w, r, true) }
func (a *App) unreadEntryForm(w http.ResponseWriter, r *http.Request) { a.setRead(w, r, false) }

// setStarred renders the new state at once and writes it in a detached task.
// It is idempotent. Starring does not change unread counts, so the sidebar
// stays as it is.
func (a *App) setStarred(w http.ResponseWriter, r *http.Request, starred bool) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	found, userID := request.entry, request.principal.User.ID
	status, err := a.summaryStatusOf(r.Context(), userID, found.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	changed := (found.StarredAt != nil) != starred
	if starred && found.StarredAt == nil {
		now := a.Now()
		found.StarredAt = &now
	}
	if !starred {
		found.StarredAt = nil
	}
	if changed {
		a.detach(func() {
			if _, err := entry.SetStarredForUser(context.Background(), a.DB, userID, found.ID, starred); err != nil {
				slog.Warn("async set_starred failed", "event", "entry.set_starred_failed", "entry_id", found.ID, "error", err)
			}
		})
	}
	row := a.rowViewFrom(found, status)
	state := views.RowState{ID: row.ID, IsRead: row.IsRead, IsStarred: row.IsStarred}
	respondToAction(w, r, found.ID, views.EntryActionsMulti(views.EntryActions{
		Row: state, CSRF: request.principal.CSRFToken, PaneStar: &state,
	}), nil)
}

// setRead is setStarred for the read state. Only an actual change to unread
// toasts.
func (a *App) setRead(w http.ResponseWriter, r *http.Request, read bool) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	found, userID := request.entry, request.principal.User.ID
	status, err := a.summaryStatusOf(r.Context(), userID, found.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	changed := (found.ReadAt != nil) != read
	if read && found.ReadAt == nil {
		now := a.Now()
		found.ReadAt = &now
	}
	if !read {
		found.ReadAt = nil
	}
	var message *toast
	if !read && changed {
		message = &toast{flash.Success, "Marked as unread."}
	}
	if changed {
		a.detach(func() {
			if _, err := entry.SetReadForUser(context.Background(), a.DB, userID, found.ID, read); err != nil {
				slog.Warn("async set_read failed", "event", "entry.set_read_failed", "entry_id", found.ID, "error", err)
			}
			a.refreshSidebarAfterWrite(userID)
		})
		a.Sidebar.Bust(userID)
		a.Events.EmitSidebar(userID)
	}
	row := a.rowViewFrom(found, status)
	state := views.RowState{ID: row.ID, IsRead: row.IsRead, IsStarred: row.IsStarred}
	respondToAction(w, r, found.ID, views.EntryActionsMulti(views.EntryActions{
		Row: state, CSRF: request.principal.CSRFToken, Flash: message.payload(),
	}), message)
}

// summarizeEntryForm is POST /entries/{id}/summarize: queue a summary job.
// The worker validates the Kagi configuration.
func (a *App) summarizeEntryForm(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	found, userID := request.entry, request.principal.User.ID
	// The database record goes first, so the state is always consistent.
	if _, err := models.UpsertPendingSummary(r.Context(), a.DB, userID, found.ID); err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	// An empty link is a sentinel; the worker reports the error.
	link := ""
	if found.Link != nil {
		link = sanitize.StripTrackingParams(*found.Link)
	}
	// Before the enqueue, or the worker could finish first.
	a.Summaries.SetPending(userID, found.ID)
	// Best effort: the database record is already pending.
	_ = summarize.Enqueue(r.Context(), a.SummaryQueue, summarize.Job{UserID: userID, EntryID: found.ID, EntryLink: link})
	pending := models.SummaryPending
	a.Events.EmitSummary(userID, found.ID, &pending)
	// No toast: the pending state in the pane is the feedback.
	respondToAction(w, r, found.ID, views.SummarizePending(found.ID, request.principal.CSRFToken), nil)
}

// summarizeCancelForm is POST /entries/{id}/summarize/cancel: stop an
// in-flight summary or clear a failed one.
func (a *App) summarizeCancelForm(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	userID, entryID := request.principal.User.ID, request.entry.ID
	if _, err := models.DeleteEntrySummary(r.Context(), a.DB, userID, entryID); err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	a.Cancels.Cancel(userID, entryID)
	a.Summaries.Remove(userID, entryID)
	a.Sidebar.Bust(userID)
	a.Events.EmitSummary(userID, entryID, nil)
	a.Events.EmitSidebar(userID)
	// No toast: the empty pane is the confirmation.
	respondToAction(w, r, entryID, views.SummaryCleared(), nil)
}

// fetchFullContentForm is POST /entries/{id}/fetch-full-content: fetch, store
// and show the source article.
func (a *App) fetchFullContentForm(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	found, userID := request.entry, request.principal.User.ID
	if found.Link == nil {
		writeError(w, ValidationError("Entry has no link"))
		return
	}
	settings, err := a.loadPaneSettings(r.Context(), userID)
	if err != nil {
		writeError(w, err)
		return
	}
	message, err := a.storeFullContent(r.Context(), userID, found)
	if err != nil {
		writeError(w, err)
		return
	}
	a.respondWithPane(w, r, paneAction{request: request, settings: settings, message: message})
}

// storeFullContent downloads the article. A fetch failure is a toast; only a
// failed write is an error.
func (a *App) storeFullContent(ctx context.Context, userID int64, found *entry.WithFeed) (toast, error) {
	extracted, err := readability.FetchAndExtract(ctx, readability.Params{
		URL: *found.Link, UserAgent: a.Config.UserAgent, Fetcher: a.Fetcher, Retry: fetch.DefaultRetry(),
	})
	if err != nil {
		return toast{flash.Error, "Failed to fetch full content: " + err.Error()}, nil
	}
	// Stored raw; the pane sanitises it like any other body.
	if err := entry.SetFullContentForUser(ctx, a.DB, userID, found.ID, extracted.Content); err != nil {
		return toast{}, DatabaseError(err)
	}
	found.FullContent = &extracted.Content
	return toast{flash.Success, "Fetched full content."}, nil
}

// paneAction is a reading pane answer that carries a toast.
type paneAction struct {
	request  entryActionRequest
	settings paneSettings
	message  toast
}

func (a *App) respondWithPane(w http.ResponseWriter, r *http.Request, action paneAction) {
	principal, found := action.request.principal, action.request.entry
	built, err := a.buildReadingPane(r.Context(), paneRequest{
		userID: principal.User.ID, entry: found, settings: action.settings, view: fullView, purpose: readerRender,
	})
	if err != nil {
		writeError(w, err)
		return
	}
	respondToAction(w, r, found.ID, views.PaneWithFlash(built.pane, principal.CSRFToken, action.message.payload()), &action.message)
}

// saveEntryForm is POST /entries/{id}/save: send the entry to every
// configured save service.
func (a *App) saveEntryForm(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginEntryAction(w, r)
	if !ok {
		return
	}
	found, userID := request.entry, request.principal.User.ID
	config, err := a.usableServices(r.Context(), userID)
	if err != nil {
		writeError(w, err)
		return
	}
	if found.Link == nil {
		writeError(w, ValidationError("Entry has no link to save"))
		return
	}
	if !config.HasAnyService() {
		writeError(w, ValidationError("No save services configured"))
		return
	}
	results := a.saveToServices(r.Context(), config, bookmarkOf(found))
	settings, err := a.loadPaneSettings(r.Context(), userID)
	if err != nil {
		writeError(w, err)
		return
	}
	// Saving changes nothing visible, so the toast is the only feedback.
	a.respondWithPane(w, r, paneAction{request: request, settings: settings, message: saveToast(results)})
}

// usableServices reads the user's service settings; an undecryptable value is
// an error, not "not configured".
func (a *App) usableServices(ctx context.Context, userID int64) (save.ServicesConfig, error) {
	key, _ := a.Config.ServiceTokenKey()
	stored, err := models.SaveServicesOf(ctx, a.DB, userID, key)
	if err != nil {
		return save.ServicesConfig{}, DatabaseError(err)
	}
	config, err := save.UsableConfigOf(stored)
	if err != nil {
		return save.ServicesConfig{}, appError(err)
	}
	return config, nil
}

func bookmarkOf(found *entry.WithFeed) save.BookmarkData {
	return save.BookmarkData{URL: sanitize.StripTrackingParams(*found.Link), Title: found.Title, Description: found.Summary}
}

// saveOutcome is one service's result; failure is set when the call itself
// failed rather than the service refusing.
type saveOutcome struct {
	service string
	result  *save.Result
	failure error
}

func (a *App) saveToServices(ctx context.Context, config save.ServicesConfig, bookmark save.BookmarkData) []saveOutcome {
	var outcomes []saveOutcome
	if config.Linkding != nil && config.Linkding.IsConfigured() {
		result, err := save.SaveToLinkding(ctx, save.LinkdingRequest{
			Config: *config.Linkding, Bookmark: bookmark, Client: a.externalClient, Retry: fetch.DefaultRetry(),
		})
		outcomes = append(outcomes, saveOutcome{service: "Linkding", result: result, failure: err})
	}
	return outcomes
}

// saveToast words the outcome per service, as the Rust server did.
func saveToast(outcomes []saveOutcome) toast {
	var succeeded, failed []string
	for _, outcome := range outcomes {
		switch {
		case outcome.failure != nil:
			// The cause stays out of the page, like the Rust Internal error.
			failed = append(failed, outcome.service+": Internal server error")
		case outcome.result.Success:
			succeeded = append(succeeded, outcome.service)
		default:
			failed = append(failed, outcome.service+": "+outcome.result.Message)
		}
	}
	switch {
	case len(failed) == 0:
		return toast{flash.Success, "Saved to " + strings.Join(succeeded, ", ") + "."}
	case len(succeeded) == 0:
		return toast{flash.Error, "Save failed — " + strings.Join(failed, "; ")}
	}
	return toast{flash.Warning, "Saved to " + strings.Join(succeeded, ", ") + ". Failed: " + strings.Join(failed, "; ")}
}
