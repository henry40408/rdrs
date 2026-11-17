package views

import (
	"strconv"

	"github.com/henry40408/rdrs/internal/flash"
)

// SummarizerCard is one URL card of the summarizer page.
type SummarizerCard struct {
	Index   int
	URL     string
	Title   string
	State   string
	Summary string
	Error   string
}

func currentText(value string) node {
	if value == "" {
		return el("span", attrs(class("muted")), text("Not set"))
	}
	return el("code", nil, text(value))
}

// csrfField is the hidden field of native POSTs, required without script.
// csrf.js overwrites it at submit, since a token rotation can leave this
// snapshot stale.
func csrfField(token string) node {
	return void("input", a("type", "hidden"), a("name", "_csrf"), a("value", token))
}

func entryAction(id int64, verb string) string {
	return "/entries/" + strconv.FormatInt(id, 10) + "/" + verb
}

// rowMarkerForm is an entry row's read toggle. Shared with the swap responses
// so they cannot drift. data-swap is only the no-template fallback.
func rowMarkerForm(id int64, isRead bool, csrf string) node {
	verb, action := "Mark read", "read"
	if isRead {
		verb, action = "Mark unread", "unread"
	}
	return el("form", attrs(class("entry-marker"), a("method", "post"), a("action", entryAction(id, action)),
		a("data-swap", "#entry-row-"+strconv.FormatInt(id, 10))),
		csrfField(csrf),
		el("button", attrs(class("unread-toggle"), a("type", "submit"), a("data-testid", "entry-read-toggle"),
			a("aria-label", verb), a("title", verb+" (m)"))))
}

// rowStarForm is an entry row's star toggle.
func rowStarForm(id int64, isStarred bool, csrf string) node {
	verb, action := "Star", "star"
	if isStarred {
		verb, action = "Unstar", "unstar"
	}
	return el("form", attrs(class("entry-star-form"), a("method", "post"), a("action", entryAction(id, action)),
		a("data-swap", "#entry-row-"+strconv.FormatInt(id, 10))),
		csrfField(csrf),
		el("button", attrs(class("entry-star", onlyIf(isStarred, "starred")), a("type", "submit"),
			a("data-testid", "entry-star-action"), a("aria-label", verb), a("title", verb+" (f)")),
			iconStar(isStarred)))
}

// paneStarForm is the reading-pane star button, shared with the multi-swap
// response. bar picks the top-bar copy (desktop) over the action-row copy
// (<=1024px); both ship, so their ids differ.
func paneStarForm(id int64, isStarred bool, csrf string, bar bool) node {
	verb, action, label := "Star", "star", "Star"
	if isStarred {
		verb, action, label = "Unstar", "unstar", "Starred"
	}
	formID := "reading-pane-star-form-"
	if bar {
		formID = "reading-pane-bar-star-form-"
	}
	return el("form", attrs(a("id", formID+strconv.FormatInt(id, 10)),
		when(!bar, a("class", "rp-bar-dup")), a("method", "post"), a("action", entryAction(id, action)),
		a("data-swap", "#entry-row-"+strconv.FormatInt(id, 10))),
		csrfField(csrf),
		el("button", attrs(class("rp-action", onlyIf(isStarred, "is-on")), a("type", "submit"), a("aria-label", verb)),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconStar(isStarred)),
			el("span", attrs(class("action-label")), text(label)),
			el("kbd", nil, text("f"))))
}

// paneUnreadForm is Mark-Unread only: opening an entry already marks it read.
func paneUnreadForm(id int64, csrf string, bar bool) node {
	return el("form", attrs(when(!bar, a("class", "rp-bar-dup")), a("method", "post"),
		a("action", entryAction(id, "unread")), a("data-swap", "#entry-row-"+strconv.FormatInt(id, 10))),
		csrfField(csrf),
		el("button", attrs(class("rp-action"), a("type", "submit"), a("aria-label", "Mark Unread")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconUnread()),
			el("span", attrs(class("action-label")), text("Unread")),
			el("kbd", nil, text("m"))))
}

// flashBanners are the banners shared by both mounts. It emits nothing for an
// empty list: .banner-stack:empty must match, or an invisible band misaligns
// the mobile header.
func flashBanners(messages []flash.Message) node {
	return each(messages, func(m flash.Message) node {
		return el("div", attrs(a("class", "banner "+m.LevelClass()), a("role", m.AriaRole()),
			a("data-testid", "flash-message")),
			el("span", attrs(class("banner-icon"), a("aria-hidden", "true"))),
			el("div", attrs(class("banner-body")),
				el("span", attrs(class("sr-only")), text(m.LevelName()), text(": ")),
				el("span", attrs(class("banner-message")), text(m.Text))),
			el("time", attrs(class("banner-time"), a("datetime", m.TimestampISO()), a("data-testid", "flash-time")),
				text(m.FormattedTime())),
			text(" "),
			el("button", attrs(class("banner-dismiss"), a("type", "button"), a("aria-label", "Dismiss notification"),
				a("data-testid", "flash-close"), flag("data-flash-dismiss")), iconClose()))
	})
}

// flashStatic is the logged-out form: a plain container, since those pages
// never flash after load.
func flashStatic(messages []flash.Message) node {
	if len(messages) == 0 {
		return ""
	}
	return el("div", attrs(class("banner-stack"), a("role", "region"), a("aria-label", "Notifications"),
		a("tabindex", "-1")), flashBanners(messages))
}

// flashMount is the logged-in form: <rdrs-flash> is the client's toast mount,
// so it must render even when empty.
func flashMount(messages []flash.Message) node {
	return el("rdrs-flash", attrs(class("banner-stack"), a("role", "region"), a("aria-label", "Notifications"),
		a("tabindex", "-1")), flashBanners(messages))
}

// summarizerCardNode is one summarizer URL card. card.Summary is trusted: it
// has passed through the summary sanitizer (Kagi output is untrusted).
func summarizerCardNode(card SummarizerCard) node {
	completed := card.State == "completed"
	failed := card.State == "error"
	index := strconv.Itoa(card.Index)

	var actions, body node
	switch {
	case completed:
		actions = group(
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-sz-copy"),
				a("aria-label", "Copy summary")), el("span", attrs(class("action-label")), text("Copy"))),
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-sz-dismiss"),
				a("aria-label", "Dismiss summary")), el("span", attrs(class("action-label")), text("Dismiss"))))
		body = el("blockquote", attrs(class("rp-summary-content"), flag("data-sz-body")), raw(card.Summary))
	case failed:
		actions = group(
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-sz-retry"),
				a("aria-label", "Retry summarization")), el("span", attrs(class("action-label")), text("Retry"))),
			el("button", attrs(class("rp-action"), a("type", "button"), flag("data-sz-dismiss"),
				a("aria-label", "Dismiss")), el("span", attrs(class("action-label")), text("Dismiss"))))
		body = el("div", attrs(class("summary-error-banner"), flag("data-sz-error")),
			el("span", attrs(class("action-icon"), a("aria-hidden", "true")), iconClose()),
			el("span", nil, text("Summarization failed: "), text(card.Error)))
	default:
		body = el("p", attrs(class("status"), flag("data-sz-status")), text("Queued"))
	}
	return el("div", attrs(class("summary-box", "sz-card", onlyIf(card.State == "queued", "pending")),
		a("id", "sz-card-"+index), flag("data-summarizer-card"), a("data-summarizer-url", card.URL),
		a("data-summarizer-index", index), a("data-state", card.State)),
		el("div", attrs(class("summary-actions"), flag("data-sz-actions")), actions),
		el("div", attrs(class("summary-header")),
			el("div", attrs(class("summary-title"), flag("data-sz-title")), text(card.Title)),
			el("a", attrs(class("summary-link"), a("href", card.URL), a("target", "_blank"),
				a("rel", "noopener noreferrer")), text(card.URL))),
		body)
}
