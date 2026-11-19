package views

import (
	"strconv"
	"unicode"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/han"
)

// ReadingPane is the data of the reading pane.
type ReadingPane struct {
	ID          int64
	Title       string
	Link        *string
	FeedTitle   string
	FeedID      int64
	FeedHasIcon bool
	Author      *string
	// PublishedISO is nil when the entry has no publication date.
	PublishedISO      *string
	PublishedRelative string
	// ContentHTML is sanitised upstream, hence raw.
	ContentHTML string
	IsRead      bool
	IsStarred   bool
	// SummaryText has passed through the summary sanitiser.
	SummaryText     *string
	SummaryInFlight bool
	// SummaryError is the latest failure; it tells "failed" from "none".
	SummaryError  *string
	HasKagi       bool
	HasSave       bool
	IsFullContent bool
	HasStoredFull bool
}

// Flash is the payload of a toast template.
type Flash struct{ Level, Message string }

// RowState is the part of an entry row the action responses re-render.
type RowState struct {
	ID        int64
	IsRead    bool
	IsStarred bool
}

// feedInitial is the uppercased first character, or "?" for an empty title.
func feedInitial(title string) string {
	r, size := utf8.DecodeRuneInString(title)
	if size == 0 {
		return "?"
	}
	return string(unicode.ToUpper(r))
}

// feedColorIndex is the stable favicon fallback colour, 0..5, from the id.
func feedColorIndex(feedID int64) int64 {
	index := feedID % 6
	if index < 0 {
		index += 6
	}
	return index
}

func langAttr(content string) attr {
	lang := han.LangAttr(content)
	return when(lang != "", a("lang", lang))
}

// emptyPane is the pane of the category swap; it must not carry
// reading-pane-active, or mobile readers get trapped behind a blank pane.
func emptyPane() node {
	return el("div", attrs(class("reading-pane", "reading-pane-empty"), a("id", "reading-pane")),
		el("p", attrs(class("empty-state-compact")), text("Select an entry from the list to start reading.")))
}

// PaneWithFlash is the pane plus an optional toast.
func PaneWithFlash(pane *ReadingPane, csrf string, flash *Flash) string {
	return string(group(
		el("template", attrs(a("data-swap-target", "#reading-pane")), readingPane(pane, csrf)),
		show(flash != nil, flashTemplate(flash))))
}

func flashTemplate(flash *Flash) node {
	if flash == nil {
		return ""
	}
	return el("template", attrs(flag("data-flash"), a("data-level", flash.Level)), text(flash.Message))
}

// SummarizePending swaps only the summary container, so a full-content view
// stays put.
func SummarizePending(id int64, csrf string) string {
	return string(el("template", attrs(a("data-swap-target", "#rp-summary-container")),
		summaryContainer(pendingBox(id, csrf))))
}

// SummaryCleared resets the container to empty.
func SummaryCleared() string {
	return string(el("template", attrs(a("data-swap-target", "#rp-summary-container")), summaryContainer("")))
}

// SummaryFragment lets an SSE summary event swap #rp-summary-container.
func SummaryFragment(pane *ReadingPane, csrf string) string {
	return string(el("template", attrs(a("data-swap-target", "#rp-summary-container")),
		summaryContainer(summaryInner(pane, csrf))))
}

// EntryActions is the response of the row actions: the two stateful forms, an
// entry-read class directive and, for a pane showing the entry, its star forms.
type EntryActions struct {
	Row  RowState
	CSRF string
	// Flash is an optional toast.
	Flash *Flash
	// PaneStar, when set, re-renders the pane's two star forms.
	PaneStar *RowState
}

// EntryActionsMulti renders an EntryActions response.
func EntryActionsMulti(resp EntryActions) string {
	rowID := strconv.FormatInt(resp.Row.ID, 10)
	var paneStar node
	if resp.PaneStar != nil {
		star := resp.PaneStar
		id := strconv.FormatInt(star.ID, 10)
		paneStar = group(
			el("template", attrs(a("data-swap-target", "#reading-pane-star-form-"+id)),
				paneStarForm(star.ID, star.IsStarred, resp.CSRF, false)),
			el("template", attrs(a("data-swap-target", "#reading-pane-bar-star-form-"+id)),
				paneStarForm(star.ID, star.IsStarred, resp.CSRF, true)))
	}
	return string(group(
		el("template", attrs(a("data-swap-target", "#entry-row-"+rowID+" .entry-marker")),
			rowMarkerForm(resp.Row.ID, resp.Row.IsRead, resp.CSRF)),
		el("template", attrs(a("data-swap-target", "#entry-row-"+rowID+" .entry-star-form")),
			rowStarForm(resp.Row.ID, resp.Row.IsStarred, resp.CSRF)),
		readClassDirective(resp.Row.ID, resp.Row.IsRead),
		flashTemplate(resp.Flash),
		paneStar))
}

// OpenEntryMulti updates the reading pane and marks the row read in one round
// trip; the star form is absent because opening never touches it.
func OpenEntryMulti(pane *ReadingPane, row RowState, csrf string) string {
	rowID := strconv.FormatInt(row.ID, 10)
	return string(group(
		el("template", attrs(a("data-swap-target", "#reading-pane")), readingPane(pane, csrf)),
		el("template", attrs(a("data-swap-target", "#entry-row-"+rowID+" .entry-marker")),
			rowMarkerForm(row.ID, row.IsRead, csrf)),
		readClassDirective(row.ID, row.IsRead)))
}

func readClassDirective(rowID int64, isRead bool) node {
	target := a("data-class-target", "#entry-row-"+strconv.FormatInt(rowID, 10))
	if isRead {
		return el("template", attrs(target, a("data-class-add", "entry-read")))
	}
	return el("template", attrs(target, a("data-class-remove", "entry-read")))
}

// readingPane carries reading-pane-active, which reveals it as a full-screen
// overlay on mobile; empty states must not.
func readingPane(pane *ReadingPane, csrf string) node {
	mode := "original"
	if pane.IsFullContent {
		mode = "full"
	}
	var byline node
	if pane.Author != nil {
		byline = el("div", attrs(class("byline")), el("span", attrs(class("author")), text(*pane.Author)))
	}
	return el("div", attrs(class("reading-pane", "reading-pane-active"), a("id", "reading-pane"),
		a("data-entry-id", strconv.FormatInt(pane.ID, 10))),
		paneToolbar(pane, csrf),
		el("div", attrs(class("reading-pane-content")),
			paneEyebrow(pane),
			paneHeadline(pane),
			byline,
			paneActionRow(pane, csrf),
			summaryContainer(summaryInner(pane, csrf)),
			el("article", attrs(class("reading-pane-article"), a("data-testid", "reading-pane-body"),
				a("data-mode", mode), langAttr(pane.ContentHTML)), raw(pane.ContentHTML))))
}

func paneFavicon(pane *ReadingPane, ariaHiddenChip bool) node {
	feedID := strconv.FormatInt(pane.FeedID, 10)
	if pane.FeedHasIcon {
		return void("img", class("entry-favicon"), a("src", "/api/feeds/"+feedID+"/icon"), a("alt", ""),
			a("decoding", "sync"), a("width", "15"), a("height", "15"))
	}
	return el("span", attrs(a("class", "entry-favicon entry-favicon-chip fav-c"+strconv.FormatInt(feedColorIndex(pane.FeedID), 10)),
		when(ariaHiddenChip, a("aria-hidden", "true"))), text(feedInitial(pane.FeedTitle)))
}

// paneToolbar holds the mobile Back button on the left and prev/next on the
// right. Desktop shows Star/Unread on the left; the <=1024px action bar
// carries them instead.
func paneToolbar(pane *ReadingPane, csrf string) node {
	return el("div", attrs(class("reading-pane-back")),
		el("div", attrs(class("reading-pane-bar-actions")),
			paneStarForm(pane.ID, pane.IsStarred, csrf, true),
			paneUnreadForm(pane.ID, csrf, true)),
		el("span", attrs(class("reading-pane-bar-title"), a("aria-hidden", "true")),
			text(" "), paneFavicon(pane, false), text(" "),
			el("span", attrs(class("reading-pane-bar-title-text"), langAttr(pane.Title)), text(pane.Title)),
			text(" ")),
		text(" "),
		el("button", attrs(class("reading-pane-back-link"), a("type", "button"), flag("data-pane-back"),
			a("data-testid", "reading-pane-back")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconChevronLeft()),
			el("span", nil, text("Back"))),
		text(" "),
		el("span", attrs(class("reading-pane-bar-pos"), flag("data-pane-pos"),
			a("data-testid", "reading-pane-position"), flag("hidden"))),
		text(" "),
		el("nav", attrs(class("reading-pane-nav"), a("aria-label", "Entry navigation")),
			el("button", attrs(class("reading-pane-nav-btn"), a("type", "button"), flag("data-pane-prev"),
				a("data-testid", "reading-pane-prev"), a("aria-label", "Previous"), flag("disabled")),
				iconChevronLeft(), el("span", attrs(class("nav-label")), text("Previous"))),
			text(" "),
			el("button", attrs(class("reading-pane-nav-btn"), a("type", "button"), flag("data-pane-next"),
				a("data-testid", "reading-pane-next"), a("aria-label", "Next"), flag("disabled")),
				el("span", attrs(class("nav-label")), text("Next")), iconChevronRight())))
}

func paneEyebrow(pane *ReadingPane) node {
	var published node
	if pane.PublishedISO != nil {
		published = group(text(" "), el("time", attrs(a("datetime", *pane.PublishedISO),
			a("data-testid", "reading-pane-published-at")), text(pane.PublishedRelative)))
	}
	return el("div", attrs(class("dispatch-eyebrow")),
		el("span", attrs(class("src")),
			text(" "), paneFavicon(pane, true), text(" "),
			el("span", attrs(a("data-testid", "reading-pane-feed-title")), text(pane.FeedTitle)),
			text(" ")),
		published)
}

func paneHeadline(pane *ReadingPane) node {
	title := text(pane.Title)
	if pane.Link != nil {
		title = el("a", attrs(a("href", *pane.Link), a("target", "_blank"), a("rel", "noopener noreferrer")), title)
	}
	return el("h1", attrs(class("reading-pane-title"), a("data-testid", "reading-pane-title"), langAttr(pane.Title)), title)
}

func paneActionRow(pane *ReadingPane, csrf string) node {
	var linked node
	if pane.Link != nil {
		linked = group(
			fullContentAction(pane, csrf),
			show(pane.HasKagi || pane.SummaryInFlight, summaryToggle(pane, csrf)),
			show(pane.HasSave, saveForm(pane.ID, csrf)))
	}
	return el("div", attrs(class("reading-pane-actions")),
		paneStarForm(pane.ID, pane.IsStarred, csrf, false),
		paneUnreadForm(pane.ID, csrf, false),
		linked)
}

func fullContentAction(pane *ReadingPane, csrf string) node {
	id := strconv.FormatInt(pane.ID, 10)
	switch {
	case pane.IsFullContent:
		// ?view=original is needed, else the stored fetched article comes back.
		return el("a", attrs(class("rp-action", "rp-slot-wide", "is-on"), a("href", "/entries/"+id+"/fragment?view=original"),
			a("data-swap", "#reading-pane"), a("aria-label", "Show Original")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconRevert()),
			el("span", attrs(class("action-label")), text("Original")),
			el("kbd", nil, text("d")))
	case pane.HasStoredFull:
		// A plain link back: undoing the toggle never re-fetches the origin.
		return el("a", attrs(class("rp-action", "rp-slot-wide"), a("href", "/entries/"+id+"/fragment"),
			a("data-swap", "#reading-pane"), a("aria-label", "Show Full Content")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconDocument()),
			el("span", attrs(class("action-label")), text("Full text")),
			el("kbd", nil, text("d")))
	}
	return el("form", attrs(class("rp-slot-wide"), a("method", "post"), a("action", "/entries/"+id+"/fetch-full-content"),
		a("data-swap", "#reading-pane"), flag("data-cancel-swap")),
		csrfField(csrf), text(" "),
		el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Fetch Full Content"),
			a("data-cancel-label", "Cancel"), a("data-cancel-aria-label", "Cancel")),
			el("span", attrs(class("action-icon", "action-icon-default"), a("aria-hidden", "true")), iconDocument()),
			el("span", attrs(class("action-icon", "action-icon-cancel"), a("aria-hidden", "true"), flag("hidden")), iconClose()),
			el("span", attrs(class("action-label")), text("Full text")),
			el("kbd", nil, text("d"))))
}

func summaryToggle(pane *ReadingPane, csrf string) node {
	hasSummary := pane.SummaryText != nil
	label, word := "Summarize", "Summary"
	if hasSummary {
		label, word = "Dismiss summary", "Dismiss"
	}
	return el("form", attrs(class("rp-slot-wide"), a("method", "post"), a("action", "/entries/"+strconv.FormatInt(pane.ID, 10)+"/summarize"),
		a("data-swap", "#rp-summary-container"), flag("data-summary-toggle")),
		csrfField(csrf), text(" "),
		el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", label),
			when(pane.SummaryInFlight, flag("disabled"))),
			el("span", attrs(class("action-icon", "action-icon-summarize"), a("aria-hidden", "true"), when(hasSummary, flag("hidden"))), iconSparkle()),
			el("span", attrs(class("action-icon", "action-icon-dismiss"), a("aria-hidden", "true"), when(!hasSummary, flag("hidden"))), iconClose()),
			el("span", attrs(class("action-label")), text(word)),
			el("kbd", nil, text("a"))))
}

func saveForm(id int64, csrf string) node {
	return el("form", attrs(a("method", "post"), a("action", "/entries/"+strconv.FormatInt(id, 10)+"/save"), a("data-swap", "#reading-pane")),
		csrfField(csrf), text(" "),
		el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Save")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconBookmark()),
			el("span", attrs(class("action-label")), text("Save")),
			el("kbd", nil, text("s"))))
}

func summaryContainer(inner node) node {
	return el("div", attrs(class("rp-summary-container"), a("id", "rp-summary-container"), flag("data-summary-container")), inner)
}

func cancelForm(id int64, csrf string) node {
	return el("form", attrs(a("method", "post"), a("action", "/entries/"+strconv.FormatInt(id, 10)+"/summarize/cancel"),
		a("data-swap", "#rp-summary-container")),
		csrfField(csrf), text(" "),
		el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Cancel summarization")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconClose()),
			el("span", attrs(class("action-label")), text("Cancel"))))
}

func pendingBox(id int64, csrf string) node {
	return el("div", attrs(class("summary-box"), flag("data-summary-pending")),
		el("div", attrs(class("summary-actions")), cancelForm(id, csrf)),
		el("p", attrs(class("muted")), text("Summarizing… (refresh to see the result)")))
}

func summaryInner(pane *ReadingPane, csrf string) node {
	switch {
	case pane.SummaryInFlight:
		return pendingBox(pane.ID, csrf)
	case pane.SummaryText != nil:
		return summaryBox(pane, *pane.SummaryText)
	case pane.SummaryError != nil:
		return failedBox(pane.ID, *pane.SummaryError, csrf)
	}
	return ""
}

// summaryBox takes summary as trusted: it has passed the summary sanitiser.
func summaryBox(pane *ReadingPane, summary string) node {
	var link node
	if pane.Link != nil {
		link = el("a", attrs(class("summary-link"), a("href", *pane.Link), a("target", "_blank"),
			a("rel", "noopener noreferrer"), flag("data-summary-link")), text(*pane.Link))
	}
	return el("div", attrs(class("summary-box")),
		el("div", attrs(class("summary-actions")),
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-summary-copy"), a("aria-label", "Copy summary")),
				el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconCopy()),
				el("span", attrs(class("action-label")), text("Copy"))),
			text(" "),
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-summary-dismiss"),
				a("data-entry-id", strconv.FormatInt(pane.ID, 10)), a("aria-label", "Dismiss summary")),
				el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconClose()),
				el("span", attrs(class("action-label")), text("Dismiss")))),
		el("div", attrs(class("summary-header")),
			el("div", attrs(class("summary-title"), flag("data-summary-title")), text(pane.Title)),
			link),
		el("blockquote", attrs(class("rp-summary-content")), raw(summary)))
}

func failedBox(id int64, message, csrf string) node {
	entry := strconv.FormatInt(id, 10)
	return el("div", attrs(class("summary-box")),
		el("div", attrs(class("summary-actions")),
			el("form", attrs(a("method", "post"), a("action", "/entries/"+entry+"/summarize"), a("data-swap", "#rp-summary-container")),
				csrfField(csrf), text(" "),
				el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Retry summarization")),
					el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconRefresh()),
					el("span", attrs(class("action-label")), text("Retry")))),
			el("form", attrs(a("method", "post"), a("action", "/entries/"+entry+"/summarize/cancel"), a("data-swap", "#rp-summary-container")),
				csrfField(csrf), text(" "),
				el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Clear failed summary")),
					el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconClose()),
					el("span", attrs(class("action-label")), text("Clear"))))),
		el("div", attrs(class("summary-error-banner"), flag("data-summary-error")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconClose()),
			el("span", nil, text("Summarization failed: "), text(message))))
}
