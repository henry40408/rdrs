package server

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/timezone"
	"github.com/henry40408/rdrs/internal/views"
)

const readingListScope = "user/-/state/com.google/reading-list"

// entryPageRoutes are the list pages and the entry redirect.
func (a *App) entryPageRoutes() []Route {
	return []Route{
		{"GET", "/", a.unreadPage},
		{"GET", "/entries", a.allEntriesPage},
		{"GET", "/entries/read", a.readEntriesPage},
		{"GET", "/entries/starred", a.starredEntriesPage},
		{"GET", "/entries/summarized", a.summarizedEntriesPage},
		{"GET", "/entries/today", a.todayEntriesPage},
		{"GET", "/entries/offline", a.offlineEntriesPage},
		{"GET", "/entries/{id}", a.entryRedirect},
		{"GET", "/categories/{id}/entries", a.categoryEntriesPage},
		{"GET", "/feeds/{id}/entries", a.feedEntriesPage},
	}
}

// listSpec is everything that tells one list page from another.
type listSpec struct {
	title  string
	layout views.EntriesLayout
	// filter is the scope plus tab or status; the search is added to it.
	filter entry.Filter
	// scope is the filter "Mark N matching as read" narrows; matching is
	// computed only when layout.MarkMatching is set.
	scope entry.Filter
	sort  entry.SortOrder
	// loadMoreStatus is echoed into Load More's next-page form.
	loadMoreStatus *string
	// alwaysCursor parses ?after= on full pages too (feed and category pages).
	alwaysCursor bool
	// paneSwap answers ?pane=1 with PaneSwap (category pages).
	paneSwap bool
	// detectOnboarding turns an empty, unsearched list into the welcome page
	// when the user has no feeds at all.
	detectOnboarding bool
	found            scopedSearch
}

// serveList answers every list page. The checks follow the Rust order: Load
// More, refresh, pane swap, full page.
func (a *App) serveList(w http.ResponseWriter, r *http.Request, principal *auth.Principal, query entriesQuery, spec listSpec) {
	ctx := r.Context()
	userID := principal.User.ID
	request := pageRequest{
		userID: userID, filter: spec.filter, found: spec.found, sort: spec.sort,
		pageSize: a.entriesPageSize(ctx, userID),
	}
	if query.After != nil && (spec.alwaysCursor || query.isFragment()) {
		request.cursor, _ = entry.ParseCursor(*query.After)
	}
	rows, next := a.buildSearchedPage(ctx, request)
	csrf := principal.CSRFToken
	if query.loadsMore() {
		writeHTML(w, http.StatusOK, views.LoadMoreFragment(views.LoadMore{
			Entries: rows, NextCursor: next, CSRF: csrf, Path: spec.layout.Path,
			Snapshot: query.Snapshot, Status: spec.loadMoreStatus, Query: spec.found.raw,
		}))
		return
	}
	layout := a.finishListLayout(ctx, userID, spec, len(rows) == 0)
	list := views.EntriesList{Title: spec.title, Entries: rows, NextCursor: next, Layout: &layout, CSRF: csrf}
	switch {
	case query.isFragment():
		writeHTML(w, http.StatusOK, views.RefreshSwap(list))
	case spec.paneSwap && query.isPane():
		writeHTML(w, http.StatusOK, views.PaneSwap(list))
	default:
		a.writeListPage(w, r, principal, list, query)
	}
}

// finishListLayout adds what depends on the fetched rows and the search.
func (a *App) finishListLayout(ctx context.Context, userID int64, spec listSpec, empty bool) views.EntriesLayout {
	layout := spec.layout
	layout.Search, layout.SearchError = spec.found.raw, spec.found.failure
	if layout.MarkMatching != nil && layout.SnapshotAt != "" {
		layout.MatchingCount = a.countMatchingUnread(ctx, userID, spec.scope, spec.found, layout.SnapshotAt)
	}
	if spec.detectOnboarding && empty && spec.found.raw == nil {
		layout.Onboarding = a.hasNoFeeds(ctx, userID)
		if layout.Onboarding {
			layout.SearchAction = nil
		}
	}
	return layout
}

// hasNoFeeds treats a failing count as "has feeds", so a database error
// never shows the welcome page.
func (a *App) hasNoFeeds(ctx context.Context, userID int64) bool {
	count, err := models.CountFeedsByUser(ctx, a.DB, userID)
	return err == nil && count == 0
}

func (a *App) writeListPage(w http.ResponseWriter, r *http.Request, principal *auth.Principal, list views.EntriesList, query entriesQuery) {
	pane := a.maybeBuildReadingPane(r.Context(), principal.User.ID, query.Entry)
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusOK, views.Entries(views.EntriesPage{
		Layout: &layout, Title: list.Title, List: list, ReadingPane: pane,
	}))
}

// maybeBuildReadingPane opens the ?entry= deep link. It is best effort and,
// unlike the fragment route, does not mark the entry read: a shared link
// must not change state.
func (a *App) maybeBuildReadingPane(ctx context.Context, userID int64, id *int64) *views.ReadingPane {
	if id == nil {
		return nil
	}
	found, err := entry.FindByIDForUser(ctx, a.DB, userID, *id)
	if err != nil || found == nil {
		return nil
	}
	settings, err := a.loadPaneSettings(ctx, userID)
	if err != nil {
		return nil
	}
	built, err := a.buildReadingPane(ctx, paneRequest{userID: userID, entry: found, settings: settings, view: fullView, purpose: readerRender})
	if err != nil {
		return nil
	}
	return built.pane
}

// listRequest is the parsed start of a list handler.
type listRequest struct {
	principal *auth.Principal
	query     entriesQuery
}

// beginList authenticates and parses the query; it reports false after
// answering.
func (a *App) beginList(w http.ResponseWriter, r *http.Request) (listRequest, bool) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return listRequest{}, false
	}
	query, ok := parseEntriesQuery(w, r)
	if !ok {
		return listRequest{}, false
	}
	return listRequest{principal: principal, query: query}, true
}

// --- GET / ---

func (a *App) unreadPage(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	query := request.query
	snapshotAt := a.snapshotNow()
	scope := readingListScope
	search, matching, empty := parseScopedSearch(query.Q), "/mark-read", "/"
	a.serveList(w, r, request.principal, query, listSpec{
		title: "Unread", found: search, detectOnboarding: true,
		filter: entry.Filter{UnreadOnly: true, ReadAfter: query.Snapshot},
		scope:  entry.Filter{},
		sort:   entry.SortPublishedAt,
		layout: views.EntriesLayout{
			Active: "unread", EmptyTitle: "All caught up",
			EmptyDetail: "You've read every unread entry — new items land here as your feeds refresh.",
			Path:        "/", MarkAsReadScope: &scope, SnapshotAt: snapshotAt,
			SearchAction: &empty, MarkMatching: &matching, ShowMarkAbove: search.raw == nil,
		},
	})
}

// --- tab pages ---

// tabSpec is a tab page of /entries: the copy, filter and actions.
type tabSpec struct {
	path, title, active, emptyTitle, emptyDetail string
	filter                                       entry.Filter
	sort                                         entry.SortOrder
	markAll, markMatching                        string
	showTabBar                                   bool
}

func tabs() map[string]tabSpec {
	return map[string]tabSpec{
		"all": {
			path: "/entries", title: "Entries", active: "all", emptyTitle: "Nothing to read yet",
			emptyDetail: "Subscribe to a few feeds and their entries will gather here.",
			sort:        entry.SortPublishedAt, markMatching: "/entries/mark-read", showTabBar: true,
		},
		"read": {
			path: "/entries/read", title: "Read Entries", active: "read", emptyTitle: "No read entries yet",
			emptyDetail: "Entries stay here once you've opened and read them.",
			filter:      entry.Filter{ReadOnly: true}, sort: entry.SortReadAt, showTabBar: true,
		},
		"starred": {
			path: "/entries/starred", title: "Starred Entries", active: "starred", emptyTitle: "No starred entries",
			emptyDetail: "Star an entry and it'll wait for you here.",
			filter:      entry.Filter{StarredOnly: true}, sort: entry.SortStarredAt, showTabBar: true,
		},
		"summarized": {
			path: "/entries/summarized", title: "Summarized Entries", active: "summarized",
			emptyTitle: "No summaries yet", emptyDetail: "Entries you summarize are collected on this page.",
			filter: entry.Filter{HasSummary: ptr(true)}, sort: entry.SortSummarizedAt, showTabBar: true,
		},
	}
}

func ptr[T any](value T) *T { return &value }

func (a *App) allEntriesPage(w http.ResponseWriter, r *http.Request) {
	a.tabPage(w, r, tabs()["all"])
}

func (a *App) readEntriesPage(w http.ResponseWriter, r *http.Request) {
	a.tabPage(w, r, tabs()["read"])
}

func (a *App) starredEntriesPage(w http.ResponseWriter, r *http.Request) {
	a.tabPage(w, r, tabs()["starred"])
}

func (a *App) summarizedEntriesPage(w http.ResponseWriter, r *http.Request) {
	a.tabPage(w, r, tabs()["summarized"])
}

// todayEntriesPage is GET /entries/today.
func (a *App) todayEntriesPage(w http.ResponseWriter, r *http.Request) {
	spec := tabSpec{
		path: "/entries/today", title: "Today", active: "today", emptyTitle: "Nothing new today",
		emptyDetail: "Unread entries published today show up here.", sort: entry.SortPublishedAt,
		markAll: "/entries/today/mark-read", markMatching: "/entries/today/mark-matching",
	}
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	location := a.userLocation(r.Context(), request.principal.User.ID)
	spec.filter = entry.UnreadFilterOn(a.snapshotDay(request.query.Snapshot, location), location)
	a.serveTab(w, r, request, spec)
}

// userLocation is the user's timezone, UTC when it cannot be read.
func (a *App) userLocation(ctx context.Context, userID int64) *time.Location {
	location, err := models.GetTimezone(ctx, a.DB, userID)
	if err != nil || location == nil {
		return time.UTC
	}
	return location
}

// snapshotDay is the local day of a page's snapshot, or of now: follow-up
// requests of a page keep meaning the day the reader saw.
func (a *App) snapshotDay(snapshot *string, location *time.Location) timezone.Date {
	if parsed, err := entry.ParseSnapshot(snapshot); err == nil && parsed != nil {
		return entry.DayOfSnapshot(*parsed, location)
	}
	return timezone.DateAt(a.Now(), location)
}

func (a *App) tabPage(w http.ResponseWriter, r *http.Request, spec tabSpec) {
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	a.serveTab(w, r, request, spec)
}

func (a *App) serveTab(w http.ResponseWriter, r *http.Request, request listRequest, spec tabSpec) {
	query := request.query
	scope := readingListScope
	path := spec.path
	layout := views.EntriesLayout{
		Active: spec.active, EmptyTitle: spec.emptyTitle, EmptyDetail: spec.emptyDetail, Path: path,
		ShowTabBar: spec.showTabBar, SnapshotAt: a.snapshotNow(), SearchAction: &path,
	}
	if spec.markMatching != "" {
		layout.MarkMatching = &spec.markMatching
	}
	if spec.active == "all" {
		layout.MarkAsReadScope = &scope
	}
	if spec.markAll != "" {
		layout.MarkAllAction = &spec.markAll
	}
	filter := spec.filter
	if filter.UnreadOnly {
		filter.ReadAfter = query.Snapshot
	}
	a.serveList(w, r, request.principal, query, listSpec{
		title: spec.title, layout: layout, filter: filter, scope: spec.filter, sort: spec.sort,
		found: parseScopedSearch(query.Q),
	})
}

// --- GET /entries/offline ---

func (a *App) offlineEntriesPage(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	ctx, userID := r.Context(), request.principal.User.ID
	keep, err := models.OfflineKeep(ctx, a.DB, userID)
	logQueryFailed("offline.keep", err)
	found, err := entry.ListOfflineSet(ctx, a.DB, userID, keep)
	logQueryFailed("offline.entries", err)
	rows := a.offlineRows(ctx, userID, found)
	title, detail := "Nothing saved yet", "Entries are mirrored while you are online. Come back once something has synced."
	if keep == models.OfflineKeepOff {
		title, detail = "Offline reading is off", "Turn it on in Settings to keep entries readable without a connection."
	}
	layout := views.EntriesLayout{
		Active: "offline", EmptyTitle: title, EmptyDetail: detail, Path: "/entries/offline",
		SnapshotAt: a.snapshotNow(),
	}
	list := views.EntriesList{Title: "Offline", Entries: rows, Layout: &layout, CSRF: request.principal.CSRFToken}
	a.writeListPage(w, r, request.principal, list, request.query)
}

// offlineRows turns the offline set into rows; a failing status query shows
// the rows without badges rather than none.
func (a *App) offlineRows(ctx context.Context, userID int64, found []entry.WithFeed) []views.EntryRow {
	ids := make([]int64, len(found))
	for i := range found {
		ids[i] = found[i].ID
	}
	statuses, err := models.SummaryStatusesFor(ctx, a.DB, userID, ids)
	logQueryFailed("offline.summary_statuses", err)
	rows := make([]views.EntryRow, len(found))
	for i := range found {
		rows[i] = a.rowViewFrom(&found[i], statuses[found[i].ID])
	}
	return rows
}

// --- GET /entries/{id} ---

// entryRedirect is GET /entries/{id}: the old deep link, now a list page with
// the entry pre-opened.
func (a *App) entryRedirect(w http.ResponseWriter, r *http.Request) {
	if _, ok := a.requirePage(w, r); !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	reader := queryReader{values: r.URL.Query()}
	origin, feed, category := reader.text("origin"), reader.id("feed"), reader.id("category")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return
	}
	http.Redirect(w, r, redirectBase(origin, feed, category)+"?entry="+strconv.FormatInt(id, 10), http.StatusSeeOther)
}

// redirectBase maps the list an entry was opened from back to its URL.
func redirectBase(origin *string, feed, category *int64) string {
	if origin == nil {
		return "/"
	}
	switch *origin {
	case "feed":
		return scopedBase("/feeds/%d/entries", feed)
	case "category":
		return scopedBase("/categories/%d/entries", category)
	case "read", "starred", "summarized", "today":
		return "/entries/" + *origin
	case "entries":
		return "/entries"
	case "search":
		return "/search"
	}
	return "/"
}

func scopedBase(pattern string, id *int64) string {
	if id == nil {
		return "/entries"
	}
	return fmt.Sprintf(pattern, *id)
}

// --- feed and category pages ---

// statusFilter narrows a feed or category scope by ?status=; anything but
// all, read or starred means unread.
func statusFilter(scope entry.Filter, status *string) entry.Filter {
	value := "unread"
	if status != nil {
		value = *status
	}
	switch value {
	case "all":
	case "read":
		scope.ReadOnly = true
	case "starred":
		scope.StarredOnly = true
	default:
		scope.UnreadOnly = true
	}
	return scope
}

// filterTabs are the All/Unread/Read/Starred pills of a scoped page.
func filterTabs(base string, status *string) []views.FilterTab {
	current := "unread"
	if status != nil {
		current = *status
	}
	var tabs []views.FilterTab
	for _, pill := range [][2]string{{"All", "all"}, {"Unread", ""}, {"Read", "read"}, {"Starred", "starred"}} {
		href := base
		if pill[1] != "" {
			href = base + "?status=" + pill[1]
		}
		active := current == pill[1] || (pill[1] == "" && current == "unread")
		tabs = append(tabs, views.FilterTab{Label: pill[0], Href: href, Active: active})
	}
	return tabs
}

// scopedPage is what a feed or category page adds to a list.
type scopedPage struct {
	scope               entry.Filter
	path                string
	markScope           string
	breadcrumb          []views.Crumb
	headerFeedIconID    int64
	activeCategoryID    int64
	activeFeedID        int64
	emptyTitle, emptyOK string
	title               string
}

func (a *App) serveScoped(w http.ResponseWriter, r *http.Request, request listRequest, page scopedPage) {
	query := request.query
	search := parseScopedSearch(query.Q)
	path, matching, markScope := page.path, page.path+"/mark-read", page.markScope
	layout := views.EntriesLayout{
		Active: "", EmptyTitle: page.emptyTitle, EmptyDetail: page.emptyOK, Path: path,
		MarkAsReadScope: &markScope, Breadcrumb: page.breadcrumb, HeaderFeedIconID: page.headerFeedIconID,
		ActiveCategoryID: page.activeCategoryID, ActiveFeedID: page.activeFeedID,
		FilterTabs: filterTabs(path, query.Status), StatusFilter: query.Status,
		SnapshotAt: a.snapshotNow(), SearchAction: &path, MarkMatching: &matching, ShowMarkAbove: search.raw == nil,
	}
	filter := statusFilter(page.scope, query.Status)
	filter.ReadAfter = query.Snapshot
	a.serveList(w, r, request.principal, query, listSpec{
		title: page.title, layout: layout, filter: filter, scope: page.scope, sort: entry.SortPublishedAt,
		loadMoreStatus: query.Status, alwaysCursor: true, paneSwap: true, found: search,
	})
}

func (a *App) categoryEntriesPage(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	ctx, principal := r.Context(), request.principal
	category, err := models.FindCategoryByIDAndUser(ctx, a.DB, id, principal.User.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if category == nil {
		a.renderNotFound(w, r, principal, "Category not found", "This category doesn't exist or you don't have access to it.")
		return
	}
	categories := "/categories"
	a.serveScoped(w, r, request, scopedPage{
		scope: entry.Filter{CategoryID: &id}, path: fmt.Sprintf("/categories/%d/entries", id),
		markScope:  "user/-/label/" + category.Name,
		breadcrumb: []views.Crumb{{Label: "Categories", Href: &categories}, {Label: category.Name}},
		emptyTitle: "Nothing in this category", emptyOK: "The feeds in this category haven't brought in any entries yet.",
		activeCategoryID: id, title: category.Name,
	})
}

func (a *App) feedEntriesPage(w http.ResponseWriter, r *http.Request) {
	request, ok := a.beginList(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	ctx, principal := r.Context(), request.principal
	feed, err := a.ownedFeed(r, id, principal.User.ID)
	if errors.Is(err, ErrFeedNotFound) {
		a.renderNotFound(w, r, principal, "Feed not found", "This feed doesn't exist or you don't have access to it.")
		return
	}
	if err != nil {
		writeError(w, err)
		return
	}
	a.serveScoped(w, r, request, a.feedScopedPage(ctx, feed, principal.User.ID))
}

// feedScopedPage describes a feed's page. A failing lookup of the category or
// icon only drops the breadcrumb link or icon.
func (a *App) feedScopedPage(ctx context.Context, feed *models.Feed, userID int64) scopedPage {
	title := "(untitled feed)"
	if feed.Title != nil {
		title = *feed.Title
	}
	feeds := "/feeds"
	crumbs := []views.Crumb{{Label: "Feeds", Href: &feeds}}
	category, err := models.FindCategoryByIDAndUser(ctx, a.DB, feed.CategoryID, userID)
	logQueryFailed("feed_page.category", err)
	if category != nil {
		link := fmt.Sprintf("/categories/%d/entries", category.ID)
		crumbs = append(crumbs, views.Crumb{Label: category.Name, Href: &link})
	}
	page := scopedPage{
		scope: entry.Filter{FeedID: &feed.ID}, path: fmt.Sprintf("/feeds/%d/entries", feed.ID),
		markScope:  "feed/" + feed.URL,
		breadcrumb: append(crumbs, views.Crumb{Label: title}),
		emptyTitle: "Nothing in this feed", emptyOK: "This feed hasn't published anything yet, or it's still syncing.",
		activeCategoryID: feed.CategoryID, activeFeedID: feed.ID, title: title,
	}
	hasIcon, err := models.ImageExists(ctx, a.DB, "feed", feed.ID)
	logQueryFailed("feed_page.icon", err)
	if hasIcon {
		page.headerFeedIconID = feed.ID
	}
	return page
}
