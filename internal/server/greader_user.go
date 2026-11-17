package server

import (
	"net/http"
	"strconv"

	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// unreadCountMax is the cap GReader clients expect in the unread-count reply.
const unreadCountMax = 1000

// greaderUserInfo is GET /reader/api/0/user-info.
func (a *App) greaderUserInfo(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	id := strconv.FormatInt(user.user.ID, 10)
	writeJSON(w, http.StatusOK, greader.UserInfo{
		UserID: id, UserName: user.user.Username, UserProfileID: id,
		UserEmail: user.user.Username + "@localhost",
	})
}

// greaderUnreadCount is GET /reader/api/0/unread-count: per feed, per label,
// then the reading-list total.
func (a *App) greaderUnreadCount(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	counts, err := a.unreadCounts(r, user.user.ID)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, counts)
}

func (a *App) unreadCounts(r *http.Request, userID int64) (*greader.UnreadCounts, error) {
	ctx := r.Context()
	feeds, err := models.ListFeedsByUser(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	categories, err := models.ListCategoriesByUser(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	byFeed, err := entry.CountUnreadByFeed(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	byCategory, err := entry.CountUnreadByCategory(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	response := &greader.UnreadCounts{Max: unreadCountMax, UnreadCounts: []greader.UnreadCount{}}
	add := func(id string, count int64) {
		response.UnreadCounts = append(response.UnreadCounts,
			greader.UnreadCount{ID: id, Count: count, NewestItemTimestampUsec: "0"})
	}
	var total int64
	for _, f := range feeds {
		add("feed/"+f.URL, byFeed[f.ID])
		total += byFeed[f.ID]
	}
	for _, category := range categories {
		add(greader.Stream{Kind: greader.Label, Name: category.Name}.String(), byCategory[category.ID])
	}
	add(greader.ReadingListID, total)
	return response, nil
}

// greaderStaticJSON answers the endpoints that only exist so clients do not
// fail on them.
func (a *App) greaderStaticJSON(body string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if _, ok := a.requireGReader(w, r); !ok {
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(body))
	}
}

// greaderRoutes are the Google Reader routes under prefix; the same handlers
// are mounted twice, bare and under greaderPrefix.
func (a *App) greaderRoutes(prefix string) []Route {
	routes := []Route{
		{"POST", "/accounts/ClientLogin", a.greaderClientLogin},
		{"GET", "/reader/api/0/token", a.greaderPostToken},
		{"GET", "/reader/api/0/subscription/list", a.greaderSubscriptionList},
		{"POST", "/reader/api/0/subscription/edit", a.greaderSubscriptionEdit},
		{"POST", "/reader/api/0/subscription/quickadd", a.greaderQuickAdd},
		{"GET", "/reader/api/0/subscription/export", a.greaderSubscriptionExport},
		{"POST", "/reader/api/0/subscription/import", a.greaderSubscriptionImport},
		{"GET", "/reader/api/0/subscribed", a.greaderSubscribed},
		{"GET", "/reader/api/0/stream/contents/{*stream}", a.greaderStreamContents},
		{"GET", "/reader/api/0/stream/items/ids", a.greaderStreamItemIDs},
		{"GET", "/reader/api/0/stream/items/count", a.greaderStreamItemCount},
		{"GET", "/reader/api/0/stream/items/contents", a.greaderItemsContentsGet},
		{"POST", "/reader/api/0/stream/items/contents", a.greaderItemsContentsPost},
		{"GET", "/reader/api/0/tag/list", a.greaderTagList},
		{"POST", "/reader/api/0/edit-tag", a.greaderEditTag},
		{"POST", "/reader/api/0/mark-all-as-read", a.greaderMarkAllAsRead},
		{"POST", "/reader/api/0/disable-tag", a.greaderDisableTag},
		{"POST", "/reader/api/0/rename-tag", a.greaderRenameTag},
		{"GET", "/reader/api/0/user-info", a.greaderUserInfo},
		{"GET", "/reader/api/0/unread-count", a.greaderUnreadCount},
		{"GET", "/reader/api/0/preference/list", a.greaderStaticJSON(`{"prefs":[]}`)},
		{"GET", "/reader/api/0/preference/stream/list", a.greaderStaticJSON(`{"streamprefs":{}}`)},
		{"GET", "/reader/api/0/friend/list", a.greaderStaticJSON(`{"friends":[]}`)},
	}
	for i := range routes {
		routes[i].Path = prefix + routes[i].Path
	}
	return routes
}
