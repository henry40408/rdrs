package server

import (
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// markReadRoutes are the "Mark N matching as read" forms of the list pages.
func (a *App) markReadRoutes() []Route {
	return []Route{
		{"POST", "/mark-read", a.markReadUnscoped("/")},
		{"POST", "/entries/mark-read", a.markReadUnscoped("/entries")},
		{"POST", "/entries/today/mark-read", a.todayMarkRead},
		{"POST", "/entries/today/mark-matching", a.todayMarkMatching},
		{"POST", "/categories/{id}/entries/mark-read", a.scopedMarkRead("categories", "category_id")},
		{"POST", "/feeds/{id}/entries/mark-read", a.scopedMarkRead("feeds", "feed_id")},
	}
}

// formValue is a form field, nil when absent.
func formValue(form url.Values, name string) *string {
	if !form.Has(name) {
		return nil
	}
	value := form.Get(name)
	return &value
}

// scopedRedirect is basePath plus the URL-encoded q and status, each omitted
// when absent.
func scopedRedirect(basePath string, search, status *string) string {
	var params []string
	if search != nil {
		params = append(params, "q="+formEscape(*search))
	}
	if status != nil {
		params = append(params, "status="+formEscape(*status))
	}
	if len(params) == 0 {
		return basePath
	}
	query := params[0]
	for _, param := range params[1:] {
		query += "&" + param
	}
	return basePath + "?" + query
}

// markReadRequest is a mark-read form: the scope to mark and the page to
// return to.
type markReadRequest struct {
	userID int64
	// scope is what the page lists, before the search and snapshot narrow it.
	scope    entry.Filter
	q        *string
	status   *string
	snapshot *string
	basePath string
}

func (a *App) markReadUnscoped(basePath string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		principal, ok := a.requirePage(w, r)
		if !ok {
			return
		}
		form, ok := parseForm(w, r)
		if !ok {
			return
		}
		a.markReadScoped(w, r, markReadRequest{
			userID: principal.User.ID, q: formValue(form, "q"), snapshot: formValue(form, "snapshot"), basePath: basePath,
		})
	}
}

// scopedMarkRead is the form of a feed or category page; idField names the
// filter field the {id} fills.
func (a *App) scopedMarkRead(kind, idField string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		principal, ok := a.requirePage(w, r)
		if !ok {
			return
		}
		id, ok := pathID(w, r)
		if !ok {
			return
		}
		form, ok := parseForm(w, r)
		if !ok {
			return
		}
		scope := entry.Filter{}
		if idField == "feed_id" {
			scope.FeedID = &id
		} else {
			scope.CategoryID = &id
		}
		a.markReadScoped(w, r, markReadRequest{
			userID: principal.User.ID, scope: scope, q: formValue(form, "q"), status: formValue(form, "status"),
			snapshot: formValue(form, "snapshot"), basePath: "/" + kind + "/" + strconv.FormatInt(id, 10) + "/entries",
		})
	}
}

const outOfDate = "The page is out of date. Reload and try again."

// markReadScoped marks the entries a search matches within the scope. The
// guards matter: a blank q would match, and mark read, the whole scope, and an
// absent snapshot would mean "no limit".
func (a *App) markReadScoped(w http.ResponseWriter, r *http.Request, request markReadRequest) {
	parsed := parseScopedSearch(request.q)
	if parsed.raw == nil {
		flash.Redirect(w, scopedRedirect(request.basePath, nil, request.status), flash.NewInfo("No search term — nothing marked."))
		return
	}
	search := strings.TrimSpace(*parsed.raw)
	back := scopedRedirect(request.basePath, &search, request.status)
	// An unparseable query matches nothing to mark; say why instead.
	if parsed.failure != nil {
		flash.Redirect(w, back, flash.NewError(*parsed.failure))
		return
	}
	storedBefore, err := entry.ParseSnapshot(request.snapshot)
	if err != nil || storedBefore == nil {
		flash.Redirect(w, back, flash.NewError(outOfDate))
		return
	}
	filter := request.scope
	filter.Query, filter.StoredBefore = parsed.query, storedBefore
	affected, err := entry.MarkReadByFilter(r.Context(), a.DB, request.userID, &filter)
	if err != nil {
		slog.Warn("mark_read_by_filter failed", "event", "entry.mark_read_scoped_failed", "user_id", request.userID, "error", err)
		flash.Redirect(w, back, flash.NewError("Failed to mark matching entries as read. Please try again."))
		return
	}
	a.refreshAfterBulkRead(request.userID, affected)
	flash.Redirect(w, back, flash.NewSuccess("Marked "+strconv.FormatInt(affected, 10)+" matching entries as read."))
}

func (a *App) refreshAfterBulkRead(userID, affected int64) {
	if affected == 0 {
		return
	}
	a.Sidebar.Bust(userID)
	a.Events.EmitSidebar(userID)
}

// todaySnapshot reads the snapshot of today's forms. Absent or malformed
// would mean "no limit", which the snapshot exists to prevent.
func todaySnapshot(form url.Values) *time.Time {
	stored, err := entry.ParseSnapshot(formValue(form, "snapshot"))
	if err != nil {
		return nil
	}
	return stored
}

const todayPath = "/entries/today"

// todayMarkRead is POST /entries/today/mark-read: today's unread entries,
// except those stored after the page was rendered.
func (a *App) todayMarkRead(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	storedBefore := todaySnapshot(form)
	if storedBefore == nil {
		flash.Redirect(w, todayPath, flash.NewError(outOfDate))
		return
	}
	userID := principal.User.ID
	location := a.userLocation(r.Context(), userID)
	filter := entry.UnreadFilterOn(entry.DayOfSnapshot(*storedBefore, location), location)
	filter.StoredBefore = storedBefore
	affected, err := entry.MarkReadByFilter(r.Context(), a.DB, userID, &filter)
	if err != nil {
		slog.Warn("mark_read_by_filter failed", "event", "entry.mark_today_read_failed", "user_id", userID, "error", err)
		flash.Redirect(w, todayPath, flash.NewError("Failed to mark today's entries as read."))
		return
	}
	a.refreshAfterBulkRead(userID, affected)
	noun := "entries"
	if affected == 1 {
		noun = "entry"
	}
	flash.Redirect(w, todayPath, flash.NewSuccess("Marked "+strconv.FormatInt(affected, 10)+" "+noun+" as read."))
}

// todayMarkMatching is POST /entries/today/mark-matching: today's unread
// entries a search matches. A bad snapshot leaves the scope unbounded, but
// markReadScoped refuses it before anything is marked.
func (a *App) todayMarkMatching(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	var scope entry.Filter
	if storedBefore := todaySnapshot(form); storedBefore != nil {
		location := a.userLocation(r.Context(), userID)
		scope = entry.UnreadFilterOn(entry.DayOfSnapshot(*storedBefore, location), location)
	}
	a.markReadScoped(w, r, markReadRequest{
		userID: userID, scope: scope, q: formValue(form, "q"), snapshot: formValue(form, "snapshot"), basePath: todayPath,
	})
}
