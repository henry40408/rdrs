package views

import (
	"strconv"

	"github.com/henry40408/rdrs/internal/flash"
)

// Layout is the shared data of every logged-in page.
type Layout struct {
	Theme                string // empty: no data-theme attribute
	GitVersion           string
	SidebarBootstrapJSON string
	// FlashMessages are server-rendered so flashes are visible without script.
	FlashMessages []flash.Message
	// IsAdmin shows the Admin link in the no-JS navigation.
	IsAdmin bool
	// TotalUnread is the unread count of the no-JS navigation.
	TotalUnread int64
	// CSRFToken is for the no-JS sign-out form; not every page has its own.
	CSRFToken string
	// OfflineKey names the offline cache (secret.OfflineID), inlined so
	// offline.js can drop another account's articles before any request.
	OfflineKey  string
	OfflineKeep int64
	// UnreadByFeed is the per-feed unread count from the sidebar cache.
	UnreadByFeed map[int64]int64
}

// appRoot holds the attributes only logged-in pages put on <html>.
type appRoot struct {
	theme      string
	offlineKey string
	keep       int64
}

// documentParts are the inputs of document; each fills one part of the page.
type documentParts struct {
	title      string // the complete <title> text, e.g. "Login - RDRS"
	gitVersion string // stamped on asset URLs
	root       *appRoot
	head       node // extra <head> children after the common ones
	body       node // the icon sprite is added automatically
}

// document is the whole HTML document, for pages that bring their own shell
// (login, setup, invite, offline).
func document(parts documentParts) node {
	v := parts.gitVersion
	htmlAttrs := attrs(a("lang", "en"))
	if root := parts.root; root != nil {
		htmlAttrs = append(htmlAttrs,
			when(root.theme != "", a("data-theme", root.theme)),
			a("data-offline-key", root.offlineKey),
			a("data-offline-keep", strconv.FormatInt(root.keep, 10)))
	}
	head := el("head", nil,
		void("meta", a("charset", "UTF-8")),
		void("meta", a("name", "viewport"), a("content", "width=device-width, initial-scale=1.0")),
		el("title", nil, text(parts.title)),
		// Version-stamped so a cached icon cannot outlive an upgrade.
		void("link", a("rel", "icon"), a("href", "/favicon.ico?v="+v), a("sizes", "32x32")),
		void("link", a("rel", "icon"), a("href", "/favicon.svg?v="+v), a("type", "image/svg+xml")),
		void("link", a("rel", "apple-touch-icon"), a("href", "/apple-touch-icon.png?v="+v)),
		// crossorigin makes the manifest request carry cookies; without it a
		// cookie-authenticating reverse proxy redirects it and installability breaks.
		void("link", a("rel", "manifest"), a("href", "/static/manifest.webmanifest?v="+v),
			a("crossorigin", "use-credentials")),
		// Two entries so browser chrome follows the system colour scheme (a
		// meta cannot follow the data-theme override). Values are --color-paper.
		void("meta", a("name", "theme-color"), a("media", "(prefers-color-scheme: light)"), a("content", "#F2F1ED")),
		void("meta", a("name", "theme-color"), a("media", "(prefers-color-scheme: dark)"), a("content", "#131416")),
		void("link", a("rel", "stylesheet"), a("href", "/static/css/app.css?v="+v)),
		// Loaded first so its fetch patch attaches the CSRF token before any
		// state-changing request.
		moduleScript(v, "csrf.js"),
		modulePreload(v, "components/rdrs-flash.js"),
		moduleScript(v, "components/rdrs-flash.js"),
		parts.head)
	body := el("body", nil,
		// Outside the page body so every page gets it; swap fragments reference its ids.
		sprite(),
		parts.body)
	return group(doctype, el("html", htmlAttrs, head, body))
}

// moduleScript is <script type="module" src="/static/js/{path}?v={version}">.
func moduleScript(gitVersion, path string) node {
	return el("script", attrs(a("type", "module"), a("src", "/static/js/"+path+"?v="+gitVersion)))
}

func modulePreload(gitVersion, path string) node {
	return void("link", a("rel", "modulepreload"), a("href", "/static/js/"+path+"?v="+gitVersion))
}

// appHead is the <head> addition for logged-in pages, then the page's script.
func appHead(layout *Layout, pageScript node) node {
	v := layout.GitVersion
	preloads := []string{
		"components/rdrs-kb-help.js", "components/rdrs-sidebar.js", "keyboard.js", "bulk-read.js",
		"entries-search.js", "live-updates.js", "summary.js", "app.js",
	}
	return group(
		each(preloads, func(path string) node { return modulePreload(v, path) }),
		moduleScript(v, "components/rdrs-kb-help.js"),
		moduleScript(v, "components/rdrs-sidebar.js"),
		moduleScript(v, "app.js"),
		// Delegated handlers replacing inline on* handlers (strict CSP).
		moduleScript(v, "behaviors.js"),
		// Not in the base document so /login and /setup never register a worker.
		// No modulepreload: it must not compete with the modules that paint.
		moduleScript(v, "pwa.js"),
		// Offline reading; runs after paint, no modulepreload.
		moduleScript(v, "offline.js"),
		// Reports browser-only facts once per change; after paint, no modulepreload.
		moduleScript(v, "client-info.js"),
		pageScript)
}

func navLink(href, label string) node {
	return el("a", attrs(a("href", href)), text(label))
}

// navFallback is the no-script navigation, since <rdrs-sidebar> renders
// nothing without script. Destinations only, to keep bytes low; inside
// <noscript> it is not parsed when scripting is on, so :has(.nav-fallback)
// never matches. Whitespace between the items is omitted: the nav is a flex
// container.
func navFallback(layout *Layout) node {
	unread := group(text("Unread"))
	if layout.TotalUnread > 0 {
		unread = group(text("Unread"), text(" "), el("span", attrs(class("nav-fallback-count")), num(layout.TotalUnread)))
	}
	return el("noscript", nil, el("nav", attrs(class("nav-fallback"), a("aria-label", "Main navigation")),
		el("a", attrs(class("nav-fallback-brand"), a("href", "/")), text("rdrs")),
		el("a", attrs(a("href", "/")), unread),
		navLink("/entries/today", "Today"),
		navLink("/entries", "All"),
		navLink("/entries/starred", "Starred"),
		navLink("/entries/summarized", "Summarized"),
		// Only for readers with offline reading on.
		show(layout.OfflineKeep > 0, navLink("/entries/offline", "Offline")),
		navLink("/categories", "Categories"),
		navLink("/feeds", "Feeds"),
		navLink("/search", "Search"),
		navLink("/summarizer", "Summarizer"),
		navLink("/statistics", "Statistics"),
		navLink("/user-settings", "Preferences"),
		navLink("/settings", "Settings"),
		show(layout.IsAdmin, navLink("/admin", "Admin")),
		// The scripted sign-out is a fetch DELETE; a form needs POST /logout.
		el("form", attrs(class("nav-fallback-logout"), a("method", "post"), a("action", "/logout")),
			csrfField(layout.CSRFToken),
			text(" "),
			el("button", attrs(a("type", "submit"), a("data-testid", "nav-fallback-logout")), text("Sign out")))))
}

// appPage is a logged-in page, with the sidebar, flash banners and
// navigation. title renders as "{title} - RDRS". pageScript is an optional
// module script (build it with moduleScript). Each page calls flashMount
// inside page.
//
// layout.SidebarBootstrapJSON is the one raw string: it comes from the
// sidebar serializer, which escapes </ to <\/, so it cannot close the
// <script> element.
func appPage(layout *Layout, title string, pageScript, page node) node {
	body := group(
		el("script", attrs(a("type", "application/json"), a("id", "rdrs-sidebar-bootstrap")),
			raw(layout.SidebarBootstrapJSON)),
		navFallback(layout),
		page,
		el("rdrs-kb-help", attrs(a("data-testid", "kb-help"))))
	return document(documentParts{
		title:      title + " - RDRS",
		gitVersion: layout.GitVersion,
		root:       &appRoot{theme: layout.Theme, offlineKey: layout.OfflineKey, keep: layout.OfflineKeep},
		head:       appHead(layout, pageScript),
		body:       body,
	})
}
