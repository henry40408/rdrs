package views

// Icon call sites into sprite; keep ids in sync with its symbols.

func useIcon(href string) node {
	return el("svg", attrs(class("ico"), a("aria-hidden", "true")), el("use", attrs(a("href", href))))
}

func iconUnread() node {
	return el("svg", attrs(class("ico", "is-filled"), a("aria-hidden", "true")), el("use", attrs(a("href", "#i-unread"))))
}

func iconStar(filled bool) node {
	return el("svg", attrs(class("ico", onlyIf(filled, "is-filled")), a("aria-hidden", "true")),
		el("use", attrs(a("href", "#i-star"))))
}

func iconSummary(filled bool) node {
	href := "#i-summary"
	if filled {
		href = "#i-summary-filled"
	}
	return el("svg", attrs(class("ico", onlyIf(filled, "is-filled")), a("aria-hidden", "true")),
		el("use", attrs(a("href", href))))
}

func iconExternal() node     { return useIcon("#i-external") }
func iconDocument() node     { return useIcon("#i-document") }
func iconRevert() node       { return useIcon("#i-revert") }
func iconRefresh() node      { return useIcon("#i-refresh") }
func iconSparkle() node      { return useIcon("#i-sparkle") }
func iconBookmark() node     { return useIcon("#i-bookmark") }
func iconCopy() node         { return useIcon("#i-copy") }
func iconClose() node        { return useIcon("#i-close") }
func iconSearch() node       { return useIcon("#i-search") }
func iconHelp() node         { return useIcon("#i-help") }
func iconChevronLeft() node  { return useIcon("#i-chevron-left") }
func iconChevronRight() node { return useIcon("#i-chevron-right") }

func path(d string) node { return el("path", attrs(a("d", d))) }

func symbol(id string, children ...node) node {
	return el("symbol", attrs(a("id", id), a("viewBox", "0 0 24 24")), children...)
}

const summaryStar = "M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"

// sprite is the symbol sheet, emitted once per document by document.
//
// No presentation attributes: .ico in app.css sets them and they inherit into
// <use>. `hidden` does not work on SVG-namespace elements, and an unsized
// sprite renders at 300x150, so it is sized with attributes rather than
// inline style (forbidden by CSP).
func sprite() node {
	return el("svg", attrs(
		a("xmlns", "http://www.w3.org/2000/svg"), a("aria-hidden", "true"), flag("data-icon-sprite"),
		a("class", "icon-sprite"), a("width", "0"), a("height", "0"), a("overflow", "hidden")),
		symbol("i-check", path("M5 13l4 4L19 7")),
		symbol("i-unread", el("circle", attrs(a("cx", "12"), a("cy", "12"), a("r", "6")))),
		symbol("i-star", path("M12 3.5l2.7 5.5 6 .9-4.3 4.2 1 6L12 17.3 6.6 20l1-6L3.3 9.9l6-.9z")),
		symbol("i-external", path("M14 4h6v6"), path("M20 4l-8.5 8.5"), path("M19 14v5H5V5h5")),
		symbol("i-document", path("M7 3h7l4 4v14H7z"), path("M14 3v4h4"), path("M9.5 12h6M9.5 16h6")),
		symbol("i-revert", path("M9 14L4 9l5-5"), path("M4 9h11a5 5 0 0 1 0 10h-4")),
		symbol("i-refresh", path("M21 12a9 9 0 1 1-2.6-6.4"), path("M21 3v5h-5")),
		symbol("i-sparkle",
			path("M12 3l1.7 4.6L18 9.3l-4.3 1.7L12 16l-1.7-4.9L6 9.3l4.3-1.7z"),
			path("M18 15l.7 1.8L20.5 17.5l-1.8.7L18 20l-.7-1.8L15.5 17.5l1.8-.7z")),
		symbol("i-bookmark", path("M6 4h12v17l-6-4-6 4z")),
		symbol("i-copy",
			el("rect", attrs(a("x", "9"), a("y", "9"), a("width", "11"), a("height", "11"), a("rx", "2"))),
			path("M5 15V6a2 2 0 0 1 2-2h8")),
		symbol("i-close", path("M6 6l12 12M18 6L6 18")),
		symbol("i-search",
			el("circle", attrs(a("cx", "11"), a("cy", "11"), a("r", "7"))), path("M21 21l-4.3-4.3")),
		symbol("i-help",
			el("circle", attrs(a("cx", "12"), a("cy", "12"), a("r", "9"))),
			path("M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 1-1 1.7"), path("M12 17h.01")),
		symbol("i-chevron-left", path("M15 6l-6 6 6 6")),
		symbol("i-chevron-right", path("M9 6l6 6-6 6")),
		symbol("i-summary-filled", path(summaryStar)),
		symbol("i-summary", el("g", attrs(a("transform", "translate(1.2 1.2) scale(0.9)")), path(summaryStar))),
	)
}
