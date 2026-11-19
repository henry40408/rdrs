package server

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strings"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/timezone"
	"github.com/henry40408/rdrs/internal/views"
)

// sidebarCategory is one category row of the sidebar JSON.
type sidebarCategory struct {
	ID          int64  `json:"id"`
	Name        string `json:"name"`
	UnreadCount int64  `json:"unread_count"`
}

// sidebarResponse is the sidebar payload, shared by the JSON API and the
// inline page embed.
type sidebarResponse struct {
	Username        string            `json:"username"`
	IsAdmin         bool              `json:"is_admin"`
	IsMasquerading  bool              `json:"is_masquerading"`
	Categories      []sidebarCategory `json:"categories"`
	TotalUnread     int64             `json:"total_unread"`
	TotalSummarized int64             `json:"total_summarized"`
	// TotalToday counts unread entries published today in the user's timezone.
	TotalToday     int64 `json:"total_today"`
	ViaForwardAuth bool  `json:"via_forward_auth"`
	// SidebarSort lists are sent in name order and the client re-orders.
	SidebarSort     string `json:"sidebar_sort"`
	SidebarHideRead bool   `json:"sidebar_hide_read"`
}

// chromeData is the per-page data around every authenticated render.
type chromeData struct {
	cache.Chrome
	// originalUserIsAdmin is set only while masquerading.
	originalUserIsAdmin bool
}

// readChrome returns the user's chrome through the per-user sidebar cache.
// A failing query degrades to an empty value: the chrome must never fail a
// page. The original-user lookup is session specific and never cached.
func (a *App) readChrome(ctx context.Context, userID int64, originalUserID *int64) chromeData {
	original := false
	if originalUserID != nil {
		if user, err := models.FindUserByID(ctx, a.DB, *originalUserID); err == nil && user != nil {
			original = user.IsAdmin()
		}
	}
	// Snapshot before any DB read so a concurrent bust blocks the publish.
	generation := a.Sidebar.BeginRead(userID)
	if cached := a.Sidebar.Get(userID); cached != nil && cached.IsCurrent() {
		return chromeData{Chrome: *cached, originalUserIsAdmin: original}
	}
	fresh, cacheable := a.loadChrome(ctx, userID)
	if cacheable {
		a.Sidebar.InsertIfCurrent(userID, generation, fresh)
	}
	return chromeData{Chrome: fresh, originalUserIsAdmin: original}
}

// logQueryFailed records a page query whose error is deliberately survived: the
// page renders without that data, so without a log the gap is invisible.
func logQueryFailed(query string, err error) {
	// A client that leaves mid-render cancels the request; that is no fault.
	if err == nil || errors.Is(err, context.Canceled) {
		return
	}
	slog.Warn("page query failed", "event", "page.query_failed", "query", query, "error", err)
}

// loadChrome reads the chrome from the database. cacheable is false when a
// query failed, which would otherwise pin a partial sidebar (such as no
// categories) for the whole TTL, and for the empty state, the likeliest to go
// stale.
func (a *App) loadChrome(ctx context.Context, userID int64) (chrome cache.Chrome, cacheable bool) {
	queryFailed := false
	checkQuery := func(name string, err error) {
		if err == nil {
			return
		}
		queryFailed = true
		slog.Warn("chrome query failed", "event", "chrome.query_failed", "query", name, "user_id", userID, "error", err)
	}
	settings, err := models.LoadUserSettings(ctx, a.DB, userID)
	checkQuery("settings", err)
	if err != nil {
		settings = models.DefaultUserSettings(userID)
	}
	location := settings.Tz()
	categories, err := models.ListCategoriesByUser(ctx, a.DB, userID)
	checkQuery("categories", err)
	feeds, err := models.ListFeedsByUser(ctx, a.DB, userID)
	checkQuery("feeds", err)
	unreadByFeed, err := entry.CountUnreadByFeed(ctx, a.DB, userID)
	checkQuery("unread_by_feed", err)
	unreadByCategory := map[int64]int64{}
	var totalUnread int64
	for _, feed := range feeds {
		unread := unreadByFeed[feed.ID]
		unreadByCategory[feed.CategoryID] += unread
		totalUnread += unread
	}
	summarized, err := models.CountCompletedSummaries(ctx, a.DB, userID)
	checkQuery("summaries", err)
	// One reading of the date for the count and the cache label: midnight
	// between two reads would label yesterday's count as today's.
	today := timezone.TodayIn(location)
	start, end := timezone.DayBoundsUTC(today, location)
	todayCount, err := entry.CountByUser(ctx, a.DB, userID, &entry.Filter{
		UnreadOnly: true, PublishedBetween: &entry.TimeRange{Start: start, End: end},
	})
	checkQuery("today_count", err)
	rows := make([]cache.SidebarCategory, 0, len(categories))
	for _, category := range categories {
		rows = append(rows, cache.SidebarCategory{
			ID: category.ID, Name: category.Name, UnreadCount: unreadByCategory[category.ID],
		})
	}
	return cache.Chrome{
		Theme: settings.Theme, Categories: rows, TotalUnread: totalUnread,
		TotalSummarized: summarized, TotalToday: todayCount, Timezone: location, Today: today,
		UnreadByFeed: unreadByFeed, SidebarPrefs: settings.SidebarPrefs(), OfflineKeep: settings.OfflineKeep,
	}, !queryFailed && (len(feeds) > 0 || totalUnread > 0)
}

// buildSidebar assembles the sidebar payload for a principal.
func (a *App) buildSidebar(ctx context.Context, principal *auth.Principal) (sidebarResponse, chromeData) {
	masquerading := principal.Session.OriginalUserID != nil
	var original *int64
	if masquerading {
		original = principal.Session.OriginalUserID
	}
	chrome := a.readChrome(ctx, principal.User.ID, original)
	isAdmin := principal.User.IsAdmin()
	if masquerading {
		isAdmin = chrome.originalUserIsAdmin
	}
	categories := make([]sidebarCategory, 0, len(chrome.Categories))
	for _, category := range chrome.Categories {
		categories = append(categories, sidebarCategory{ID: category.ID, Name: category.Name, UnreadCount: category.UnreadCount})
	}
	return sidebarResponse{
		Username: principal.User.Username, IsAdmin: isAdmin, IsMasquerading: masquerading,
		Categories: categories, TotalUnread: chrome.TotalUnread, TotalSummarized: chrome.TotalSummarized,
		TotalToday: chrome.TotalToday, ViaForwardAuth: principal.ViaForwardAuth,
		SidebarSort: chrome.SidebarPrefs.Sort, SidebarHideRead: chrome.SidebarPrefs.HideRead,
	}, chrome
}

// jsonForScript serialises value for a raw inline <script>: serde's output
// (no HTML escaping), with "</" broken up so a name such as "</script>"
// cannot close the element. "null" if encoding fails.
func jsonForScript(value any) string {
	var buf bytes.Buffer
	encoder := json.NewEncoder(&buf)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(value); err != nil {
		return "null"
	}
	encoded := strings.TrimSuffix(buf.String(), "\n")
	return strings.ReplaceAll(encoded, "</", `<\/`)
}

// buildLayout is the shared layout data of a logged-in page.
func (a *App) buildLayout(r *http.Request, principal *auth.Principal, banners []flash.Message) views.Layout {
	sidebar, chrome := a.buildSidebar(r.Context(), principal)
	theme := ""
	if chrome.Theme != nil {
		theme = *chrome.Theme
	}
	return views.Layout{
		Theme:                theme,
		GitVersion:           gitVersion(),
		SidebarBootstrapJSON: jsonForScript(sidebar),
		FlashMessages:        banners,
		IsAdmin:              sidebar.IsAdmin,
		TotalUnread:          sidebar.TotalUnread,
		CSRFToken:            principal.CSRFToken,
		OfflineKey:           secret.OfflineID(a.Config.Secret, principal.User.ID),
		OfflineKeep:          chrome.OfflineKeep,
		UnreadByFeed:         chrome.UnreadByFeed,
	}
}

// chromeRoutes are the JSON endpoints behind the page chrome.
func (a *App) chromeRoutes() []Route {
	return []Route{
		{"GET", "/api/sidebar", a.sidebarAPI},
		{"GET", "/api/sidebar/categories/{id}/feeds", a.sidebarCategoryFeeds},
		{"GET", "/api/user", a.currentUser},
	}
}

// sidebarAPI is GET /api/sidebar.
func (a *App) sidebarAPI(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	sidebar, _ := a.buildSidebar(r.Context(), principal)
	writeJSON(w, http.StatusOK, sidebar)
}

type sidebarFeed struct {
	ID          int64  `json:"id"`
	Title       string `json:"title"`
	UnreadCount int64  `json:"unread_count"`
	// HasIcon avoids a broken-image 404 on /api/feeds/{id}/icon.
	HasIcon bool `json:"has_icon"`
}

// sidebarCategoryFeeds is GET /api/sidebar/categories/{id}/feeds, kept out of
// /api/sidebar because that is embedded in every page.
func (a *App) sidebarCategoryFeeds(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	categoryID, ok := pathID(w, r)
	if !ok {
		return
	}
	feeds, err := a.sidebarFeeds(r.Context(), principal.User.ID, categoryID)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, struct {
		CategoryID int64         `json:"category_id"`
		Feeds      []sidebarFeed `json:"feeds"`
	}{categoryID, feeds})
}

// sidebarFeeds lists a category's feeds; ownership is checked first.
func (a *App) sidebarFeeds(ctx context.Context, userID, categoryID int64) ([]sidebarFeed, error) {
	category, err := models.FindCategoryByIDAndUser(ctx, a.DB, categoryID, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if category == nil {
		return nil, ErrCategoryNotFound
	}
	feeds, err := models.ListFeedsByCategory(ctx, a.DB, categoryID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	unread, err := entry.CountUnreadByFeedInCategory(ctx, a.DB, userID, categoryID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	ids := make([]int64, len(feeds))
	for i, feed := range feeds {
		ids[i] = feed.ID
	}
	withIcon, err := models.ExistingImageIDs(ctx, a.DB, models.EntityFeed, ids)
	if err != nil {
		return nil, DatabaseError(err)
	}
	rows := make([]sidebarFeed, 0, len(feeds))
	for _, feed := range feeds {
		title := feed.URL
		if feed.Title != nil {
			title = *feed.Title
		}
		rows = append(rows, sidebarFeed{ID: feed.ID, Title: title, UnreadCount: unread[feed.ID], HasIcon: withIcon[feed.ID]})
	}
	return rows, nil
}

// currentUser is GET /api/user.
func (a *App) currentUser(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, userJSON(&principal.User))
}
