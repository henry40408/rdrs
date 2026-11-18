package server

import (
	"context"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/views"
)

// entryFragmentRoutes are the reading pane fragments the swap helper fetches.
func (a *App) entryFragmentRoutes() []Route {
	return []Route{
		{"GET", "/entries/{id}/fragment", a.entryFragment},
		{"GET", "/entries/{id}/summary/fragment", a.summaryFragment},
	}
}

// isDocumentNavigation is a top-level navigation rather than a swap-helper
// fetch(). Fragments render blank as documents, so such a request must get a
// page or a redirect instead.
func isDocumentNavigation(r *http.Request) bool {
	return r.Header.Get("Sec-Fetch-Dest") == "document"
}

// isSpeculativeLoad is a prefetch or prerender, which must not mark the entry
// read. Sec-Purpose may be "prefetch;prerender" (hence the substring);
// Purpose is legacy Chromium and X-Moz is Firefox.
func isSpeculativeLoad(r *http.Request) bool {
	return strings.Contains(strings.ToLower(r.Header.Get("Sec-Purpose")), "prefetch") ||
		strings.EqualFold(r.Header.Get("Purpose"), "prefetch") ||
		strings.EqualFold(r.Header.Get("X-Moz"), "prefetch")
}

// denyStorage keeps a speculative response out of the prefetch cache, or the
// real click would be served from it and never mark the entry read.
func denyStorage(w http.ResponseWriter) { w.Header().Set("Cache-Control", "no-store") }

// isScopedEntriesPath is /{kind}/{id}/entries with a purely numeric id.
func isScopedEntriesPath(path, kind string) bool {
	id, ok := strings.CutSuffix(strings.TrimPrefix(path, "/"+kind+"/"), "/entries")
	if !ok || !strings.HasPrefix(path, "/"+kind+"/") || id == "" {
		return false
	}
	for i := 0; i < len(id); i++ {
		if id[i] < '0' || id[i] > '9' {
			return false
		}
	}
	return true
}

// isEntryListPath is a list route that honours ?entry=; /search does not
// deep-link an entry.
func isEntryListPath(path string) bool {
	switch path {
	case "/", "/entries", "/entries/read", "/entries/starred", "/entries/summarized", "/entries/today":
		return true
	}
	return isScopedEntriesPath(path, "categories") || isScopedEntriesPath(path, "feeds")
}

// referringEntryList is the entry list page from Referer, if any. Only its
// path and query are used, so redirects built from it stay same-origin.
func referringEntryList(r *http.Request) *url.URL {
	parsed, err := url.Parse(r.Header.Get("Referer"))
	if err != nil || !parsed.IsAbs() || !isEntryListPath(parsed.Path) {
		return nil
	}
	return parsed
}

// formEscape encodes like form_urlencoded: space is "+", and only
// alphanumerics and "*-._" stay.
func formEscape(value string) string {
	escaped := url.QueryEscape(value)
	escaped = strings.ReplaceAll(escaped, "%2A", "*")
	return strings.ReplaceAll(escaped, "~", "%7E")
}

func pathAndQuery(u *url.URL) string {
	if u.RawQuery == "" {
		return u.EscapedPath()
	}
	return u.EscapedPath() + "?" + u.RawQuery
}

// fragmentDocumentRedirect is where a document navigation to a fragment goes:
// the referring list with its filters plus ?entry=, else All Entries.
func fragmentDocumentRedirect(r *http.Request, entryID int64) string {
	id := strconv.FormatInt(entryID, 10)
	list := referringEntryList(r)
	if list == nil {
		return "/entries?entry=" + id
	}
	var pairs []string
	for _, pair := range strings.Split(list.RawQuery, "&") {
		if pair == "" {
			continue
		}
		key, value, _ := strings.Cut(pair, "=")
		decodedKey, _ := url.QueryUnescape(key)
		if decodedKey == "entry" {
			continue
		}
		decodedValue, _ := url.QueryUnescape(value)
		pairs = append(pairs, formEscape(decodedKey)+"="+formEscape(decodedValue))
	}
	pairs = append(pairs, "entry="+id)
	return list.EscapedPath() + "?" + strings.Join(pairs, "&")
}

// actionDocumentRedirect is where a scriptless entry-action POST goes. The
// referrer is reused verbatim so a row action does not open the pane.
func actionDocumentRedirect(r *http.Request, entryID int64) string {
	if list := referringEntryList(r); list != nil {
		return pathAndQuery(list)
	}
	return "/entries?entry=" + strconv.FormatInt(entryID, 10)
}

// refreshSidebarAfterWrite busts and notifies again once a detached write has
// landed: counts cached in between would keep the old state for the whole TTL.
func (a *App) refreshSidebarAfterWrite(userID int64) {
	a.Sidebar.Bust(userID)
	a.Events.EmitSidebar(userID)
}

// dispatchMarkReadOnOpen marks the entry read off the critical path and
// refreshes the sidebar counts over SSE.
func (a *App) dispatchMarkReadOnOpen(userID, entryID int64) {
	a.detach(func() {
		if err := entry.MarkAsReadQuietly(context.Background(), a.DB, entryID); err != nil {
			slog.Warn("async mark_as_read_quietly failed", "event", "entry.mark_read_failed", "entry_id", entryID, "error", err)
		}
		a.refreshSidebarAfterWrite(userID)
	})
	a.Sidebar.Bust(userID)
	a.Events.EmitSidebar(userID)
}

// fragmentQuery is the query string of GET /entries/{id}/fragment.
type fragmentQuery struct {
	view contentView
	// prefetch is ?offline=1: an offline-sync mirror that must not mark
	// anything read (fetch() cannot set Sec-Purpose).
	prefetch bool
}

func parseFragmentQuery(w http.ResponseWriter, r *http.Request) (fragmentQuery, bool) {
	reader := queryReader{values: r.URL.Query()}
	viewName, offline := reader.text("view"), reader.small("offline")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return fragmentQuery{}, false
	}
	query := fragmentQuery{prefetch: offline != nil && *offline == 1}
	if viewName != nil && *viewName == "original" {
		query.view = originalView
	}
	return query, true
}

// entryFragment is GET /entries/{id}/fragment: open an entry, which marks it
// read; 404 when it is not the user's.
func (a *App) entryFragment(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	query, ok := parseFragmentQuery(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	// Loaded before the document-navigation branch: a scriptless open marks
	// read too.
	found, err := entry.FindByIDForUser(r.Context(), a.DB, userID, id)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	speculative := isSpeculativeLoad(r) || query.prefetch
	markRead := !speculative && found != nil && found.ReadAt == nil
	// Document navigations (no-JS, new tab, refresh) would see a blank page,
	// so redirect to the list with the pane open. A missing entry redirects
	// too: lists ignore a bad ?entry=.
	if isDocumentNavigation(r) {
		if markRead {
			a.dispatchMarkReadOnOpen(userID, id)
		}
		if speculative {
			denyStorage(w)
		}
		redirect(w, fragmentDocumentRedirect(r, id))
		return
	}
	if found == nil {
		writeError(w, ErrEntryNotFound)
		return
	}
	a.writeOpenedEntry(w, r, principal, openedEntry{entry: found, query: query, speculative: speculative, markRead: markRead})
}

type openedEntry struct {
	entry       *entry.WithFeed
	query       fragmentQuery
	speculative bool
	markRead    bool
}

func (a *App) writeOpenedEntry(w http.ResponseWriter, r *http.Request, principal *auth.Principal, opened openedEntry) {
	userID, found := principal.User.ID, opened.entry
	if opened.markRead {
		// Optimistic; tied to markRead so speculative loads render true state.
		now := a.Now()
		found.ReadAt = &now
	}
	settings, err := a.loadPaneSettings(r.Context(), userID)
	if err != nil {
		writeError(w, err)
		return
	}
	purpose := readerRender
	if opened.speculative {
		purpose = speculativeRender
	}
	built, err := a.buildReadingPane(r.Context(), paneRequest{
		userID: userID, entry: found, settings: settings, view: opened.query.view, purpose: purpose,
	})
	if err != nil {
		writeError(w, err)
		return
	}
	row := a.rowViewFrom(found, built.summaryStatus)
	// After the render only to stay off the critical path.
	if opened.markRead {
		a.dispatchMarkReadOnOpen(userID, found.ID)
	}
	if opened.speculative {
		denyStorage(w)
	}
	state := views.RowState{ID: row.ID, IsRead: row.IsRead, IsStarred: row.IsStarred}
	writeHTML(w, http.StatusOK, views.OpenEntryMulti(built.pane, state, principal.CSRFToken))
}

// summaryFragment is GET /entries/{id}/summary/fragment, refreshed by the SSE
// summary event. It does not mark the entry read.
func (a *App) summaryFragment(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	found, err := entry.FindByIDForUser(r.Context(), a.DB, principal.User.ID, id)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if found == nil {
		writeError(w, ErrEntryNotFound)
		return
	}
	// The save and Kagi flags and the open pixel do not matter to the summary
	// container.
	built, err := a.buildReadingPane(r.Context(), paneRequest{userID: principal.User.ID, entry: found, view: fullView, purpose: readerRender})
	if err != nil {
		writeError(w, err)
		return
	}
	writeHTML(w, http.StatusOK, views.SummaryFragment(built.pane, principal.CSRFToken))
}

// summaryStatusOf is the entry's summary status, "" when it has none. A failing
// query shows the row without a badge.
func (a *App) summaryStatusOf(ctx context.Context, userID, entryID int64) (models.SummaryStatus, error) {
	statuses, err := models.SummaryStatusesFor(ctx, a.DB, userID, []int64{entryID})
	return statuses[entryID], err
}
