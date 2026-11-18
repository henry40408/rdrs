package server

import (
	"context"
	"fmt"
	"log/slog"
	"math"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/timezone"
	"github.com/henry40408/rdrs/internal/views"
)

func (a *App) statisticsRoutes() []Route {
	return []Route{{"GET", "/statistics", a.statisticsPage}}
}

// maxDailyBars caps the bars so long ranges stay tappable on mobile.
const maxDailyBars = 14

// maxCustomRangeDays caps a custom range at a year.
const maxCustomRangeDays = 365

// earliestDay starts the "all" period: before any stored entry, and far from
// the zero time so shifting by a zone offset cannot overflow.
var earliestDay = timezone.Date{Year: 1900, Month: time.January, Day: 1}

// statisticsQuery is the query string of /statistics.
type statisticsQuery struct {
	period, from, to *string
}

func orDefault(value *string, fallback string) string {
	if value == nil {
		return fallback
	}
	return *value
}

// lenientDate matches chrono's %Y-%m-%d, which takes one or two digits for
// the month and day.
var lenientDate = regexp.MustCompile(`^(\d{4})-(\d{1,2})-(\d{1,2})$`)

func parseLenientDate(s *string) (timezone.Date, bool) {
	if s == nil {
		return timezone.Date{}, false
	}
	match := lenientDate.FindStringSubmatch(*s)
	if match == nil {
		return timezone.Date{}, false
	}
	return timezone.ParseDate(fmt.Sprintf("%s-%02s-%02s", match[1], match[2], match[3]))
}

// statisticsPeriod is the range the page covers: dates on the reader's own
// calendar, to exclusive, and the period that is active.
type statisticsPeriod struct {
	from, to timezone.Date
	active   string
}

// resolveStatisticsPeriod maps the query to a range ending on the reader's
// today. An invalid custom range falls back to 7d.
func resolveStatisticsPeriod(query statisticsQuery, today timezone.Date) statisticsPeriod {
	var period statisticsPeriod
	switch orDefault(query.period, "7d") {
	case "30d":
		period = statisticsPeriod{today.AddDays(-30), today, "30d"}
	case "90d":
		period = statisticsPeriod{today.AddDays(-90), today, "90d"}
	case "all":
		period = statisticsPeriod{earliestDay, today, "all"}
	case "custom":
		period = customPeriod(query, today)
	default:
		period = statisticsPeriod{today.AddDays(-7), today, "7d"}
	}
	period.to = period.to.AddDays(1)
	return period
}

func customPeriod(query statisticsQuery, today timezone.Date) statisticsPeriod {
	from, fromOK := parseLenientDate(query.from)
	to, toOK := parseLenientDate(query.to)
	if !fromOK || !toOK || to.Before(from) {
		return statisticsPeriod{today.AddDays(-7), today, "7d"}
	}
	if limit := from.AddDays(maxCustomRangeDays); limit.Before(to) {
		to = limit
	}
	return statisticsPeriod{from, to, "custom"}
}

// chartFrom is where the daily chart starts: "all" charts only the last 90 days.
func (p statisticsPeriod) chartFrom(today timezone.Date) timezone.Date {
	if p.active != "all" {
		return p.from
	}
	return today.AddDays(-90)
}

// barPercent is count as a whole percentage of largest. It is whole because it
// selects a pct-N class (CSP forbids inline style); non-zero counts floor at 1.
func barPercent(count, largest int64) int {
	if largest <= 0 || count <= 0 {
		return 0
	}
	percent := (count*100 + largest/2) / largest
	return int(min(max(percent, 1), 100))
}

// formatDBBytes writes a byte count in binary units, one decimal above 1 KB.
func formatDBBytes(bytes int64) string {
	const kb = 1024.0
	size := float64(bytes)
	switch {
	case size >= kb*kb*kb:
		return fmt.Sprintf("%.1f GB", size/(kb*kb*kb))
	case size >= kb*kb:
		return fmt.Sprintf("%.1f MB", size/(kb*kb))
	case size >= kb:
		return fmt.Sprintf("%.1f KB", size/kb)
	}
	return fmt.Sprintf("%d B", bytes)
}

func oneDecimal(value float64) string { return strconv.FormatFloat(value, 'f', 1, 64) }

// logStatsFailure keeps a broken query visible: the page degrades to an empty
// section, which would otherwise look like an empty database.
func logStatsFailure(what string, err error) bool {
	if err == nil {
		return false
	}
	slog.Error("statistics query failed", "event", "statistics.query_failed", "query", what, "error", err)
	return true
}

// statsRange is the bounds of the statistics queries.
type statsRange struct {
	period    statisticsPeriod
	chartFrom timezone.Date
	loc       *time.Location
}

// utcWindow is the period as the UTC instants the entry queries compare to.
func (r statsRange) utcWindow() (from, to string) {
	return models.UTCBound(r.period.from.String(), r.loc), models.UTCBound(r.period.to.String(), r.loc)
}

func (a *App) statisticsPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	reader := queryReader{values: r.URL.Query()}
	query := statisticsQuery{period: reader.text("period"), from: reader.text("from"), to: reader.text("to")}
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return
	}
	ctx, userID := r.Context(), principal.User.ID
	loc, err := models.GetTimezone(ctx, a.DB, userID)
	if logStatsFailure("timezone", err) || loc == nil {
		loc = time.UTC
	}
	today := timezone.DateAt(a.Now(), loc)
	period := resolveStatisticsPeriod(query, today)
	page := views.StatisticsPage{ActivePeriod: period.active, MinTrackedForRate: models.MinTrackedForRate}
	if period.active == "custom" {
		page.CustomFrom, page.CustomTo = orDefault(query.from, ""), orDefault(query.to, "")
	}
	rng := statsRange{period: period, chartFrom: period.chartFrom(today), loc: loc}
	a.fillPersonalStats(ctx, &page, userID, rng)
	page.OpenRates = openRateBars(a.openRates(ctx, userID))
	page.TrackedSince = a.trackedSince(ctx, userID, loc)
	if showsAdminStats(principal) {
		page.Site = a.siteStats(ctx, rng)
		page.Database = a.databaseStats(ctx)
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	page.Layout = &layout
	writeHTML(w, http.StatusOK, views.Statistics(page))
}

// showsAdminStats is true for admins, except while masquerading as another user.
func showsAdminStats(principal *auth.Principal) bool {
	return !principal.Session.IsMasquerading() && principal.User.IsAdmin()
}

func (a *App) fillPersonalStats(ctx context.Context, page *views.StatisticsPage, userID int64, rng statsRange) {
	from, to := rng.utcWindow()
	overview, err := models.GetPersonalOverview(ctx, a.DB, userID, from, to)
	logStatsFailure("overview", err)
	page.TotalEntries, page.ReadEntries, page.UnreadEntries = overview.TotalEntries, overview.ReadEntries, overview.UnreadEntries()
	page.StarredEntries, page.Summaries, page.ReadRate = overview.StarredEntries, overview.Summaries, oneDecimal(overview.ReadRate())

	daily, err := models.GetDailyReadCounts(ctx, a.DB, models.DailyReadQuery{
		UserID: userID, From: rng.chartFrom.String(), To: rng.period.to.String(), Loc: rng.loc,
	})
	logStatsFailure("daily_read_counts", err)
	page.DailyMax, page.Daily = dailyBars(daily)

	categories, err := models.GetEntriesByCategory(ctx, a.DB, userID, from, to)
	logStatsFailure("entries_by_category", err)
	page.Categories = categoryBars(categories)

	feeds, err := models.GetTopFeeds(ctx, a.DB, models.TopFeedsQuery{UserID: userID, From: from, To: to, Limit: 10})
	logStatsFailure("top_feeds", err)
	page.TopFeeds = feedBars(feeds)
}

// dailyBars returns the largest bucket, taken over buckets rather than days
// so bars scale to what is drawn, and the bars.
func dailyBars(daily []models.DailyReadCount) (int64, []views.DailyBar) {
	buckets := models.BucketDailyCounts(daily, maxDailyBars)
	var dailyMax int64
	for _, bucket := range buckets {
		dailyMax = max(dailyMax, bucket.Count)
	}
	maxIndex := -1
	if dailyMax > 0 {
		maxIndex = slices.IndexFunc(buckets, func(b models.DailyBucket) bool { return b.Count == dailyMax })
	}
	bars := make([]views.DailyBar, len(buckets))
	for i, bucket := range buckets {
		label := bucket.Start.String()
		if bucket.Start != bucket.End {
			label += " – " + shortDate(bucket.End)
		}
		bars[i] = views.DailyBar{
			DateLabel: label, Count: bucket.Count, HeightPercent: barPercent(bucket.Count, dailyMax),
			ShortLabel: shortDate(bucket.Start), IsMax: i == maxIndex,
		}
	}
	return dailyMax, bars
}

func shortDate(date timezone.Date) string {
	return fmt.Sprintf("%02d/%02d", int(date.Month), date.Day)
}

func categoryBars(categories []models.CategoryCount) []views.StatBar {
	var largest int64
	for _, c := range categories {
		largest = max(largest, c.Count)
	}
	bars := make([]views.StatBar, len(categories))
	for i, c := range categories {
		bars[i] = views.StatBar{Label: c.Name, Detail: strconv.FormatInt(c.Count, 10), WidthPercent: barPercent(c.Count, largest)}
	}
	return bars
}

func feedBars(feeds []models.FeedCount) []views.StatBar {
	var largest int64
	for _, f := range feeds {
		largest = max(largest, f.Count)
	}
	bars := make([]views.StatBar, len(feeds))
	for i, f := range feeds {
		bars[i] = views.StatBar{Label: f.Title, Detail: strconv.FormatInt(f.Count, 10), WidthPercent: barPercent(f.Count, largest)}
	}
	return bars
}

// openRates ignores the period: the open rate only means something since opt-in.
func (a *App) openRates(ctx context.Context, userID int64) []models.FeedOpenRate {
	rates, err := models.OpenRatesByFeed(ctx, a.DB, userID)
	logStatsFailure("open_rates", err)
	return rates
}

// openRateBars lists the ten lowest rates first; feeds below the tracking
// minimum have no rate and are left out.
func openRateBars(rates []models.FeedOpenRate) []views.StatBar {
	type rated struct {
		title   string
		percent int64
		rate    models.FeedOpenRate
	}
	var kept []rated
	for _, rate := range rates {
		percent, ok := rate.Percent()
		if !ok {
			continue
		}
		title := "(untitled feed)"
		if rate.Title != nil {
			title = *rate.Title
		}
		kept = append(kept, rated{title, percent, rate})
	}
	slices.SortStableFunc(kept, func(x, y rated) int {
		if x.percent != y.percent {
			return int(x.percent - y.percent)
		}
		return strings.Compare(strings.ToLower(x.title), strings.ToLower(y.title))
	})
	kept = kept[:min(len(kept), 10)]
	bars := make([]views.StatBar, len(kept))
	for i, row := range kept {
		bars[i] = views.StatBar{
			Label:        row.title,
			Detail:       fmt.Sprintf("%d%% (%d/%d)", row.percent, row.rate.Opened, row.rate.Tracked),
			WidthPercent: int(min(max(row.percent, 0), 100)),
		}
	}
	return bars
}

func (a *App) trackedSince(ctx context.Context, userID int64, loc *time.Location) *string {
	window, err := models.FindTrackingWindow(ctx, a.DB, userID)
	logStatsFailure("tracking_window", err)
	since := window.TrackedSince()
	if since == nil {
		return nil
	}
	formatted := since.In(loc).Format(timezone.DateLayout)
	return &formatted
}

// siteStats is nil when either query fails, so the block is left out.
func (a *App) siteStats(ctx context.Context, rng statsRange) *views.SiteStats {
	from, to := rng.utcWindow()
	counts, countsErr := models.GetAdminCounts(ctx, a.DB)
	entries, entriesErr := models.GetAdminEntryStats(ctx, a.DB, from, to)
	failed := logStatsFailure("admin_counts", countsErr)
	if logStatsFailure("admin_entry_stats", entriesErr) || failed {
		return nil
	}
	return &views.SiteStats{
		TotalUsers: counts.TotalUsers, TotalFeeds: counts.TotalFeeds,
		TotalEntries: entries.TotalEntries, ReadRate: oneDecimal(entries.ReadRate()),
	}
}

// databaseStats is a plain read-through: a concurrent miss recomputes harmlessly.
func (a *App) databaseStats(ctx context.Context) *views.DatabaseStats {
	stats, ok := a.AdminDBStats.Get()
	if !ok {
		fresh, err := models.GetAdminDatabaseStats(ctx, a.DB)
		if logStatsFailure("admin_database_stats", err) {
			return nil
		}
		stats = fresh
		a.AdminDBStats.Put(stats)
	}
	view := &views.DatabaseStats{
		Size: formatDBBytes(stats.DBSizeBytes), TotalEntries: stats.TotalEntries,
		AvgPerDay:      strconv.FormatInt(int64(math.Round(stats.AvgNewEntriesPerDay)), 10),
		Coverage:       strconv.FormatInt(int64(math.Round(stats.CoverageDays)), 10) + "d",
		TombstoneCount: stats.TombstoneCount,
	}
	if stats.Reclaimable != nil {
		view.Reclaimable = &views.Reclaimable{
			Size: formatDBBytes(stats.Reclaimable.Bytes), FragPercent: int64(math.Round(stats.Reclaimable.FragmentationRatio * 100)),
		}
	}
	return view
}
