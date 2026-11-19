package views

import "strconv"

// EntryRow is one row of an entry list.
type EntryRow struct {
	ID                int64
	FeedID            int64
	FeedTitle         string
	FeedHasIcon       bool
	CategoryID        int64
	Title             string
	Link              *string
	PublishedISO      string
	PublishedRelative string
	IsRead            bool
	IsStarred         bool
	// SummaryStatus is empty when the entry has no summary.
	SummaryStatus string
}

// Crumb is a breadcrumb segment; a nil Href marks the current page.
type Crumb struct {
	Label string
	Href  *string
}

// FilterTab is a status-filter option of the feed and category pages.
type FilterTab struct {
	Label  string
	Href   string
	Active bool
}

// EntriesLayout is what the list column needs beyond its rows.
type EntriesLayout struct {
	Active          string
	EmptyTitle      string
	EmptyDetail     string
	Path            string
	ShowTabBar      bool
	MarkAsReadScope *string
	Breadcrumb      []Crumb
	// HeaderFeedIconID is 0 when no icon shows beside the title.
	HeaderFeedIconID int64
	// ActiveCategoryID and ActiveFeedID are 0 for no sidebar highlight.
	ActiveCategoryID int64
	ActiveFeedID     int64
	FilterTabs       []FilterTab
	StatusFilter     *string
	ShowMarkAbove    bool
	MarkAllAction    *string
	Onboarding       bool
	SnapshotAt       string
	Search           *string
	SearchAction     *string
	MatchingCount    *int64
	MarkMatching     *string
	SearchError      *string
}

// EntriesList is a list column: its rows and the cursor of the next page.
type EntriesList struct {
	Title      string
	Entries    []EntryRow
	NextCursor *string
	Layout     *EntriesLayout
	CSRF       string
}

// EntriesPage is a full entry-list page.
type EntriesPage struct {
	Layout      *Layout
	Title       string
	List        EntriesList
	ReadingPane *ReadingPane
}

// Entries renders a full entry-list page.
func Entries(page EntriesPage) string {
	pane := emptyPane()
	if page.ReadingPane != nil {
		pane = readingPane(page.ReadingPane, page.List.CSRF)
	}
	body := el("div", attrs(class("app-layout")),
		entriesSidebar(page.List.Layout),
		el("main", attrs(class("main-content")),
			flashMount(page.Layout.FlashMessages),
			el("div", attrs(class("split-view")), listPane(&page.List), pane)))
	return string(appPage(page.Layout, page.Title, "", body))
}

func optionalID(name string, id int64) attr {
	return when(id != 0, a(name, strconv.FormatInt(id, 10)))
}

func optionalText(name string, value *string) attr {
	if value == nil {
		return attr{skip: true}
	}
	return a(name, *value)
}

func entriesSidebar(layout *EntriesLayout) node {
	return el("rdrs-sidebar", attrs(a("active", layout.Active), optionalID("active-category-id", layout.ActiveCategoryID),
		optionalID("active-feed-id", layout.ActiveFeedID)))
}

// LoadMore describes a Load More response: the rows replace #load-more, and a
// further form follows when more rows exist.
type LoadMore struct {
	Entries    []EntryRow
	NextCursor *string
	CSRF       string
	Path       string
	Snapshot   *string
	Status     *string
	Query      *string
}

// LoadMoreFragment renders a Load More response. The cursor and snapshot are
// echoed, never re-stamped: every page of one session paginates against the
// same boundary.
func LoadMoreFragment(fragment LoadMore) string {
	var next node
	if fragment.NextCursor != nil {
		next = loadMoreForm(loadMoreFields{path: fragment.Path, after: *fragment.NextCursor,
			snapshot: fragment.Snapshot, status: fragment.Status, query: fragment.Query})
	}
	return string(el("template", attrs(a("data-swap-target", "#load-more")),
		each(fragment.Entries, func(row EntryRow) node { return entryRow(row, fragment.CSRF) }), next))
}

// PaneSwap is the category-switch response: it replaces the left column and
// resets the reading pane, leaving the sidebar alone.
func PaneSwap(list EntriesList) string {
	return string(group(
		el("template", attrs(a("data-swap-target", "[data-list-pane]")), listPane(&list)),
		el("template", attrs(a("data-swap-target", "#reading-pane")), emptyPane())))
}

// RefreshSwap is the scoped-search response: it replaces the list and the
// mark slots, leaving the focused search input alone.
func RefreshSwap(list EntriesList) string {
	layout := list.Layout
	list.Title = ""
	return string(group(
		el("template", attrs(a("data-swap-target", "[data-entries-list]")), entriesList(&list)),
		el("template", attrs(a("data-swap-target", "[data-search-error]")), searchError(layout)),
		el("template", attrs(a("data-swap-target", "[data-mark-all-slot]")), markAllSlot(layout, list.CSRF)),
		el("template", attrs(a("data-swap-target", "[data-mark-matching-slot]")), markMatchingSlot(layout, list.CSRF))))
}

func markAllSlot(layout *EntriesLayout, csrf string) node {
	return el("span", attrs(class("mark-all-slot"), flag("data-mark-all-slot")), markAllButton(layout, csrf))
}

func markMatchingSlot(layout *EntriesLayout, csrf string) node {
	return el("span", attrs(class("mark-matching-slot"), flag("data-mark-matching-slot")), markMatchingButton(layout, csrf))
}

// searchError is always rendered so a swap can fill it; hidden while the
// query parses.
func searchError(layout *EntriesLayout) node {
	var message node
	if layout.SearchError != nil {
		message = text(*layout.SearchError)
	}
	return el("div", attrs(class("search-error", "search-drawer-error"), a("role", "alert"), flag("data-search-error"),
		a("data-testid", "scoped-search-error"), when(layout.SearchError == nil, flag("hidden"))), message)
}

func entriesList(list *EntriesList) node {
	return el("div", attrs(class("list-pane-body"), flag("data-entries-list"), a("data-snapshot-at", list.Layout.SnapshotAt)),
		listBody(list))
}

func listPane(list *EntriesList) node {
	layout := list.Layout
	var drawer, tabs, filters node
	if layout.SearchAction != nil {
		drawer = searchDrawer(layout, *layout.SearchAction)
	}
	if layout.ShowTabBar {
		tabs = tabBar(layout.Active)
	}
	if layout.FilterTabs != nil || layout.MarkAsReadScope != nil || layout.MarkAllAction != nil {
		filters = entriesFilterBar(layout, list.CSRF)
	}
	return el("div", attrs(class("list-pane"), flag("data-list-pane")),
		drawer,
		el("div", attrs(class("list-pane-header")), breadcrumb(layout), titleRow(list), tabs, filters),
		entriesList(list))
}

// searchDrawer is a drawer rather than inline (it cost a whole row at
// <=1024px); it renders open when q is present so deep links do not flash.
func searchDrawer(layout *EntriesLayout, action string) node {
	var status node
	if layout.StatusFilter != nil {
		status = void("input", a("type", "hidden"), a("name", "status"), a("value", *layout.StatusFilter))
	}
	return el("div", attrs(class("search-drawer", onlyIf(layout.Search != nil, "is-open")), a("id", "search-drawer"), flag("data-search-drawer")),
		el("div", attrs(class("search-drawer-clip")),
			el("div", attrs(class("search-drawer-row")),
				el("form", attrs(class("entries-search"), a("method", "get"), a("action", action), a("data-swap", "[data-entries-list]"),
					flag("data-swap-fragment"), flag("data-swap-latest"), flag("data-sync-q"), a("data-live-search", "250"),
					flag("data-entries-search")),
					status, text(" "),
					void("input", a("type", "search"), a("name", "q"), a("class", "search-input"),
						a("placeholder", "Search in this view…"), a("autocomplete", "off"),
						a("data-testid", "scoped-search-input"), optionalText("value", layout.Search))),
				el("button", attrs(class("filter-chip-btn"), a("type", "button"), flag("data-search-help"),
					a("data-testid", "scoped-search-help"), a("aria-label", "Search syntax"), a("title", "Search syntax")), iconHelp()),
				text(" "),
				el("button", attrs(class("filter-chip-btn"), a("type", "button"), flag("data-search-close"),
					a("data-testid", "scoped-search-close"), a("aria-label", "Close search")), iconClose())),
			searchError(layout)))
}

func breadcrumb(layout *EntriesLayout) node {
	if len(layout.Breadcrumb) == 0 {
		return ""
	}
	var items []node
	for index, item := range layout.Breadcrumb {
		if index > 0 {
			items = append(items, el("span", attrs(class("breadcrumb-sep")), text(" / ")))
		}
		if item.Href != nil {
			items = append(items, el("a", attrs(a("href", *item.Href)), text(item.Label)))
			continue
		}
		items = append(items, el("span", nil, text(item.Label)))
	}
	return el("nav", attrs(class("breadcrumb"), a("data-testid", "breadcrumb")), group(items...))
}

// titleRow puts the search toggle beside the title, not in the filter bar:
// views without selects (Read) would otherwise get a row holding only it.
func titleRow(list *EntriesList) node {
	layout := list.Layout
	var icon, toggle node
	if layout.HeaderFeedIconID != 0 {
		icon = void("img", class("feed-icon"), a("src", "/api/feeds/"+strconv.FormatInt(layout.HeaderFeedIconID, 10)+"/icon"),
			a("alt", ""), a("width", "20"), a("height", "20"), a("decoding", "sync"))
	}
	if layout.SearchAction != nil {
		expanded := "false"
		if layout.Search != nil {
			expanded = "true"
		}
		toggle = el("button", attrs(class("filter-chip-btn"), a("type", "button"), flag("data-search-toggle"),
			a("data-testid", "scoped-search-toggle"), a("aria-controls", "search-drawer"), a("aria-expanded", expanded),
			a("aria-label", "Search in this view")), iconSearch())
	}
	return el("div", attrs(class("list-pane-title-row")), el("h1", nil, icon, text(list.Title)), toggle)
}

var entryTabs = []struct{ key, href, label string }{
	{"all", "/entries", "All"},
	{"read", "/entries/read", "Read"},
	{"starred", "/entries/starred", "Starred"},
	{"summarized", "/entries/summarized", "Summarized"},
}

func tabBar(active string) node {
	var tabs []node
	for index, tab := range entryTabs {
		if index > 0 {
			tabs = append(tabs, text(" "))
		}
		tabs = append(tabs, el("a", attrs(when(active == tab.key, a("class", "active")), a("href", tab.href),
			a("data-testid", "tab-"+tab.key), a("data-tab", tab.key)), text(tab.label)))
	}
	return el("div", attrs(class("tab-bar")), group(tabs...))
}

var markReadAges = []struct{ value, label string }{
	{"", "Mark as Read..."},
	{"1", "Older than 1 day"},
	{"7", "Older than 1 week"},
	{"30", "Older than 1 month"},
	{"365", "Older than 1 year"},
	{"all", "All entries"},
}

func entriesFilterBar(layout *EntriesLayout, csrf string) node {
	var statusSelect, ageSelect, markAll, matching node
	if layout.FilterTabs != nil {
		statusSelect = el("div", attrs(class("form-group", "form-group-inline"), flag("data-status-filter")),
			el("select", attrs(a("id", "status-filter"), a("data-testid", "status-filter-select"), flag("data-status-select"), a("class", "select-auto")),
				each(layout.FilterTabs, func(tab FilterTab) node {
					return el("option", attrs(a("value", tab.Href), when(tab.Active, flag("selected"))), text(tab.Label))
				})))
	}
	if layout.MarkAsReadScope != nil {
		ageSelect = el("div", attrs(class("form-group", "form-group-inline")),
			el("select", attrs(a("id", "mark-read-age"), a("data-testid", "mark-read-select"), a("class", "select-auto"),
				a("data-mark-read-scope", *layout.MarkAsReadScope)),
				each(markReadAges, func(age struct{ value, label string }) node {
					return el("option", attrs(a("value", age.value)), text(age.label))
				})))
	}
	if layout.MarkAllAction != nil {
		markAll = group(markAllSlot(layout, csrf), text(" "))
	}
	if layout.SearchAction != nil {
		matching = markMatchingSlot(layout, csrf)
	}
	return el("div", attrs(class("filter-bar")), statusSelect, ageSelect, markAll, matching)
}

// markAllButton is hidden while searching: it would mark rows the list does
// not show. data-snapshot-from-list refreshes the field at submit, after any
// in-place list refresh.
func markAllButton(layout *EntriesLayout, csrf string) node {
	if layout.MarkAllAction == nil || layout.Search != nil {
		return ""
	}
	return el("form", attrs(class("form-group", "form-group-inline"), a("method", "post"), a("action", *layout.MarkAllAction),
		a("data-confirm", "Mark all of today's unread entries as read?"), flag("data-snapshot-from-list"), a("data-testid", "mark-all-form")),
		csrfField(csrf), text(" "),
		void("input", a("type", "hidden"), a("name", "snapshot"), a("value", layout.SnapshotAt)), text(" "),
		el("button", attrs(class("btn", "btn-secondary"), a("type", "submit"), a("data-testid", "mark-all-btn")), text("Mark all as Read")))
}

func markMatchingButton(layout *EntriesLayout, csrf string) node {
	if layout.MatchingCount == nil || *layout.MatchingCount <= 0 || layout.MarkMatching == nil {
		return ""
	}
	count := strconv.FormatInt(*layout.MatchingCount, 10)
	var status, query node
	if layout.StatusFilter != nil {
		status = void("input", a("type", "hidden"), a("name", "status"), a("value", *layout.StatusFilter))
	}
	if layout.Search != nil {
		query = void("input", a("type", "hidden"), a("name", "q"), a("value", *layout.Search))
	}
	return el("form", attrs(class("form-group", "form-group-inline"), a("method", "post"), a("action", *layout.MarkMatching),
		a("data-testid", "mark-matching-form"), a("data-confirm", "Mark "+count+" matching entries as read?"), flag("data-snapshot-from-list")),
		csrfField(csrf), text(" "),
		status, text(" "),
		void("input", a("type", "hidden"), a("name", "snapshot"), a("value", layout.SnapshotAt)), text(" "),
		query, text(" "),
		el("button", attrs(class("btn", "btn-secondary"), a("type", "submit"), a("data-testid", "mark-matching-btn")),
			text("Mark "+count+" matching as Read")))
}

func listBody(list *EntriesList) node {
	if len(list.Entries) == 0 {
		return emptyState(list.Layout)
	}
	layout := list.Layout
	var more, above node
	if list.NextCursor != nil {
		// The boundary keeps entries read meanwhile from vanishing from the next page.
		more = loadMoreForm(loadMoreFields{path: layout.Path, after: *list.NextCursor,
			snapshot: &layout.SnapshotAt, status: layout.StatusFilter, query: layout.Search})
	}
	if layout.ShowMarkAbove {
		above = el("div", attrs(class("mark-above-form")),
			el("button", attrs(class("btn", "btn-secondary"), a("id", "mark-above-read"), a("type", "button"),
				a("data-testid", "mark-above-btn")), text("Mark Above as Read")))
	}
	return group(each(list.Entries, func(row EntryRow) node { return entryRow(row, list.CSRF) }), more, above)
}

func emptyState(layout *EntriesLayout) node {
	switch {
	case layout.Onboarding:
		return onboardingGuide()
	case layout.SearchError != nil:
		// The error banner above the list says why.
		return ""
	case layout.Search != nil:
		return el("div", attrs(class("empty-state-quiet"), a("data-testid", "scoped-search-empty")),
			el("p", attrs(class("empty-state-quiet-title")), text("No matches")),
			el("p", attrs(class("empty-state-quiet-text")), text("Nothing in this view matches your search.")))
	}
	return el("div", attrs(class("empty-state-quiet")),
		el("p", attrs(class("empty-state-quiet-title")), text(layout.EmptyTitle)),
		el("p", attrs(class("empty-state-quiet-text")), text(layout.EmptyDetail)))
}

func onboardingGuide() node {
	return el("div", attrs(class("empty-state-quiet", "onboarding-guide"), a("data-testid", "onboarding-guide")),
		el("p", attrs(class("empty-state-quiet-title")), text("Welcome to rdrs")),
		el("p", attrs(class("empty-state-quiet-text")), text("Three steps to your first read:")),
		el("ol", attrs(class("onboarding-steps")),
			el("li", nil, text("Add a feed, or import an OPML file")),
			el("li", nil, text("Wait for the first sync to fetch entries")),
			el("li", nil, text("Read"))),
		el("div", attrs(class("onboarding-cta")),
			el("a", attrs(class("btn"), a("href", "/feeds"), a("data-testid", "onboarding-add-feed")), text("Add your first feed →")),
			text(" "),
			el("a", attrs(class("btn", "btn-secondary"), a("href", "/feeds/import"), a("data-testid", "onboarding-import-opml")), text("Import OPML"))))
}

type loadMoreFields struct {
	path, after             string
	snapshot, status, query *string
}

func hiddenField(name string, value *string) node {
	if value == nil {
		return ""
	}
	return void("input", a("type", "hidden"), a("name", name), a("value", *value))
}

func loadMoreForm(fields loadMoreFields) node {
	return el("form", attrs(class("load-more-form"), a("id", "load-more"), a("method", "get"), a("action", fields.path), a("data-swap", "#load-more")),
		void("input", a("type", "hidden"), a("name", "after"), a("value", fields.after)), text(" "),
		void("input", a("type", "hidden"), a("name", "fragment"), a("value", "1")), text(" "),
		hiddenField("snapshot", fields.snapshot), text(" "),
		hiddenField("status", fields.status), text(" "),
		hiddenField("q", fields.query), text(" "),
		el("button", attrs(a("type", "submit"), a("data-testid", "load-more-btn")), text("Load More")))
}

// entryRow's ids, data-* attributes, data-testids and the .entry-status /
// .entry-read / .selected hooks are relied on by app.js, SSE and e2e.
func entryRow(row EntryRow, csrf string) node {
	id := strconv.FormatInt(row.ID, 10)
	var external node
	if row.Link != nil {
		external = el("a", attrs(class("entry-open-ext"), a("href", *row.Link), a("target", "_blank"), a("rel", "noopener noreferrer"),
			a("data-testid", "entry-open-original"), a("aria-label", "Open original"), a("title", "Open original (v)")), iconExternal())
	}
	return el("div", attrs(class("entry-item", onlyIf(row.IsRead, "entry-read")), a("id", "entry-row-"+id), flag("data-entry-row"),
		a("data-entry-id", id), a("data-category-id", strconv.FormatInt(row.CategoryID, 10)), optionalText("data-entry-link", row.Link),
		a("data-testid", "entry-item")),
		rowMarkerForm(row.ID, row.IsRead, csrf),
		el("a", attrs(class("entry-item-title"), a("href", "/entries/"+id+"/fragment"), a("data-swap", "#reading-pane"),
			a("title", row.Title), a("data-testid", "entry-title-link"), langAttr(row.Title)), text(row.Title)),
		text(" "),
		el("span", attrs(class("entry-timerow")),
			text(" "), entryStatus(row), text(" "),
			el("time", attrs(class("entry-time"), a("datetime", row.PublishedISO)), text(row.PublishedRelative)),
			text(" ")),
		el("div", attrs(class("entry-item-meta")),
			rowFavicon(row), text(" "),
			el("span", attrs(class("entry-meta-text")),
				el("a", attrs(class("entry-feed"), a("href", "/feeds/"+strconv.FormatInt(row.FeedID, 10)+"/entries")), text(row.FeedTitle)))),
		el("div", attrs(class("rail-actions")), rowStarForm(row.ID, row.IsStarred, csrf), external))
}

// entryStatus must render truly empty with no badge so .entry-status:empty
// hides it; SSE swaps this node.
func entryStatus(row EntryRow) node {
	var badge node
	switch row.SummaryStatus {
	case "completed":
		badge = el("span", attrs(class("summary-badge"), a("title", "Has Summary"), a("aria-hidden", "true")), iconSummary(true))
	case "pending":
		badge = el("span", attrs(class("summary-badge-pending"), a("title", "Pending"), a("aria-hidden", "true")), iconSummary(false))
	case "processing":
		badge = el("span", attrs(class("summary-badge-processing"), a("title", "Processing"), a("aria-hidden", "true")), iconSummary(false))
	case "failed":
		badge = el("span", attrs(class("summary-badge-failed"), a("title", "Failed"), a("aria-hidden", "true")), iconSummary(true))
	}
	return el("span", attrs(class("entry-status")), badge)
}

// rowFavicon has no loading="lazy" and decodes synchronously: WebKit
// otherwise blinks these icons on iOS whenever the row is repainted.
func rowFavicon(row EntryRow) node {
	feedID := strconv.FormatInt(row.FeedID, 10)
	if row.FeedHasIcon {
		return void("img", class("entry-favicon"), a("src", "/api/feeds/"+feedID+"/icon"), a("alt", ""),
			a("decoding", "sync"), a("width", "15"), a("height", "15"))
	}
	return el("span", attrs(a("class", "entry-favicon entry-favicon-chip fav-c"+strconv.FormatInt(feedColorIndex(row.FeedID), 10)),
		a("aria-hidden", "true")), text(feedInitial(row.FeedTitle)))
}
