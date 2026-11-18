package views

import "strconv"

// SearchResult is one hit of /search. TitleHTML and SnippetHTML are already
// escaped and carry only <mark> tags.
type SearchResult struct {
	EntryID           int64
	TitleHTML         string
	FeedTitle         string
	PublishedRelative string
	// PublishedISO is empty when the time is unknown.
	PublishedISO string
	SnippetHTML  string
}

// SearchResults is what the page and both fragments render.
type SearchResults struct {
	Query      string
	Error      *string
	Results    []SearchResult
	NextCursor *string
}

// SearchPage is the full /search page.
type SearchPage struct {
	Layout  *Layout
	Results SearchResults
}

// Search renders the full /search page.
func Search(page SearchPage) string {
	layout := page.Layout
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "search"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content", "page-content-full")),
				el("h1", nil, text("Search")),
				searchForm(page.Results.Query),
				searchSyntaxPopover(),
				searchRegion(page.Results))))
	return string(appPage(layout, "Search", moduleScript(layout.GitVersion, "search.js"), body))
}

// SearchRefresh is the live-search response: it replaces the results and
// leaves the form, with its focus, alone.
func SearchRefresh(results SearchResults) string {
	return string(el("template", attrs(a("data-swap-target", "[data-search-results]")), searchRegion(results)))
}

// SearchMore is the Load More response: the items, and the next Load More if
// any, go in place of the old #search-load-more.
func SearchMore(results SearchResults) string {
	return string(el("template", attrs(a("data-swap-target", "#search-load-more")),
		each(results.Results, searchItem), searchLoadMore(results)))
}

func searchRegion(results SearchResults) node {
	return el("div", attrs(class("search-results-region"), flag("data-search-results")), searchBody(results))
}

// searchForm sits outside the swapped results so typing keeps focus. It has no
// `required`: clearing the field searches too, back to the empty state.
func searchForm(q string) node {
	return el("form", attrs(a("method", "get"), a("action", "/search"), a("data-swap", "[data-search-results]"),
		flag("data-swap-fragment"), flag("data-swap-latest"), flag("data-sync-q"), a("data-live-search", "300"), a("data-live-search-min", "2")),
		el("div", attrs(class("search-form-row")),
			void("input", a("type", "text"), a("id", "search-q"), a("name", "q"), a("value", q),
				a("placeholder", "Find entries by title or content"), a("aria-label", "Search keywords"),
				a("autocomplete", "off"), flag("autofocus"), a("data-testid", "search-input")),
			text(" "),
			el("button", attrs(a("type", "button"), class("filter-chip-btn"), flag("data-search-help"), a("popovertarget", "search-syntax-popover"),
				a("data-testid", "search-help"), a("aria-label", "Search syntax"), a("title", "Search syntax")), iconHelp()),
			text(" "),
			el("button", attrs(a("type", "submit"), a("data-testid", "search-btn")), text("Search"))))
}

// searchSyntaxPopover is the no-JS fallback of the help button; with scripts
// app.js opens <rdrs-kb-help> instead. Keep it in step with SEARCH_SYNTAX.
func searchSyntaxPopover() node {
	code := func(s string) node { return el("code", nil, text(s)) }
	return el("div", attrs(a("id", "search-syntax-popover"), class("search-syntax-popover"), flag("popover"),
		a("data-testid", "search-syntax-popover")),
		el("h2", nil, text("Search syntax")),
		el("dl", nil,
			el("dt", nil, code("is:unread"), text(" "), code("is:read"), text(" "), code("is:starred")),
			el("dd", nil, text("Status")),
			el("dt", nil, code("feed:name"), text(" "), code("category:name")),
			el("dd", nil, text("By feed or category (fuzzy, case-insensitive)")),
			el("dt", nil, code("title:word"), text(" "), code("author:name")),
			el("dd", nil, text("Title contains word; by author")),
			el("dt", nil, code("before:2026-01-01"), text(" "), code("after:2026-01-01")),
			el("dd", nil, text("Published before / after (UTC)")),
			el("dt", nil, code("AND OR NOT"), text(" "), code("( )")),
			el("dd", nil, text("Boolean operators and grouping; adjacent words imply AND")),
			el("dt", nil, code("-term"), text(" "), code(`"exact phrase"`)),
			el("dd", nil, text("Exclude a term; quote phrases and values with spaces, e.g. "), code(`feed:"Rust Blog"`))))
}

func searchBody(results SearchResults) node {
	switch {
	case results.Error != nil:
		return el("div", attrs(class("search-error"), a("role", "alert"), a("data-testid", "search-error")), text(*results.Error))
	case results.Query == "":
		return el("div", attrs(class("empty-state")),
			el("h2", attrs(class("empty-state-title")), text("Search your library")),
			el("p", attrs(class("empty-state-text")), text("Type a keyword and press "),
				el("kbd", attrs(class("empty-state-kbd")), text("Enter")), text(" to find entries by title or content.")))
	case len(results.Results) == 0:
		return el("div", attrs(class("empty-state"), a("data-testid", "search-empty")),
			el("h2", attrs(class("empty-state-title")), text("No matches")),
			el("p", attrs(class("empty-state-text")), text("Nothing matched “"), text(results.Query), text("”. Try another keyword or check the spelling.")))
	}
	return el("ul", attrs(class("search-results"), a("data-testid", "search-results")),
		each(results.Results, searchItem), searchLoadMore(results))
}

func searchItem(result SearchResult) node {
	var published node
	if result.PublishedRelative != "" {
		when := text(result.PublishedRelative)
		if result.PublishedISO != "" {
			when = el("time", attrs(a("datetime", result.PublishedISO)), text(result.PublishedRelative))
		}
		published = group(text(" "), el("span", attrs(class("muted")), text("· "), when, text(" ")))
	}
	var snippet node
	if result.SnippetHTML != "" {
		snippet = el("p", attrs(class("search-result-snippet")), raw(result.SnippetHTML))
	}
	return el("li", attrs(class("search-result")),
		el("a", attrs(a("href", "/entries/"+strconv.FormatInt(result.EntryID, 10)), class("search-result-title")), raw(result.TitleHTML)),
		el("div", attrs(class("search-result-meta")),
			el("span", attrs(class("muted")), text(result.FeedTitle)), published),
		snippet)
}

// searchLoadMore is the last item of the list, so a swap puts the next items
// inside the <ul>. Without JS the form navigates to the next page whole.
func searchLoadMore(results SearchResults) node {
	if results.NextCursor == nil {
		return ""
	}
	return el("li", attrs(a("id", "search-load-more"), class("search-load-more")),
		el("form", attrs(a("method", "get"), a("action", "/search"), a("data-swap", "#search-load-more"),
			flag("data-swap-fragment"), class("load-more-form")),
			void("input", a("type", "hidden"), a("name", "q"), a("value", results.Query)),
			text(" "),
			void("input", a("type", "hidden"), a("name", "after"), a("value", *results.NextCursor)),
			text(" "),
			el("button", attrs(a("type", "submit"), a("data-testid", "search-load-more-btn")), text("Load More"))))
}
