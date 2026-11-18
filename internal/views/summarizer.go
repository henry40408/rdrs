package views

// SummarizerPage is the /summarizer page.
type SummarizerPage struct {
	Layout         *Layout
	KagiConfigured bool
	URLsText       string
	Error          *string
	Cards          []SummarizerCard
	CSRFToken      string
}

// Summarizer renders the /summarizer page.
func Summarizer(page SummarizerPage) string {
	layout := page.Layout
	content := summarizerNotConfigured()
	if page.KagiConfigured {
		content = summarizerFormAndResults(&page)
	}
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "summarizer"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content", "page-content-full")),
				el("h1", nil, text("Summarizer")),
				el("p", attrs(class("page-lead")), text("Paste one or more links and get a Kagi summary for each, one after another — without adding them to a feed. Nothing here is saved to your library.")),
				content)))
	return string(appPage(layout, "Summarizer", moduleScript(layout.GitVersion, "summarizer.js"), body))
}

// SummarizerCardFragment renders one card, the answer of /summarizer/item.
func SummarizerCardFragment(card SummarizerCard) string { return string(summarizerCardNode(card)) }

func summarizerNotConfigured() node {
	return el("div", attrs(class("empty-state")),
		el("p", attrs(class("empty-state-text")),
			text("Kagi isn’t configured yet. Add your Kagi session link in "),
			el("a", attrs(a("href", "/user-settings")), text("Settings")),
			text(" to use the summarizer.")))
}

func summarizerFormAndResults(page *SummarizerPage) node {
	var failure node
	if page.Error != nil {
		failure = el("p", attrs(class("form-error"), a("data-testid", "summarizer-error")), text(*page.Error))
	}
	return group(
		el("form", attrs(a("method", "post"), a("action", "/summarizer"), a("data-testid", "summarizer-form"), flag("data-summarizer-form")),
			csrfField(page.CSRFToken),
			text(" "),
			el("label", attrs(class("field-label"), a("for", "sz-urls")), text("Links to summarize")),
			el("textarea", attrs(a("id", "sz-urls"), a("name", "urls"), a("rows", "6"), a("spellcheck", "false"), class("textarea-full"),
				a("placeholder", "https://example.com/article\nhttps://another.com/post"), a("data-testid", "summarizer-input")),
				text(page.URLsText)),
			failure,
			el("div", attrs(class("form-actions")),
				el("span", attrs(class("muted")), text("One URL per line · up to 30")),
				text(" "),
				el("button", attrs(class("btn"), a("type", "submit")), text("Summarize")))),
		el("div", attrs(a("id", "sz-results"), flag("data-summarizer-results")),
			each(page.Cards, summarizerCardNode)))
}
