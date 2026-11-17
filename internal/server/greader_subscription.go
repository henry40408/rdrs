package server

import (
	"context"
	"errors"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"github.com/henry40408/rdrs/internal/discovery"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/opml"
)

const uncategorizedName = "Uncategorized"

// greaderSubscriptionList is GET /reader/api/0/subscription/list.
func (a *App) greaderSubscriptionList(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	list, err := a.subscriptionList(r.Context(), user.user.ID)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, list)
}

func (a *App) subscriptionList(ctx context.Context, userID int64) (*greader.SubscriptionList, error) {
	feeds, err := models.ListFeedsByUser(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	categories, err := models.ListCategoriesByUser(ctx, a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	feedIDs := make([]int64, len(feeds))
	for i, f := range feeds {
		feedIDs[i] = f.ID
	}
	// One query for every feed's icon instead of one per feed.
	withIcon, err := models.ExistingImageIDs(ctx, a.DB, models.EntityFeed, feedIDs)
	if err != nil {
		return nil, DatabaseError(err)
	}
	categoryByID := make(map[int64]models.Category, len(categories))
	for _, category := range categories {
		categoryByID[category.ID] = category
	}
	list := &greader.SubscriptionList{Subscriptions: make([]greader.Subscription, len(feeds))}
	for i, f := range feeds {
		list.Subscriptions[i] = subscriptionOf(f, categoryByID[f.CategoryID], withIcon[f.ID])
	}
	return list, nil
}

// subscriptionOf converts a feed; a zero category is shown as Uncategorized.
func subscriptionOf(f models.Feed, category models.Category, hasIcon bool) greader.Subscription {
	name := category.Name
	if category.ID == 0 {
		name = uncategorizedName
	}
	iconURL := ""
	if hasIcon {
		iconURL = "/api/feeds/" + strconv.FormatInt(f.ID, 10) + "/icon"
	}
	title := f.URL
	if f.Title != nil {
		title = *f.Title
	}
	return greader.Subscription{
		ID: "feed/" + f.URL, Title: title,
		Categories: []greader.SubscriptionCategory{{ID: "user/-/label/" + name, Label: name}},
		SortID:     sortID(category.ID), HTMLURL: deref(f.SiteURL), URL: f.URL, IconURL: iconURL,
		FeedID: f.ID, FetchError: f.FetchError, Description: f.Description,
		CustomUserAgent: f.CustomUserAgent, HTTP2Disabled: f.HTTP2Disabled, CustomReferrer: f.CustomReferrer,
	}
}

// feedURLFromStream is the URL of a feed/<url> stream argument.
func feedURLFromStream(form url.Values, name string) (string, error) {
	if !form.Has(name) {
		return "", ValidationError("Missing stream ID (s parameter)")
	}
	feedURL, ok := strings.CutPrefix(form.Get(name), "feed/")
	if !ok {
		return "", ValidationError("Stream ID must start with feed/")
	}
	return feedURL, nil
}

// presentText is a form field that may be absent; unlike optionalText, an
// empty value is still a value, as with a serde Option<String>.
func presentText(form url.Values, name string) *string {
	if !form.Has(name) {
		return nil
	}
	return ptr(form.Get(name))
}

// categoryForLabel is the label's category, created when missing.
func (a *App) categoryForLabel(ctx context.Context, userID int64, label string) (int64, error) {
	category, err := a.findOrCreateCategory(ctx, userID, label)
	if err != nil {
		return 0, err
	}
	return category.ID, nil
}

// firstOrUncategorizedID is the user's first category, creating Uncategorized
// for a user with none.
func (a *App) firstOrUncategorizedID(ctx context.Context, userID int64) (int64, error) {
	categories, err := models.ListCategoriesByUser(ctx, a.DB, userID)
	if err != nil {
		return 0, DatabaseError(err)
	}
	if len(categories) > 0 {
		return categories[0].ID, nil
	}
	created, err := models.CreateCategory(ctx, a.DB, userID, uncategorizedName)
	if err != nil {
		return 0, appError(err)
	}
	return created.ID, nil
}

// discoverFeed finds the feed at rawURL, mapping a failure to its AppError.
func (a *App) discoverFeed(ctx context.Context, rawURL string) (*discovery.Feed, error) {
	discovered, err := discovery.Discover(ctx, discovery.Params{
		URL: rawURL, UserAgent: a.Config.UserAgent, Fetcher: a.Fetcher, Retry: fetch.DefaultRetry(),
	})
	if err == nil {
		return discovered, nil
	}
	var failure *discovery.Error
	if !errors.As(err, &failure) {
		return nil, DatabaseError(err)
	}
	switch failure.Kind {
	case discovery.KindInvalidURL:
		return nil, ErrInvalidURL
	case discovery.KindFetch:
		return nil, FetchError(failure.Detail)
	case discovery.KindNoFeed:
		return nil, ErrNoFeedFound
	}
	return nil, ValidationError(failure.Detail)
}

// addDiscoveredFeed stores a discovered feed unless the user has it.
func (a *App) addDiscoveredFeed(ctx context.Context, userID, categoryID int64, discovered *discovery.Feed, title *string) error {
	existing, err := models.FindFeedByURLForUser(ctx, a.DB, discovered.FeedURL, userID)
	if err != nil {
		return DatabaseError(err)
	}
	if existing != nil {
		return ErrFeedExists
	}
	if title == nil {
		title = discovered.Title
	}
	_, err = models.CreateFeed(ctx, a.DB, models.CreateFeedParams{
		CategoryID: categoryID, URL: discovered.FeedURL, Title: title,
		Description: discovered.Description, SiteURL: discovered.SiteURL,
	})
	if errors.Is(err, models.ErrFeedExists) {
		return ErrFeedExists
	}
	if err != nil {
		return appError(err)
	}
	return nil
}

// greaderSubscriptionEdit is POST /reader/api/0/subscription/edit.
func (a *App) greaderSubscriptionEdit(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "ac") {
		return
	}
	if err := a.subscriptionEdit(r.Context(), user, form); err != nil {
		writeError(w, err)
		return
	}
	writeText(w, "OK")
}

func (a *App) subscriptionEdit(ctx context.Context, user greaderUser, form url.Values) error {
	if err := a.checkPostToken(user, form); err != nil {
		return err
	}
	userID := user.user.ID
	switch action := form.Get("ac"); action {
	case "subscribe":
		return a.subscribeByForm(ctx, userID, form)
	case "edit":
		return a.editByForm(ctx, userID, form)
	case "unsubscribe":
		return a.unsubscribeByForm(ctx, userID, form)
	default:
		return ValidationError("Unknown action: " + action)
	}
}

func (a *App) subscribeByForm(ctx context.Context, userID int64, form url.Values) error {
	feedURL, err := feedURLFromStream(form, "s")
	if err != nil {
		return err
	}
	if feedURL == "" {
		return ValidationError("Empty feed URL")
	}
	discovered, err := a.discoverFeed(ctx, feedURL)
	if err != nil {
		return err
	}
	categoryID, err := a.subscriptionCategory(ctx, userID, form)
	if err != nil {
		return err
	}
	return a.addDiscoveredFeed(ctx, userID, categoryID, discovered, presentText(form, "t"))
}

// subscriptionCategory is the category of the a label, else the first one.
func (a *App) subscriptionCategory(ctx context.Context, userID int64, form url.Values) (int64, error) {
	if label, ok := greader.LabelName(form.Get("a")); ok && form.Has("a") {
		return a.categoryForLabel(ctx, userID, label)
	}
	return a.firstOrUncategorizedID(ctx, userID)
}

func (a *App) editByForm(ctx context.Context, userID int64, form url.Values) error {
	feedURL, err := feedURLFromStream(form, "s")
	if err != nil {
		return err
	}
	stored, err := models.FindFeedByURLForUser(ctx, a.DB, feedURL, userID)
	if err != nil {
		return DatabaseError(err)
	}
	if stored == nil {
		return ErrFeedNotFound
	}
	http2Disabled, err := optionalFlag(form, "http2_disabled")
	if err != nil {
		return err
	}
	newCategoryID := stored.CategoryID
	if label, ok := greader.LabelName(form.Get("a")); ok && form.Has("a") {
		if newCategoryID, err = a.categoryForLabel(ctx, userID, label); err != nil {
			return err
		}
	}
	params := models.UpdateFeedParams{
		ID: stored.ID, CategoryID: stored.CategoryID, NewCategoryID: newCategoryID, URL: stored.URL,
		Title:           firstNonNil(presentText(form, "t"), stored.Title),
		Description:     firstNonNil(presentText(form, "description"), stored.Description),
		SiteURL:         firstNonNil(presentText(form, "site_url"), stored.SiteURL),
		CustomUserAgent: firstNonNil(presentText(form, "custom_user_agent"), stored.CustomUserAgent),
		HTTP2Disabled:   stored.HTTP2Disabled, CustomReferrer: replacedReferrer(form, stored.CustomReferrer),
	}
	if http2Disabled != nil {
		params.HTTP2Disabled = *http2Disabled
	}
	if _, err := models.UpdateFeed(ctx, a.DB, params); err != nil {
		return appError(err)
	}
	return nil
}

// replacedReferrer keeps the stored referrer when the field is absent and
// clears it when the field is present but empty.
func replacedReferrer(form url.Values, stored *string) *string {
	if !form.Has("custom_referrer") {
		return stored
	}
	if form.Get("custom_referrer") == "" {
		return nil
	}
	return ptr(form.Get("custom_referrer"))
}

// optionalFlag reads a bool form field the way serde does: only "true" or
// "false".
func optionalFlag(form url.Values, name string) (*bool, error) {
	if !form.Has(name) {
		return nil, nil
	}
	switch form.Get(name) {
	case "true":
		return ptr(true), nil
	case "false":
		return ptr(false), nil
	}
	return nil, newError(http.StatusUnprocessableEntity,
		"Failed to deserialize form body: "+name+": provided string was not `true` or `false`")
}

func (a *App) unsubscribeByForm(ctx context.Context, userID int64, form url.Values) error {
	feedURL, err := feedURLFromStream(form, "s")
	if err != nil {
		return err
	}
	stored, err := models.FindFeedByURLForUser(ctx, a.DB, feedURL, userID)
	if err != nil {
		return DatabaseError(err)
	}
	if stored == nil {
		return ErrFeedNotFound
	}
	if err := models.DeleteFeed(ctx, a.DB, stored.ID, stored.CategoryID); err != nil {
		return DatabaseError(err)
	}
	return nil
}

// greaderQuickAdd is POST /reader/api/0/subscription/quickadd.
func (a *App) greaderQuickAdd(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "quickadd") {
		return
	}
	response, err := a.quickAdd(r.Context(), user, form)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, response)
}

func (a *App) quickAdd(ctx context.Context, user greaderUser, form url.Values) (*greader.QuickAdd, error) {
	if err := a.checkPostToken(user, form); err != nil {
		return nil, err
	}
	rawURL := strings.TrimSpace(form.Get("quickadd"))
	if rawURL == "" {
		return nil, ValidationError("URL cannot be empty")
	}
	discovered, err := a.discoverFeed(ctx, rawURL)
	if err != nil {
		return nil, err
	}
	// Checked before the category lookup so a duplicate creates nothing.
	existing, err := models.FindFeedByURLForUser(ctx, a.DB, discovered.FeedURL, user.user.ID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if existing != nil {
		return nil, ErrFeedExists
	}
	categoryID, err := a.firstOrUncategorizedID(ctx, user.user.ID)
	if err != nil {
		return nil, err
	}
	if err := a.addDiscoveredFeed(ctx, user.user.ID, categoryID, discovered, nil); err != nil {
		return nil, err
	}
	return &greader.QuickAdd{NumResults: 1, Query: rawURL, StreamID: "feed/" + discovered.FeedURL}, nil
}

// greaderSubscriptionExport is GET /reader/api/0/subscription/export.
func (a *App) greaderSubscriptionExport(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	categories, err := models.ListCategoriesByUser(r.Context(), a.DB, user.user.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	feeds, err := models.ListFeedsByUser(r.Context(), a.DB, user.user.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	w.Header().Set("Content-Type", "application/xml; charset=utf-8")
	w.Header().Set("Content-Disposition", `attachment; filename="subscriptions.opml"`)
	_, _ = w.Write([]byte(opml.Export(categories, feeds)))
}

// greaderSubscriptionImport is POST /reader/api/0/subscription/import; the
// body is the raw OPML.
func (a *App) greaderSubscriptionImport(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	body, ok := readBody(w, r)
	if !ok {
		return
	}
	outlines, err := opml.Parse(string(body))
	if err != nil {
		writeError(w, ValidationError(err.Error()))
		return
	}
	summary, err := opml.Import(r.Context(), opml.ImportParams{
		DB: a.DB, UserID: user.user.ID, Outlines: outlines, Fetcher: a.Fetcher,
	})
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	writeOKWithAffected(w, int64(summary.FeedsAdded))
}

// greaderSubscribed is GET /reader/api/0/subscribed.
func (a *App) greaderSubscribed(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	reader := queryReader{values: r.URL.Query()}
	raw := reader.text("s")
	if reader.err != nil {
		deserializeFailure(w, reader.err.Error())
		return
	}
	if raw == nil {
		deserializeFailure(w, "missing field `s`")
		return
	}
	feedURL, ok := strings.CutPrefix(*raw, "feed/")
	if !ok {
		writeError(w, ValidationError("Stream ID must start with feed/"))
		return
	}
	found, err := models.FindFeedByURLForUser(r.Context(), a.DB, feedURL, user.user.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	writeText(w, map[bool]string{true: "true", false: "false"}[found != nil])
}
