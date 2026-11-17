package views

// Offline is the page the service worker serves without a network. It holds no
// user data and links plainly, since scripts may be what failed. The saved
// entries link is unconditional: this shared page cannot know if a library
// exists.
func Offline(gitVersion string) string {
	body := el("div", attrs(class("auth-layout")),
		el("div", attrs(class("auth-card"), a("data-testid", "offline-page")),
			el("h1", nil, text("rdrs")),
			el("p", attrs(class("offline-message")), text("You are offline.")),
			el("p", attrs(class("offline-message", "muted")),
				text("Feeds live on the server, so syncing, searching and marking entries have to wait for the connection. Anything you saved for offline reading is still here.")),
			el("div", attrs(class("link")),
				el("a", attrs(a("href", "/entries/offline"), a("data-testid", "offline-library-link")), text("Saved entries")),
				text(" "),
				el("a", attrs(a("href", "/")), text("Try again")))))
	return string(document(documentParts{title: "Offline - RDRS", gitVersion: gitVersion, body: body}))
}

// ErrorPage is the chrome-wrapped 404 of logged-in routes.
type ErrorPage struct {
	Layout  *Layout
	Heading string
	Message string
}

// Error renders the 404 page.
func Error(page ErrorPage) string {
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", nil),
		el("main", attrs(class("main-content")),
			flashMount(page.Layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text(page.Heading)),
				el("p", attrs(class("muted")), text(page.Message)),
				el("p", nil, el("a", attrs(a("href", "/")), text("Back to Unread"))))))
	return string(appPage(page.Layout, "Not Found", "", body))
}
