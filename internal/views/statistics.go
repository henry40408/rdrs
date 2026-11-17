package views

import "strconv"

// DailyBar is one bar of the daily-read chart; it may span several days.
type DailyBar struct {
	DateLabel     string
	Count         int64
	HeightPercent int
	ShortLabel    string
	// IsMax marks only the first busiest bar, so one count label is drawn.
	IsMax bool
}

// StatBar is a labelled row of a bar list. Detail is the right-hand figure.
type StatBar struct {
	Label        string
	Detail       string
	WidthPercent int
}

// SiteStats is the site-wide block shown to admins.
type SiteStats struct {
	TotalUsers, TotalFeeds, TotalEntries int64
	ReadRate                             string
}

// Reclaimable is the reclaimable-space card; SQLite only.
type Reclaimable struct {
	Size        string
	FragPercent int64
}

// DatabaseStats is the storage block shown to admins.
type DatabaseStats struct {
	Size           string
	Reclaimable    *Reclaimable
	TotalEntries   int64
	AvgPerDay      string
	Coverage       string
	TombstoneCount int64
}

// StatisticsPage is the /statistics page.
type StatisticsPage struct {
	Layout       *Layout
	ActivePeriod string
	CustomFrom   string
	CustomTo     string

	TotalEntries, ReadEntries, UnreadEntries, StarredEntries, Summaries int64
	ReadRate                                                            string

	DailyMax   int64
	Daily      []DailyBar
	Categories []StatBar
	TopFeeds   []StatBar

	OpenRates         []StatBar
	TrackedSince      *string
	MinTrackedForRate int64

	Site     *SiteStats
	Database *DatabaseStats
}

// Statistics renders the /statistics page.
func Statistics(page StatisticsPage) string {
	layout := page.Layout
	sections := []node{
		statsHeader(&page), overviewCards(&page), dailyChart(&page),
		el("div", attrs(class("stats-columns")),
			breakdown("Entries by Category", page.Categories), breakdown("Top Feeds", page.TopFeeds)),
	}
	if page.TrackedSince != nil {
		sections = append(sections, openRateSection(&page, *page.TrackedSince))
	}
	if page.Site != nil {
		sections = append(sections, siteSection(page.Site))
	}
	if page.Database != nil {
		sections = append(sections, databaseSection(page.Database))
	}
	body := el("div", attrs(class("app-layout")),
		el("rdrs-sidebar", attrs(a("active", "statistics"))),
		el("main", attrs(class("main-content")),
			flashMount(layout.FlashMessages),
			el("div", attrs(class("page-content")), group(sections...))))
	return string(appPage(layout, "Statistics", moduleScript(layout.GitVersion, "statistics.js"), body))
}

func periodLink(page *StatisticsPage, period, label string) node {
	return el("a", attrs(a("href", "/statistics?period="+period),
		class("stats-period-btn", onlyIf(page.ActivePeriod == period, "active"))), text(label))
}

func statsHeader(page *StatisticsPage) node {
	space := text(" ")
	return el("div", attrs(class("stats-header")),
		el("h1", nil, text("Statistics")),
		el("form", attrs(class("stats-period"), a("method", "get"), a("action", "/statistics")),
			periodLink(page, "7d", "7d"), space,
			periodLink(page, "30d", "30d"), space,
			periodLink(page, "90d", "90d"), space,
			periodLink(page, "all", "All"), space,
			el("span", attrs(class("stats-period-divider")), text("|")), space,
			void("input", a("type", "hidden"), a("name", "period"), a("value", "custom")), space,
			void("input", a("type", "date"), a("name", "from"), a("value", page.CustomFrom), class("stats-date-input")), space,
			el("span", attrs(class("stats-period-dash")), text("—")), space,
			void("input", a("type", "date"), a("name", "to"), a("value", page.CustomTo), class("stats-date-input")), space,
			el("button", attrs(a("type", "submit"), class("stats-period-btn")), text("Apply"))))
}

func statCard(value node, label string) node {
	return el("div", attrs(class("stats-card")), value, el("div", attrs(class("stats-card-label")), text(label)))
}

func cardValue(modifier string, value ...node) node {
	return el("div", attrs(class("stats-card-value", modifier)), group(value...))
}

func overviewCards(page *StatisticsPage) node {
	return el("div", attrs(class("stats-cards stats-cards--tiles")),
		statCard(cardValue("", num(page.TotalEntries)), "Total Entries"),
		statCard(cardValue("stats-card-success", num(page.ReadEntries)), "Read"),
		statCard(cardValue("stats-card-warning", num(page.UnreadEntries)), "Unread"),
		statCard(cardValue("", text(page.ReadRate), text("%")), "Read Rate"),
		statCard(cardValue("", num(page.StarredEntries)), "Starred"),
		statCard(cardValue("", num(page.Summaries)), "Summaries"))
}

func dailyChart(page *StatisticsPage) node {
	var chart node
	if page.DailyMax == 0 {
		chart = el("p", attrs(class("muted")), text("No read activity in this period"))
	} else {
		chart = el("div", attrs(class("stats-chart", onlyIf(len(page.Daily) > 10, "stats-chart--dense"))),
			each(page.Daily, dailyBar))
	}
	return el("div", attrs(class("stats-section")), el("h2", nil, text("Daily Read Articles")), chart)
}

func dailyBar(day DailyBar) node {
	count := num(day.Count)
	var value node
	if day.IsMax {
		value = el("span", attrs(class("stats-bar-value")), count)
	}
	return el("div", attrs(class("stats-bar-col"), a("tabindex", "0"), a("aria-label", day.DateLabel+": "+strconv.FormatInt(day.Count, 10)+" read")),
		el("div", attrs(class("stats-bar-tip"), a("role", "tooltip")), text(day.DateLabel), text(": "), el("strong", nil, count)),
		el("div", attrs(class("stats-bar", "pct-"+strconv.Itoa(day.HeightPercent))), value),
		el("div", attrs(class("stats-bar-label")), text(day.ShortLabel)))
}

func barRow(row StatBar) node {
	return el("div", attrs(class("stats-bar-row")),
		el("div", attrs(class("stats-bar-row-header")),
			el("span", nil, text(row.Label)), text(" "), el("span", attrs(class("muted")), text(row.Detail))),
		el("div", attrs(class("stats-progress")),
			el("div", attrs(class("stats-progress-fill", "pct-"+strconv.Itoa(row.WidthPercent))))))
}

func breakdown(heading string, rows []StatBar) node {
	content := el("p", attrs(class("empty-state-compact")), text("No entries in this period."))
	if len(rows) > 0 {
		content = each(rows, barRow)
	}
	return el("div", attrs(class("stats-section")), el("h2", nil, text(heading)), content)
}

func openRateSection(page *StatisticsPage, since string) node {
	content := el("p", attrs(class("empty-state-compact")), text("No feed has enough tracked entries yet."))
	if len(page.OpenRates) > 0 {
		content = each(page.OpenRates, barRow)
	}
	// Lowest first: the top of this list is what to unsubscribe from.
	return el("div", attrs(class("stats-section"), a("data-testid", "stats-open-rate")),
		el("h2", nil, text("Feeds by Open Rate")),
		el("p", attrs(class("muted text-xs")), text("How often a feed's entries actually get opened, lowest first. Tracked since "+since+
			"; feeds with fewer than "+strconv.FormatInt(page.MinTrackedForRate, 10)+" tracked entries are not listed.")),
		content)
}

func adminCard(value node, label string) node {
	return el("div", attrs(class("stats-card stats-card-admin")), value, el("div", attrs(class("stats-card-label")), text(label)))
}

func testValue(testID string, value node) node {
	return el("div", attrs(class("stats-card-value"), a("data-testid", testID)), value)
}

func siteSection(site *SiteStats) node {
	return el("div", attrs(class("stats-admin-section")),
		el("h2", nil, text("Site-wide Statistics")),
		el("div", attrs(class("stats-cards")),
			adminCard(cardValue("", num(site.TotalUsers)), "Total Users"),
			adminCard(testValue("stat-site-entries-total", num(site.TotalEntries)), "Site Entries · in period"),
			adminCard(testValue("stat-site-feeds-total", num(site.TotalFeeds)), "Total Feeds"),
			adminCard(cardValue("", text(site.ReadRate), text("%")), "Site Read Rate · in period")))
}

func databaseSection(db *DatabaseStats) node {
	var reclaimable node
	if db.Reclaimable != nil {
		reclaimable = adminCard(cardValue("", text(db.Reclaimable.Size)),
			"Reclaimable · "+strconv.FormatInt(db.Reclaimable.FragPercent, 10)+"% of file")
	}
	return el("div", attrs(class("stats-admin-section")),
		el("h2", nil, text("Database")),
		el("div", attrs(class("stats-cards stats-cards--db")),
			adminCard(cardValue("", text(db.Size)), "Database Size"),
			reclaimable,
			adminCard(testValue("stat-db-total-entries", num(db.TotalEntries)), "Total Entries"),
			adminCard(cardValue("", text(db.AvgPerDay)), "Avg New / Day · retained"),
			adminCard(cardValue("", text(db.Coverage)), "Coverage"),
			adminCard(testValue("stat-db-pruned", num(db.TombstoneCount)), "Pruned Entries")))
}
