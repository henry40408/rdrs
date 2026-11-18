package views

import "strconv"

const errorRowClass = "feed-error-no-border"

// FeedRow is one row of the /feeds table.
type FeedRow struct {
	ID         int64
	URL        string
	Title      string
	CategoryID int64
	// CategoryName is shown in the Category column.
	CategoryName string
	HasIcon      bool
	// FetchError is empty when the last fetch worked.
	FetchError          string
	HasFetchError       bool
	UnreadCount         int64
	FetchedAtRelative   string
	FetchedAtDatetime   string
	FeedUpdatedRelative string
	FeedUpdatedDatetime string
	FreshnessClass      string
	FreshnessKey        string
	OpenRateLabel       string
	HasOpenRateLabel    bool
	OpenRatePercent     int64
	HasOpenRatePercent  bool
}

// FeedCategoryOption is a category in a select, with its feed count on /feeds.
type FeedCategoryOption struct {
	ID        int64
	Name      string
	FeedCount int64
}

// FeedFilterLink is a pill of the filter bar.
type FeedFilterLink struct {
	Label  string
	Href   string
	Active bool
}

// FeedsPage is the data of GET /feeds.
type FeedsPage struct {
	Layout         *Layout
	CSRFToken      string
	Feeds          []FeedRow
	Categories     []FeedCategoryOption
	TotalFeedCount int64
	ActiveFilter   string
	ActiveSort     string
	// ActiveCategoryID is 0 for all categories.
	ActiveCategoryID int64
	FilterLinks      []FeedFilterLink
	// ReturnTo is this filtered view, posted back by the row forms.
	ReturnTo string
	// EditQuery is ReturnTo as a query string for the edit links.
	EditQuery string
	// FreshMaxDays and WarningMaxDays are the thresholds of the freshness help.
	FreshMaxDays   int64
	WarningMaxDays int64
	// OpenRateShown adds the Open rate column.
	OpenRateShown     bool
	MinTrackedForRate int64
}

// Feeds renders the feed list.
func Feeds(page FeedsPage) string {
	layout := page.Layout
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "feeds"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Feeds")),
				el("div", attrs(class("feeds-toolbar")),
					el("a", attrs(a("href", "/reader/api/0/subscription/export"), class("btn", "btn-secondary")), text("Export OPML")),
					text(" "),
					el("a", attrs(a("href", "/feeds/import"), class("btn", "btn-secondary")), text("Import OPML"))),
				addFeedForm(page),
				filterBar(page),
				freshnessHelp(page),
				feedsTable(page))))
	return string(appPage(layout, "Feeds", "", body))
}

func hiddenReturnTo(value string) node {
	return void("input", a("type", "hidden"), a("name", "return_to"), a("value", value))
}

func addFeedForm(page FeedsPage) node {
	var options node
	if len(page.Categories) == 0 {
		options = el("option", attrs(a("value", "")), text("No categories available"))
	} else {
		options = each(page.Categories, func(c FeedCategoryOption) node {
			return el("option", attrs(a("value", strconv.FormatInt(c.ID, 10))), text(c.Name))
		})
	}
	return el("form", attrs(a("method", "post"), a("action", "/feeds")),
		csrfField(page.CSRFToken),
		text(" "),
		hiddenReturnTo(page.ReturnTo),
		text(" "),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "url")), text("Feed URL")),
			text(" "),
			void("input", a("type", "text"), a("id", "url"), a("name", "url"),
				a("placeholder", "https://example.com/feed.xml or https://example.com"),
				flag("required"), a("data-testid", "feed-url-input"))),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "category_id")), text("Category")),
			text(" "),
			el("select", attrs(a("id", "category_id"), a("name", "category_id"), flag("required"),
				a("data-testid", "feed-category-select")), options)),
		el("button", attrs(a("type", "submit"), a("data-testid", "add-feed-btn")), text("Add Feed")))
}

func filterBar(page FeedsPage) node {
	sortOption := func(value, label string) node {
		return el("option", attrs(a("value", value), when(page.ActiveSort == value, flag("selected"))), text(label))
	}
	return el("form", attrs(a("method", "get"), a("action", "/feeds"), class("filter-bar")),
		el("div", attrs(class("form-group", "form-group-inline")),
			el("label", attrs(a("for", "filter-category")), text("Category")),
			text(" "),
			el("select", attrs(a("id", "filter-category"), a("name", "category"), class("select-auto"),
				flag("data-submit-on-change")),
				el("option", attrs(a("value", "")), text("All Categories ("), num(page.TotalFeedCount), text(")")),
				each(page.Categories, func(c FeedCategoryOption) node {
					return el("option", attrs(a("value", strconv.FormatInt(c.ID, 10)),
						when(page.ActiveCategoryID == c.ID, flag("selected"))),
						text(c.Name), text(" ("), num(c.FeedCount), text(")"))
				}))),
		el("div", attrs(class("form-group", "form-group-inline")),
			el("label", attrs(a("for", "sort-by")), text("Sort")),
			text(" "),
			el("select", attrs(a("id", "sort-by"), a("name", "sort"), class("select-auto"), flag("data-submit-on-change")),
				sortOption("title", "Title"),
				sortOption("unread", "Unread Count"),
				sortOption("category", "Category"),
				show(page.OpenRateShown, sortOption("open_rate", "Open Rate (lowest first)")))),
		el("div", attrs(class("form-group", "form-group-inline", "feed-filter-links")),
			each(page.FilterLinks, func(link FeedFilterLink) node {
				return group(el("a", attrs(a("href", link.Href),
					class("feed-filter-link", onlyIf(link.Active, "active"))), text(link.Label)), text(" "))
			})),
		void("input", a("type", "hidden"), a("name", "filter"), a("value", page.ActiveFilter)),
		text(" "),
		el("noscript", nil,
			el("div", attrs(class("form-group", "form-group-inline")),
				el("button", attrs(a("type", "submit"), class("btn", "btn-secondary"),
					a("data-testid", "feed-filter-apply")), text("Apply")))))
}

func freshnessHelp(page FeedsPage) node {
	fresh, warning := strconv.FormatInt(page.FreshMaxDays, 10), strconv.FormatInt(page.WarningMaxDays, 10)
	items := group(
		el("li", nil, el("strong", nil, text("Fetched")), text(" — when rdrs last "), el("em", nil, text("tried")),
			text(" to read this feed, whether or not it worked. Every feed is retried about once an hour.")),
		el("li", nil, el("strong", nil, text("Updated")), text(" — the "), el("em", nil, text("newest")),
			text(" of three signals, whichever of them this feed provides. They are not ranked: a missing signal is skipped, not treated as old.")),
		el("li", nil,
			text("Those signals are the feed’s own date ("), el("code", nil, text("<updated>")),
			text(", falling back to "), el("code", nil, text("<pubDate>")), text(" / "), el("code", nil, text("<lastBuildDate>")),
			text("), its newest entry’s date ("), el("code", nil, text("<published>")),
			text(", falling back to that entry’s "), el("code", nil, text("<updated>")), text("), and the "),
			el("code", nil, text("Last-Modified")), text(" header its server sends.")),
		el("li", nil, text("rdrs re-asks with "), el("code", nil, text("If-None-Match")), text(" / "),
			el("code", nil, text("If-Modified-Since")), text(". A "), el("code", nil, text("304 Not Modified")),
			text(" moves "), el("strong", nil, text("Fetched")), text(" forward and leaves "),
			el("strong", nil, text("Updated")), text(" untouched.")),
		el("li", nil, text("A feed that publishes no dates at all shows "), el("em", nil, text("No date info")),
			text("; its age is then judged by "), el("strong", nil, text("Fetched")), text(" instead.")),
		el("li", nil,
			text("Age: up to "), text(fresh), text(" days is normal, "),
			el("span", attrs(class("feed-freshness-warning")), text(strconv.FormatInt(page.FreshMaxDays+1, 10)), text("–"), text(warning), text(" days")),
			text(" is a warning, "),
			el("span", attrs(class("feed-freshness-stale")), text("over "), text(warning), text(" days")),
			text(" — or no date at all — is stale. The "), el("strong", nil, text("Stale")),
			text(" filter matches only that last group.")),
		el("li", nil, text("A failed fetch shows its error under the row and counts towards the "),
			el("strong", nil, text("Errors")),
			text(" filter; it clears as soon as one fetch succeeds. Feeds are never auto-disabled for failing.")))
	return el("details", attrs(class("feed-freshness-help"), a("data-testid", "feed-freshness-help")),
		el("summary", nil, text("How “Fetched” and “Updated” are decided")),
		el("div", attrs(class("feed-freshness-help-body")), el("ul", nil, items)))
}

func feedsTable(page FeedsPage) node {
	colspan := "4"
	if page.OpenRateShown {
		colspan = "5"
	}
	rows := each(page.Feeds, func(feed FeedRow) node {
		return group(feedRow(page, feed), show(feed.HasFetchError,
			el("tr", attrs(class("error-row"), a("data-feed-id", strconv.FormatInt(feed.ID, 10)),
				a("data-is-error-row", "true")),
				el("td", attrs(a("colspan", colspan), class("error-text", "feed-error-cell")),
					text("Error: "), text(feed.FetchError)))))
	})
	return el("table", attrs(class("mobile-cards", "feeds-table")),
		el("thead", nil, el("tr", nil,
			el("th", nil, text("Title")), el("th", nil, text("Category")), el("th", nil, text("Unread")),
			show(page.OpenRateShown, el("th", nil, text("Open Rate"))),
			el("th", nil, text("Actions")))),
		el("tbody", attrs(a("data-testid", "feeds-table")),
			rows,
			show(len(page.Feeds) == 0, el("tr", nil,
				el("td", attrs(a("colspan", colspan), class("empty-state-compact")),
					text("No feeds yet — add one using the form above."))))))
}

func feedRow(page FeedsPage, feed FeedRow) node {
	id := strconv.FormatInt(feed.ID, 10)
	cellClass := onlyIf(feed.HasFetchError, errorRowClass)
	cell := func(extra ...attr) []attr {
		return append(extra, when(feed.HasFetchError, a("class", cellClass)))
	}
	unread := text("0")
	if feed.UnreadCount > 0 {
		unread = el("strong", nil, num(feed.UnreadCount))
	}
	var openRate node
	if page.OpenRateShown {
		content := el("span", attrs(class("muted"),
			a("title", "Needs at least "+strconv.FormatInt(page.MinTrackedForRate, 10)+" tracked entries")), text("—"))
		if feed.HasOpenRateLabel {
			content = text(feed.OpenRateLabel)
		}
		openRate = el("td", attrs(a("data-label", "Open Rate"), when(feed.HasFetchError, a("class", cellClass)),
			a("data-testid", "feed-open-rate")), content)
	}
	var icon node
	if feed.HasIcon {
		icon = group(void("img", a("src", "/api/feeds/"+id+"/icon"), a("alt", ""), class("feed-icon"),
			flag("data-hide-on-error")), text(" "))
	}
	return el("tr", attrs(a("id", "row-feed-"+id), a("data-feed-id", id), when(feed.HasFetchError, a("class", cellClass))),
		el("td", cell(a("data-label", "Title")),
			el("div", attrs(class("feed-title-cell")),
				el("div", attrs(class("feed-title-line")), icon, el("span", attrs(a("title", feed.URL)), text(feed.Title))),
				el("div", attrs(class("feed-health-info")),
					el("span", attrs(class("muted")), text("Fetched: "), relativeTime(feed.FetchedAtDatetime, feed.FetchedAtRelative)),
					text(" · "),
					el("span", attrs(a("class", feed.FreshnessClass)), text("Updated: "),
						relativeTime(feed.FeedUpdatedDatetime, feed.FeedUpdatedRelative))))),
		el("td", cell(a("data-label", "Category")), text(feed.CategoryName)),
		el("td", cell(a("data-label", "Unread")), unread),
		openRate,
		el("td", attrs(class("actions", onlyIf(feed.HasFetchError, errorRowClass))), feedActions(page, id)))
}

// relativeTime is a <time> whose title repeats the timestamp only when there is one.
func relativeTime(datetime, relative string) node {
	return el("time", attrs(a("datetime", datetime), when(datetime != "", a("title", datetime))), text(relative))
}

func feedActions(page FeedsPage, id string) node {
	return group(
		el("a", attrs(a("href", "/feeds/"+id+"/entries")), text("entries")),
		text(" "),
		el("form", attrs(a("method", "post"), a("action", "/feeds/"+id+"/refresh"), class("form-inline")),
			csrfField(page.CSRFToken), text(" "), hiddenReturnTo(page.ReturnTo), text(" "),
			el("button", attrs(a("type", "submit"), class("action-link")), text("refresh"))),
		text(" "),
		el("a", attrs(a("href", "/feeds/"+id+"/edit?"+page.EditQuery)), text("edit")),
		text(" "),
		el("form", attrs(a("method", "post"), a("action", "/feeds/"+id+"/delete"), class("form-inline"),
			a("data-confirm", "Delete this feed? This cannot be undone.")),
			csrfField(page.CSRFToken), text(" "), hiddenReturnTo(page.ReturnTo), text(" "),
			el("button", attrs(a("type", "submit"), class("action-link", "action-link-danger")), text("delete"))))
}

// FeedEdit is the editable view of one feed.
type FeedEdit struct {
	ID                   int64
	URL, Title           string
	Description, SiteURL string
	CategoryID           int64
	CustomUserAgent      string
	HTTP2Disabled        bool
	CustomReferrer       string
	ReferrerSuggestions  []string
}

// FeedEditPage is the data of GET /feeds/{id}/edit.
type FeedEditPage struct {
	Layout     *Layout
	CSRFToken  string
	Feed       FeedEdit
	Categories []FeedCategoryOption
	// ReturnTo is the list Save and Cancel go back to.
	ReturnTo             string
	UserAgentSuggestions []string
}

// FeedEditView renders the feed edit page.
func FeedEditView(page FeedEditPage) string {
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "feeds"))),
		el("main", attrs(class("main-content")),
			flashMount(page.Layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Edit Feed")),
				editForm(page),
				void("hr"),
				refetchMetadataForm(page))))
	return string(appPage(page.Layout, "Edit Feed", "", body))
}

func textField(id, label, value, testID string) node {
	return el("div", attrs(class("form-group")),
		el("label", attrs(a("for", id)), text(label)),
		text(" "),
		void("input", a("type", "text"), a("id", id), a("name", id), a("value", value),
			when(id == "url", flag("required")), when(testID != "", a("data-testid", testID))))
}

func editForm(page FeedEditPage) node {
	feed := page.Feed
	id := strconv.FormatInt(feed.ID, 10)
	return el("form", attrs(a("method", "post"), a("action", "/feeds/"+id+"/edit")),
		csrfField(page.CSRFToken), text(" "), hiddenReturnTo(page.ReturnTo), text(" "),
		textField("url", "Feed URL", feed.URL, "feed-edit-url-input"),
		textField("title", "Title", feed.Title, "feed-edit-title-input"),
		textField("description", "Description", feed.Description, ""),
		textField("site_url", "Site URL", feed.SiteURL, ""),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "category_id")), text("Category")),
			text(" "),
			el("select", attrs(a("id", "category_id"), a("name", "category_id"), flag("required"),
				a("data-testid", "feed-edit-category-select")),
				each(page.Categories, func(c FeedCategoryOption) node {
					return el("option", attrs(a("value", strconv.FormatInt(c.ID, 10)),
						when(c.ID == feed.CategoryID, flag("selected"))), text(c.Name))
				}))),
		httpSettings(page),
		el("div", attrs(class("modal-actions")),
			el("button", attrs(a("type", "submit"), a("data-testid", "feed-edit-save-btn")), text("Save")),
			text(" "),
			el("a", attrs(a("href", page.ReturnTo), class("btn", "btn-secondary")), text("Cancel"))))
}

func httpSettings(page FeedEditPage) node {
	feed := page.Feed
	hasSuggestions := len(feed.ReferrerSuggestions) > 0
	return el("details", attrs(class("feed-http-settings")),
		el("summary", nil, text("HTTP Settings")),
		el("div", attrs(class("feed-http-settings-body")),
			el("div", attrs(class("form-group")),
				el("label", attrs(a("for", "custom_user_agent")), text("Custom User Agent")),
				text(" "),
				void("input", a("type", "text"), a("id", "custom_user_agent"), a("name", "custom_user_agent"),
					a("value", feed.CustomUserAgent), a("list", "user-agent-list"), a("autocomplete", "off")),
				el("datalist", attrs(a("id", "user-agent-list")),
					each(page.UserAgentSuggestions, func(ua string) node {
						return void("option", a("value", ua))
					})),
				el("div", attrs(class("feed-http-hint")), text("Leave empty to use the server's own user agent"))),
			el("div", attrs(class("form-group")),
				el("label", attrs(a("for", "custom_referrer")), text("Custom Referrer")),
				text(" "),
				void("input", a("type", "text"), a("id", "custom_referrer"), a("name", "custom_referrer"),
					a("value", feed.CustomReferrer), when(hasSuggestions, a("list", "referrer-list")),
					a("autocomplete", "off")),
				show(hasSuggestions, el("datalist", attrs(a("id", "referrer-list")),
					each(feed.ReferrerSuggestions, func(r string) node { return void("option", a("value", r)) }))),
				el("div", attrs(class("feed-http-hint")),
					text("Some image servers require a specific Referer header to serve images"))),
			el("div", attrs(class("form-group")),
				el("label", nil,
					text(" "),
					void("input", a("type", "checkbox"), a("id", "http2_disabled"), a("name", "http2_disabled"),
						when(feed.HTTP2Disabled, flag("checked"))),
					text(" Disable HTTP/2 ")),
				el("div", attrs(class("feed-http-hint")),
					text("Enable this if the feed server has HTTP/2 compatibility issues")))))
}

func refetchMetadataForm(page FeedEditPage) node {
	id := strconv.FormatInt(page.Feed.ID, 10)
	return el("form", attrs(a("method", "post"), a("action", "/feeds/"+id+"/fetch-metadata"),
		a("data-confirm", "Re-fetch metadata from the feed URL? This will overwrite current title/description/site URL.")),
		csrfField(page.CSRFToken), text(" "), hiddenReturnTo(page.ReturnTo), text(" "),
		el("button", attrs(a("type", "submit"), class("btn-secondary")), text("Re-fetch Metadata")))
}

// FeedsImportPage is the data of GET /feeds/import.
type FeedsImportPage struct {
	Layout    *Layout
	CSRFToken string
}

// FeedsImport renders the OPML import page.
func FeedsImport(page FeedsImportPage) string {
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "feeds"))),
		el("main", attrs(class("main-content")),
			flashMount(page.Layout.FlashMessages),
			el("div", attrs(class("page-content")),
				el("h1", nil, text("Import OPML")),
				el("form", attrs(a("method", "post"), a("action", "/feeds/import"), a("enctype", "multipart/form-data")),
					csrfField(page.CSRFToken), text(" "),
					el("div", attrs(class("form-group")),
						el("label", attrs(a("for", "file")), text("Upload .opml file")),
						text(" "),
						void("input", a("type", "file"), a("id", "file"), a("name", "file"), a("accept", ".opml,.xml"),
							a("data-testid", "opml-file-input"))),
					el("div", attrs(class("form-group")),
						el("label", attrs(a("for", "content")), text("Or paste OPML content")),
						text(" "),
						el("textarea", attrs(a("id", "content"), a("name", "content"), a("rows", "10"),
							class("textarea-full")))),
					el("div", attrs(class("modal-actions")),
						el("button", attrs(a("type", "submit"), a("data-testid", "opml-import-btn")), text("Import")),
						text(" "),
						el("a", attrs(a("href", "/feeds"), class("btn", "btn-secondary")), text("Cancel")))))))
	return string(appPage(page.Layout, "Import OPML", "", body))
}
