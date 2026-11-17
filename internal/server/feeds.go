package server

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/discovery"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/opml"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/views"
)

// feedRoutes are the feed pages, their no-JS form actions and the icon.
func (a *App) feedRoutes() []Route {
	return []Route{
		{"GET", "/feeds", a.feedsPage},
		{"POST", "/feeds", a.createFeedForm},
		{"GET", "/feeds/import", a.feedsImportPage},
		{"POST", "/feeds/import", a.importOPMLForm},
		{"GET", "/feeds/{id}/edit", a.feedEditPage},
		{"POST", "/feeds/{id}/edit", a.editFeedForm},
		{"POST", "/feeds/{id}/delete", a.deleteFeedForm},
		{"POST", "/feeds/{id}/refresh", a.refreshFeedForm},
		{"POST", "/feeds/{id}/fetch-metadata", a.fetchMetadataForm},
		{"GET", "/api/feeds/{id}/icon", a.feedIcon},
	}
}

// --- GET /feeds ---

// normalizeSort and normalizeFilter turn unknown values into the default, so a
// crafted value never reaches the links built from it.
func normalizeSort(raw string) string {
	switch raw {
	case "unread", "category", "open_rate":
		return raw
	}
	return "title"
}

func normalizeFilter(raw string) string {
	switch raw {
	case "errors", "stale":
		return raw
	}
	return "all"
}

// feedsURL builds every link to the /feeds view, so each value is encoded.
func feedsURL(category int64, sort, filter string) string {
	query := url.Values{}
	var parts []string
	if category != 0 {
		parts = append(parts, "category="+strconv.FormatInt(category, 10))
	}
	query.Set("sort", sort)
	query.Set("filter", filter)
	parts = append(parts, "sort="+url.QueryEscape(sort), "filter="+url.QueryEscape(filter))
	return "/feeds?" + strings.Join(parts, "&")
}

// categoryFilter is the category query value; empty or non-numeric means all.
func categoryFilter(raw string) int64 {
	id, err := strconv.ParseInt(raw, 10, 64)
	if err != nil {
		return 0
	}
	return id
}

// feedRowFor builds one table row.
func feedRowFor(feed models.Feed, lookups rowLookups, now time.Time) views.FeedRow {
	fetched, fetchedAt := relativeTime(feed.FetchedAt, now)
	updated, updatedAt := feedUpdatedDisplay(feed, now)
	class, key := freshness(feed.FeedUpdatedAt, feed.FetchedAt, now)
	row := views.FeedRow{
		ID: feed.ID, URL: feed.URL, Title: feed.URL, CategoryID: feed.CategoryID,
		CategoryName: "Unknown", HasIcon: lookups.withIcon[feed.ID], UnreadCount: lookups.unread[feed.ID],
		FetchedAtRelative: fetched, FetchedAtDatetime: fetchedAt,
		FeedUpdatedRelative: updated, FeedUpdatedDatetime: updatedAt,
		FreshnessClass: class, FreshnessKey: key,
	}
	if feed.Title != nil {
		row.Title = *feed.Title
	}
	if name, ok := lookups.categoryNames[feed.CategoryID]; ok {
		row.CategoryName = name
	}
	if feed.FetchError != nil {
		row.HasFetchError, row.FetchError = true, *feed.FetchError
	}
	if rate, ok := lookups.openRates[feed.ID]; ok {
		if percent, enough := rate.Percent(); enough {
			row.HasOpenRatePercent, row.OpenRatePercent = true, percent
			// Raw counts: "40%" alone hides the sample size.
			row.HasOpenRateLabel = true
			row.OpenRateLabel = fmt.Sprintf("%d%% (%d/%d)", percent, rate.Opened, rate.Tracked)
		}
	}
	return row
}

// feedUpdatedDisplay is the "Updated" text. A feed fetched recently that
// carries no date says so, instead of looking abandoned.
func feedUpdatedDisplay(feed models.Feed, now time.Time) (relative, datetime string) {
	if feed.FeedUpdatedAt != nil {
		return relativeTime(feed.FeedUpdatedAt, now)
	}
	if feed.FetchedAt != nil && wholeDays(now.Sub(*feed.FetchedAt)) <= freshMaxDays {
		return "No date info", ""
	}
	return "Never", ""
}

// rowLookups is what the row builder looks feeds up in, fetched once.
type rowLookups struct {
	categoryNames map[int64]string
	withIcon      map[int64]bool
	openRates     map[int64]models.FeedOpenRate
	unread        map[int64]int64
}

// feedsListing is the table rows and the category options of /feeds.
type feedsListing struct {
	rows       []views.FeedRow
	categories []views.FeedCategoryOption
	total      int64
}

// listFeeds reads the rows; a failing query degrades to an empty list.
func (a *App) listFeeds(r *http.Request, userID int64, unread map[int64]int64) feedsListing {
	ctx := r.Context()
	categories, _ := models.ListCategoriesByUser(ctx, a.DB, userID)
	feeds, _ := models.ListFeedsByUser(ctx, a.DB, userID)
	rates, _ := models.OpenRatesByFeed(ctx, a.DB, userID)
	lookups := rowLookups{
		categoryNames: map[int64]string{}, openRates: map[int64]models.FeedOpenRate{}, unread: unread,
	}
	for _, rate := range rates {
		lookups.openRates[rate.FeedID] = rate
	}
	countByCategory := map[int64]int64{}
	ids := make([]int64, 0, len(feeds))
	for _, feed := range feeds {
		countByCategory[feed.CategoryID]++
		ids = append(ids, feed.ID)
	}
	lookups.withIcon, _ = models.ExistingImageIDs(ctx, a.DB, models.EntityFeed, ids)
	listing := feedsListing{total: int64(len(feeds))}
	now := a.Now()
	for _, feed := range feeds {
		listing.rows = append(listing.rows, feedRowFor(feed, lookups, now))
	}
	for _, category := range categories {
		lookups.categoryNames[category.ID] = category.Name
		listing.categories = append(listing.categories, views.FeedCategoryOption{
			ID: category.ID, Name: category.Name, FeedCount: countByCategory[category.ID],
		})
	}
	// Names are looked up per row above, so fill them now that they are known.
	for i := range listing.rows {
		if name, ok := lookups.categoryNames[listing.rows[i].CategoryID]; ok {
			listing.rows[i].CategoryName = name
		}
	}
	return listing
}

// filterAndSortRows filters first, then sorts (stably, as Rust did).
func filterAndSortRows(rows []views.FeedRow, category int64, filter, order string) []views.FeedRow {
	kept := rows[:0:0]
	for _, row := range rows {
		switch {
		case category != 0 && row.CategoryID != category:
		case filter == "errors" && !row.HasFetchError:
		case filter == "stale" && row.FreshnessKey != "stale":
		default:
			kept = append(kept, row)
		}
	}
	sort.SliceStable(kept, func(i, j int) bool { return rowLess(kept[i], kept[j], order) })
	return kept
}

func rowLess(x, y views.FeedRow, order string) bool {
	switch order {
	case "unread":
		return x.UnreadCount > y.UnreadCount
	case "category":
		return x.CategoryName < y.CategoryName
	case "open_rate":
		// Least opened first; feeds without a rate last.
		if x.HasOpenRatePercent != y.HasOpenRatePercent {
			return x.HasOpenRatePercent
		}
		return x.OpenRatePercent < y.OpenRatePercent
	}
	return strings.ToLower(x.Title) < strings.ToLower(y.Title)
}

func filterLinks(category int64, order, active string) []views.FeedFilterLink {
	var links []views.FeedFilterLink
	for _, pill := range [][2]string{{"All", "all"}, {"Errors", "errors"}, {"Stale", "stale"}} {
		links = append(links, views.FeedFilterLink{
			Label: pill[0], Href: feedsURL(category, order, pill[1]), Active: active == pill[1],
		})
	}
	return links
}

// feedsPage is GET /feeds.
func (a *App) feedsPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	userID := principal.User.ID
	// Gated on the setting, so an opted-in reader sees the column even when empty.
	enabledAt, _ := models.PixelTrackingEnabledAt(r.Context(), a.DB, userID)
	listing := a.listFeeds(r, userID, layout.UnreadByFeed)
	query := r.URL.Query()
	filter, order := normalizeFilter(query.Get("filter")), normalizeSort(query.Get("sort"))
	category := categoryFilter(query.Get("category"))
	returnTo := feedsURL(category, order, filter)
	writeHTML(w, http.StatusOK, views.Feeds(views.FeedsPage{
		Layout: &layout, CSRFToken: principal.CSRFToken,
		Feeds:      filterAndSortRows(listing.rows, category, filter, order),
		Categories: listing.categories, TotalFeedCount: listing.total,
		ActiveFilter: filter, ActiveSort: order, ActiveCategoryID: category,
		FilterLinks: filterLinks(category, order, filter), ReturnTo: returnTo, EditQuery: ReturnToQuery(returnTo),
		FreshMaxDays: freshMaxDays, WarningMaxDays: warningMaxDays,
		OpenRateShown: enabledAt != nil, MinTrackedForRate: models.MinTrackedForRate,
	}))
}

// --- GET /feeds/{id}/edit, GET /feeds/import ---

// referrerSuggestions are the deduplicated http(s) origins to offer as
// Referer: the site first (it serves the embedded images), then the feed's.
func referrerSuggestions(siteURL *string, feedURL string) []string {
	var out []string
	candidates := []string{feedURL}
	if siteURL != nil {
		candidates = []string{*siteURL, feedURL}
	}
	for _, candidate := range candidates {
		parsed, err := url.Parse(strings.TrimSpace(candidate))
		if err != nil || (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.Hostname() == "" {
			continue
		}
		origin := parsed.Scheme + "://" + parsed.Host + "/"
		if !contains(out, origin) {
			out = append(out, origin)
		}
	}
	return out
}

func contains(list []string, want string) bool {
	for _, item := range list {
		if item == want {
			return true
		}
	}
	return false
}

func valueOr(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

// ownedFeed finds a feed the user owns: another user's feed is not found.
func (a *App) ownedFeed(r *http.Request, id, userID int64) (*models.Feed, error) {
	feed, err := models.FindFeedByID(r.Context(), a.DB, id)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if feed == nil {
		return nil, ErrFeedNotFound
	}
	owner, err := models.FindCategoryByIDAndUser(r.Context(), a.DB, feed.CategoryID, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if owner == nil {
		return nil, ErrFeedNotFound
	}
	return feed, nil
}

// feedEditPage is GET /feeds/{id}/edit.
func (a *App) feedEditPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	returnTo := FeedsListReturn(r.URL.Query().Get("return_to"))
	feed, err := a.ownedFeed(r, id, userID)
	if errors.Is(err, ErrFeedNotFound) {
		a.renderNotFound(w, r, principal, "Feed not found", "This feed doesn't exist or you don't have access to it.")
		return
	}
	if err != nil {
		writeError(w, err)
		return
	}
	categories, err := models.ListCategoriesByUser(r.Context(), a.DB, userID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	options := make([]views.FeedCategoryOption, 0, len(categories))
	for _, category := range categories {
		options = append(options, views.FeedCategoryOption{ID: category.ID, Name: category.Name})
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusOK, views.FeedEditView(views.FeedEditPage{
		Layout: &layout, CSRFToken: principal.CSRFToken, Categories: options, ReturnTo: returnTo,
		UserAgentSuggestions: config.CustomUserAgentSuggestions,
		Feed: views.FeedEdit{
			ID: feed.ID, URL: feed.URL, Title: valueOr(feed.Title), Description: valueOr(feed.Description),
			SiteURL: valueOr(feed.SiteURL), CategoryID: feed.CategoryID,
			CustomUserAgent: valueOr(feed.CustomUserAgent), HTTP2Disabled: feed.HTTP2Disabled,
			CustomReferrer: valueOr(feed.CustomReferrer), ReferrerSuggestions: referrerSuggestions(feed.SiteURL, feed.URL),
		},
	}))
}

// renderNotFound answers the chrome-wrapped 404 of logged-in pages.
func (a *App) renderNotFound(w http.ResponseWriter, r *http.Request, principal *auth.Principal, heading, message string) {
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusNotFound, views.Error(views.ErrorPage{Layout: &layout, Heading: heading, Message: message}))
}

// feedsImportPage is GET /feeds/import.
func (a *App) feedsImportPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	layout := a.buildLayout(r, principal, flash.Take(w, r))
	writeHTML(w, http.StatusOK, views.FeedsImport(views.FeedsImportPage{Layout: &layout, CSRFToken: principal.CSRFToken}))
}

// --- form actions ---

// returnToOf is the return_to of a form that has no other field. A POST
// without a usable form body (an API client, a test) lands on the bare list.
func returnToOf(r *http.Request) string {
	form, ok := optionalForm(r)
	if !ok {
		return FeedsList
	}
	return FeedsListReturn(form.Get("return_to"))
}

// editPath is /feeds/{id}/edit, carrying back unless it is the bare list.
func editPath(id int64, back string) string {
	path := "/feeds/" + strconv.FormatInt(id, 10) + "/edit"
	if back == FeedsList {
		return path
	}
	return path + "?" + ReturnToQuery(back)
}

// formInt64 reads a required integer field, answering 422 like serde does.
func formInt64(w http.ResponseWriter, form url.Values, name string) (int64, bool) {
	if !requireFields(w, form, name) {
		return 0, false
	}
	value, err := strconv.ParseInt(form.Get(name), 10, 64)
	if err != nil {
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize form body: "+name+": invalid digit found in string")
		return 0, false
	}
	return value, true
}

// createFeedForm is POST /feeds.
func (a *App) createFeedForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "url") {
		return
	}
	categoryID, ok := formInt64(w, form, "category_id")
	if !ok {
		return
	}
	back := FeedsListReturn(form.Get("return_to"))
	feedURL := strings.TrimSpace(form.Get("url"))
	if feedURL == "" {
		flash.Redirect(w, back, flash.NewError("Feed URL cannot be empty"))
		return
	}
	userID := principal.User.ID
	owner, err := models.FindCategoryByIDAndUser(r.Context(), a.DB, categoryID, userID)
	if err != nil || owner == nil {
		flash.Redirect(w, back, flash.NewError("Invalid category"))
		return
	}
	discovered, err := discovery.Discover(r.Context(), discovery.Params{
		URL: feedURL, UserAgent: a.Config.UserAgent, Fetcher: a.Fetcher, Retry: fetch.DefaultRetry(),
	})
	if err != nil {
		flash.Redirect(w, back, flash.NewError("Failed to discover feed: "+err.Error()))
		return
	}
	message := a.subscribe(r, userID, categoryID, discovered)
	if message.Level == flash.Success {
		a.Sidebar.Bust(userID)
	}
	flash.Redirect(w, back, message)
}

// subscribe stores a discovered feed and says how it went.
func (a *App) subscribe(r *http.Request, userID, categoryID int64, discovered *discovery.Feed) flash.Message {
	existing, err := models.FindFeedByURLForUser(r.Context(), a.DB, discovered.FeedURL, userID)
	if err != nil {
		return flash.NewError("Failed to add feed")
	}
	if existing != nil {
		return flash.NewError("Feed already subscribed")
	}
	_, err = models.CreateFeed(r.Context(), a.DB, models.CreateFeedParams{
		CategoryID: categoryID, URL: discovered.FeedURL, Title: discovered.Title,
		Description: discovered.Description, SiteURL: discovered.SiteURL,
	})
	var validation *models.ValidationError
	switch {
	case err == nil:
		return flash.NewSuccess("Feed added.")
	case errors.Is(err, models.ErrFeedExists):
		return flash.NewError("Feed already subscribed")
	case errors.As(err, &validation):
		return flash.NewError(validation.Message)
	}
	return flash.NewError("Failed to add feed")
}

// resolveOptionalField: absent keeps stored, blank clears, anything else wins
// after trimming.
func resolveOptionalField(form url.Values, name string, stored *string) *string {
	if !form.Has(name) {
		return stored
	}
	trimmed := strings.TrimSpace(form.Get(name))
	if trimmed == "" {
		return nil
	}
	return &trimmed
}

// editFeedForm is POST /feeds/{id}/edit.
func (a *App) editFeedForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "url") {
		return
	}
	newCategoryID, ok := formInt64(w, form, "category_id")
	if !ok {
		return
	}
	// Success returns to the list; errors stay here, still carrying back.
	back := FeedsListReturn(form.Get("return_to"))
	failHere := editPath(id, back)
	newURL := strings.TrimSpace(form.Get("url"))
	if newURL == "" {
		flash.Redirect(w, failHere, flash.NewError("Feed URL cannot be empty"))
		return
	}
	// The sync worker will fetch this URL.
	if parsed, err := url.Parse(newURL); err != nil || a.Fetcher.Validate(parsed) != nil {
		flash.Redirect(w, failHere, flash.NewError(
			"Feed URL must be an http(s) address that does not point to a private or local host"))
		return
	}
	userID := principal.User.ID
	if err := a.updateFeed(r, id, userID, newCategoryID, newURL, form); err != nil {
		var validation *models.ValidationError
		if errors.As(err, &validation) {
			flash.Redirect(w, failHere, flash.NewError(validation.Message))
			return
		}
		flash.Redirect(w, failHere, flash.NewError("Failed to update feed"))
		return
	}
	a.Sidebar.Bust(userID)
	flash.Redirect(w, back, flash.NewSuccess("Feed updated."))
}

// updateFeed applies the edit form to a feed the user owns.
func (a *App) updateFeed(r *http.Request, id, userID, newCategoryID int64, newURL string, form url.Values) error {
	feed, err := a.ownedFeed(r, id, userID)
	if err != nil {
		return err
	}
	target, err := models.FindCategoryByIDAndUser(r.Context(), a.DB, newCategoryID, userID)
	if err != nil {
		return err
	}
	if target == nil {
		return ErrCategoryNotFound
	}
	title := feed.Title
	if trimmed := strings.TrimSpace(form.Get("title")); trimmed != "" {
		title = &trimmed
	}
	_, err = models.UpdateFeed(r.Context(), a.DB, models.UpdateFeedParams{
		ID: feed.ID, CategoryID: feed.CategoryID, NewCategoryID: newCategoryID, URL: newURL, Title: title,
		Description:     resolveOptionalField(form, "description", feed.Description),
		SiteURL:         resolveOptionalField(form, "site_url", feed.SiteURL),
		CustomUserAgent: resolveOptionalField(form, "custom_user_agent", feed.CustomUserAgent),
		HTTP2Disabled:   form.Has("http2_disabled"),
		CustomReferrer:  resolveOptionalField(form, "custom_referrer", feed.CustomReferrer),
	})
	return err
}

// deleteFeedForm is POST /feeds/{id}/delete.
func (a *App) deleteFeedForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	back := returnToOf(r)
	userID := principal.User.ID
	feed, err := a.ownedFeed(r, id, userID)
	if errors.Is(err, ErrFeedNotFound) {
		flash.Redirect(w, back, flash.NewError("Feed not found."))
		return
	}
	if err == nil {
		err = models.DeleteFeed(r.Context(), a.DB, feed.ID, feed.CategoryID)
	}
	if err != nil {
		flash.Redirect(w, back, flash.NewError("Failed to delete feed."))
		return
	}
	a.Sidebar.Bust(userID)
	flash.Redirect(w, back, flash.NewSuccess("Feed deleted."))
}

// refreshFeedForm is POST /feeds/{id}/refresh.
func (a *App) refreshFeedForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	back := returnToOf(r)
	userID := principal.User.ID
	if _, err := a.ownedFeed(r, id, userID); err != nil {
		flash.Redirect(w, back, flash.NewError("Feed not found"))
		return
	}
	result, err := a.Syncer.Refresh(r.Context(), id)
	if err != nil {
		flash.Redirect(w, back, flash.NewError("Refresh failed: "+err.Error()))
		return
	}
	if result.NewEntries > 0 || result.UpdatedEntries > 0 {
		a.Sidebar.Bust(userID)
	}
	flash.Redirect(w, back, flash.NewSuccess(
		fmt.Sprintf("Refreshed: %d new, %d updated.", result.NewEntries, result.UpdatedEntries)))
}

// fetchMetadataForm is POST /feeds/{id}/fetch-metadata.
func (a *App) fetchMetadataForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	back := returnToOf(r)
	failHere := editPath(id, back)
	feed, err := a.ownedFeed(r, id, principal.User.ID)
	if err != nil {
		// No edit page to return to.
		flash.Redirect(w, back, flash.NewError("Feed not found"))
		return
	}
	discovered, err := discovery.Discover(r.Context(), discovery.Params{
		URL: feed.URL, UserAgent: a.Config.UserAgent, Fetcher: a.Fetcher, Retry: fetch.DefaultRetry(),
	})
	if err != nil {
		flash.Redirect(w, failHere, flash.NewError("Failed to fetch metadata: "+err.Error()))
		return
	}
	_, err = models.UpdateFeed(r.Context(), a.DB, models.UpdateFeedParams{
		ID: feed.ID, CategoryID: feed.CategoryID, NewCategoryID: feed.CategoryID, URL: feed.URL,
		Title: firstNonNil(discovered.Title, feed.Title), Description: firstNonNil(discovered.Description, feed.Description),
		SiteURL: firstNonNil(discovered.SiteURL, feed.SiteURL), CustomUserAgent: feed.CustomUserAgent,
		HTTP2Disabled: feed.HTTP2Disabled, CustomReferrer: feed.CustomReferrer,
	})
	if err != nil {
		flash.Redirect(w, failHere, flash.NewError("Failed to update feed"))
		return
	}
	flash.Redirect(w, failHere, flash.NewSuccess("Metadata fetched."))
}

func firstNonNil(first, second *string) *string {
	if first != nil {
		return first
	}
	return second
}

// --- OPML import ---

// opmlUpload is what the import form carried.
type opmlUpload struct {
	content   string
	csrfField string
}

// readOPMLUpload reads every part so the _csrf field is found anywhere; the
// first non-blank file or content part wins.
func readOPMLUpload(r *http.Request) (opmlUpload, bool) {
	reader, err := r.MultipartReader()
	if err != nil {
		return opmlUpload{}, false
	}
	var upload opmlUpload
	for {
		part, err := reader.NextPart()
		if err != nil {
			return upload, errors.Is(err, io.EOF) || upload.content != "" || upload.csrfField != ""
		}
		name := part.FormName()
		if name != "file" && name != "content" && name != "_csrf" {
			continue
		}
		data, err := io.ReadAll(part)
		if err != nil || !utf8.Valid(data) {
			continue
		}
		upload.accept(name, string(data))
	}
}

func (u *opmlUpload) accept(name, text string) {
	if name == "_csrf" {
		if token := strings.TrimSpace(text); token != "" {
			u.csrfField = token
		}
		return
	}
	if strings.TrimSpace(u.content) == "" && strings.TrimSpace(text) != "" {
		u.content = text
	}
}

// importOPMLForm is POST /feeds/import. Multipart bodies bypass the CSRF guard,
// so the token is checked here: the _csrf field or the X-CSRF-Token header.
// This is the one handler that reads its own body.
func (a *App) importOPMLForm(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, maxBodyBytes)
	upload, ok := readOPMLUpload(r)
	if !ok {
		rejection(w, http.StatusUnsupportedMediaType, "Multipart form requests must have `Content-Type: multipart/form-data`")
		return
	}
	token := strings.TrimSpace(r.Header.Get("X-CSRF-Token"))
	if upload.csrfField != "" {
		token = upload.csrfField
	}
	if !secret.VerifyCSRF(a.Config.Secret, a.cookies.SessionToken(r), token) {
		w.WriteHeader(http.StatusForbidden)
		return
	}
	if strings.TrimSpace(upload.content) == "" {
		flash.Redirect(w, "/feeds/import", flash.NewError("Please upload a file or paste OPML content"))
		return
	}
	outlines, err := opml.Parse(upload.content)
	if err != nil {
		flash.Redirect(w, "/feeds/import", flash.NewError("Failed to parse OPML: "+err.Error()))
		return
	}
	userID := principal.User.ID
	summary, err := opml.Import(r.Context(), opml.ImportParams{DB: a.DB, UserID: userID, Outlines: outlines, Fetcher: a.Fetcher})
	if err != nil {
		flash.Redirect(w, "/feeds/import", flash.NewError("Failed to import OPML"))
		return
	}
	a.Sidebar.Bust(userID)
	flash.Redirect(w, "/feeds", flash.NewSuccess(summary.Describe()))
}

// --- GET /api/feeds/{id}/icon ---

// feedIcon is GET /api/feeds/{id}/icon.
func (a *App) feedIcon(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	id, ok := pathID(w, r)
	if !ok {
		return
	}
	if _, err := a.ownedFeed(r, id, principal.User.ID); err != nil {
		writeError(w, err)
		return
	}
	image, err := models.FindImage(r.Context(), a.DB, models.EntityFeed, id)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	if image == nil {
		writeError(w, NotFoundError("Icon not found"))
		return
	}
	// private, not public: the response is per user and carries no Vary: Cookie.
	w.Header().Set("Content-Type", image.ContentType)
	w.Header().Set("Cache-Control", "private, max-age=86400")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(image.Data)
}
