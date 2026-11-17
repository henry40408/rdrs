package server

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

const grBase = "/reader/api/0/"

// rssFixture is what the stub feed server answers to every request.
const rssFixture = `<?xml version="1.0"?><rss version="2.0"><channel>
  <title>Mock Feed</title><description>D</description><link>https://e</link>
  <item><guid>m1</guid><title>One</title><link>https://e/1</link><description>c1</description></item>
</channel></rss>`

// grFixture is a signed-in admin whose GReader requests can use a cookie, a
// ClientLogin token, or nothing.
type grFixture struct {
	*signedInFixture
}

func newGReader(t *testing.T) *grFixture {
	t.Helper()
	f := newSignedIn(t)
	f.app.detach = func(task func()) { task() }
	policy, err := fetch.ParsePolicy("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	f.app.Fetcher = fetch.NewFetcher(policy)
	return &grFixture{f}
}

// request builds a form POST (fields != nil) or a GET.
func grRequest(method, path string, fields url.Values) *http.Request {
	var request *http.Request
	if method == http.MethodPost {
		request = httptest.NewRequest(method, path, strings.NewReader(fields.Encode()))
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		return request
	}
	return httptest.NewRequest(method, path, nil)
}

// asBrowser adds the session cookie and CSRF token of the signed-in admin.
func (f *grFixture) asBrowser(request *http.Request) *http.Request {
	request.AddCookie(f.session)
	request.Header.Set("X-CSRF-Token", f.csrf)
	return request
}

func withToken(request *http.Request, token string) *http.Request {
	request.Header.Set("Authorization", "GoogleLogin auth="+token)
	return request
}

func (f *grFixture) browserGet(path string) *httptest.ResponseRecorder {
	return f.do(f.asBrowser(grRequest("GET", path, nil)))
}

func (f *grFixture) browserPost(path string, fields url.Values) *httptest.ResponseRecorder {
	return f.do(f.asBrowser(grRequest("POST", path, fields)))
}

// clientLogin returns the Auth token of a successful ClientLogin.
func (f *grFixture) clientLogin(t *testing.T) string {
	t.Helper()
	rec := f.do(grRequest("POST", "/accounts/ClientLogin",
		url.Values{"Email": {"root"}, "Passwd": {testPassword}}))
	if rec.Code != http.StatusOK {
		t.Fatalf("ClientLogin = %d %s", rec.Code, rec.Body)
	}
	for _, line := range strings.Split(rec.Body.String(), "\n") {
		if token, ok := strings.CutPrefix(line, "Auth="); ok {
			return token
		}
	}
	t.Fatalf("no Auth= in %q", rec.Body)
	return ""
}

func (f *grFixture) sessionToken() string {
	request := httptest.NewRequest("GET", "/", nil)
	request.AddCookie(f.session)
	return f.app.cookies.SessionToken(request)
}

func (f *grFixture) count(t *testing.T, query string) int {
	t.Helper()
	var n int
	if err := f.database.QueryRowContext(context.Background(), query).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

// entries stores n entries on a feed with no publish date, oldest first.
func (f *grFixture) entries(t *testing.T, feedID int64, n int) []int64 {
	t.Helper()
	var ids []int64
	for i := range n {
		guid, title, content := fmt.Sprint("guid-", i), fmt.Sprint("Entry ", i), "Content"
		link := fmt.Sprint("https://example.com/entry/", i)
		result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{
			FeedID: feedID, GUID: guid, Title: &title, Link: &link, Content: &content,
		})
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, result.ID)
	}
	return ids
}

func jsonOf(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d: %s", rec.Code, rec.Body)
	}
	return decodeBody(t, rec)
}

func (f *grFixture) unreadCount(t *testing.T, stream string) float64 {
	t.Helper()
	body := jsonOf(t, f.browserGet(grBase+"unread-count"))
	for _, item := range body["unreadcounts"].([]any) {
		counts := item.(map[string]any)
		if counts["id"] == stream {
			return counts["count"].(float64)
		}
	}
	t.Fatalf("no unread count for %s in %v", stream, body)
	return 0
}

func (f *grFixture) feedCount(t *testing.T) int {
	t.Helper()
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))
	return len(list["subscriptions"].([]any))
}

func itemRef(id int64) string { return "tag:google.com,2005:reader/item/" + fmt.Sprintf("%016x", id) }

// --- ClientLogin and credentials ---

func TestClientLoginIssuesAnAPITokenAndTrimsTheUsername(t *testing.T) {
	f := newGReader(t)
	rec := f.do(grRequest("POST", "/accounts/ClientLogin",
		url.Values{"Email": {"root "}, "Passwd": {testPassword}}))
	body := rec.Body.String()
	if rec.Code != http.StatusOK || !strings.Contains(body, "SID=") || !strings.Contains(body, "LSID=") ||
		!strings.Contains(body, "Auth="+models.APITokenPrefix) {
		t.Fatalf("ClientLogin = %d %q", rec.Code, body)
	}
}

func TestClientLoginRefusesBadCredentials(t *testing.T) {
	f := newGReader(t)
	for name, fields := range map[string]url.Values{
		"bad password": {"Email": {"root"}, "Passwd": {"wrong"}},
		"unknown user": {"Email": {"nobody"}, "Passwd": {testPassword}},
	} {
		if rec := f.do(grRequest("POST", "/accounts/ClientLogin", fields)); rec.Code != http.StatusUnauthorized {
			t.Errorf("%s = %d", name, rec.Code)
		}
	}
}

func TestAPITokenAuthenticatesTheAuthorizationHeader(t *testing.T) {
	f := newGReader(t)
	token := f.clientLogin(t)
	rec := f.do(withToken(grRequest("GET", grBase+"subscription/list", nil), token))
	if rec.Code != http.StatusOK {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
}

func TestAPITokenIsNotAWebSession(t *testing.T) {
	f := newGReader(t)
	token := f.clientLogin(t)
	sessionsBefore := f.count(t, "SELECT COUNT(*) FROM session")
	signed := f.app.cookies.Session(token)
	for name, cookie := range map[string]*http.Cookie{
		"bare cookie":   {Name: auth.SessionCookieName, Value: token},
		"signed cookie": signed,
	} {
		request := grRequest("GET", grBase+"subscription/list", nil)
		request.AddCookie(cookie)
		if rec := f.do(request); rec.Code != http.StatusUnauthorized {
			t.Errorf("%s = %d", name, rec.Code)
		}
	}
	if got := f.count(t, "SELECT COUNT(*) FROM session"); got != sessionsBefore {
		t.Fatalf("sessions %d -> %d", sessionsBefore, got)
	}
}

func TestWebSessionTokenIsNotAnAPIToken(t *testing.T) {
	f := newGReader(t)
	raw := f.sessionToken()
	for name, token := range map[string]string{"bare": raw, "signed": f.session.Value} {
		rec := f.do(withToken(grRequest("GET", grBase+"subscription/list", nil), token))
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("%s = %d", name, rec.Code)
		}
	}
	if rec := f.browserGet(grBase + "subscription/list"); rec.Code != http.StatusOK {
		t.Fatalf("the session no longer works: %d", rec.Code)
	}
}

func TestUnauthenticatedGReaderRequestsAre401(t *testing.T) {
	f := newGReader(t)
	for _, path := range []string{"subscription/list", "unread-count", "stream/contents/user/-/state/com.google/reading-list"} {
		if rec := f.do(grRequest("GET", grBase+path, nil)); rec.Code != http.StatusUnauthorized {
			t.Errorf("%s = %d", path, rec.Code)
		}
	}
}

func TestExpiredAPITokenIs401AndLazilyDeleted(t *testing.T) {
	f := newGReader(t)
	token := f.clientLogin(t)
	_, err := f.database.ExecContext(context.Background(),
		"UPDATE api_token SET expires_at = datetime('now', '-1 hour')")
	if err != nil {
		t.Fatal(err)
	}
	if rec := f.do(withToken(grRequest("GET", grBase+"subscription/list", nil), token)); rec.Code != http.StatusUnauthorized {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.count(t, "SELECT COUNT(*) FROM api_token"); got != 0 {
		t.Fatalf("%d api tokens left", got)
	}
}

func TestPostTokenIsPerCredentialAndBoundToIt(t *testing.T) {
	f := newGReader(t)
	apiToken := f.clientLogin(t)
	cookieT := f.browserGet(grBase + "token").Body.String()
	apiT := f.do(withToken(grRequest("GET", grBase+"token", nil), apiToken)).Body.String()
	if cookieT == "" || apiT == "" || cookieT == apiT {
		t.Fatalf("tokens %q %q must exist and differ", cookieT, apiT)
	}
	edit := func(posted string) int {
		fields := url.Values{"ac": {"edit"}, "s": {"feed/https://nowhere.example/x"}, "T": {posted}}
		return f.do(withToken(grRequest("POST", grBase+"subscription/edit", fields), apiToken)).Code
	}
	if got := edit(cookieT); got != http.StatusUnauthorized {
		t.Errorf("a token from another credential = %d", got)
	}
	if got := edit(apiT); got != http.StatusNotFound {
		t.Errorf("the matching token = %d, want the unknown feed's 404", got)
	}
	if got := edit("garbage"); got != http.StatusUnauthorized {
		t.Errorf("garbage = %d", got)
	}
}

func TestClientLoginIsRateLimitedOnBothPrefixes(t *testing.T) {
	f := newGReader(t)
	f.app.Limiter = auth.NewRateLimiter(5, 60)
	attempt := func(prefix string) int {
		return f.do(grRequest("POST", prefix+"/accounts/ClientLogin",
			url.Values{"Email": {"root"}, "Passwd": {"wrong"}})).Code
	}
	for i := range 5 {
		prefix := ""
		if i%2 == 1 {
			prefix = greaderPrefix
		}
		if got := attempt(prefix); got != http.StatusUnauthorized {
			t.Fatalf("attempt %d = %d", i, got)
		}
	}
	for _, prefix := range []string{"", greaderPrefix} {
		if got := attempt(prefix); got != http.StatusTooManyRequests {
			t.Errorf("%q after the budget = %d", prefix, got)
		}
	}
}

// --- subscriptions ---

func TestSubscriptionListIsEmptyThenShowsFeeds(t *testing.T) {
	f := newGReader(t)
	if got := f.feedCount(t); got != 0 {
		t.Fatalf("%d feeds", got)
	}
	f.withFeed(t)
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))["subscriptions"].([]any)
	first := list[0].(map[string]any)
	category := first["categories"].([]any)[0].(map[string]any)
	if !strings.HasPrefix(first["id"].(string), "feed/") || first["title"] != "Example" ||
		category["label"] != "Tech" || category["id"] != "user/-/label/Tech" {
		t.Fatalf("subscription = %v", first)
	}
}

func (f *grFixture) edit(fields url.Values) *httptest.ResponseRecorder {
	return f.browserPost(grBase+"subscription/edit", fields)
}

func (f *grFixture) feed(t *testing.T, feedURL string) *models.Feed {
	t.Helper()
	found, err := models.FindFeedByURLForUser(context.Background(), f.database, feedURL, 1)
	if err != nil || found == nil {
		t.Fatalf("feed %s: %v %v", feedURL, found, err)
	}
	return found
}

func TestSubscriptionUnsubscribeAndRename(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	const stream = "feed/https://example.com/feed.xml"
	if rec := f.edit(url.Values{"ac": {"edit"}, "s": {stream}, "t": {"Renamed"}}); rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("rename = %d %q", rec.Code, rec.Body)
	}
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))["subscriptions"].([]any)
	if list[0].(map[string]any)["title"] != "Renamed" {
		t.Fatalf("not renamed: %v", list)
	}
	if rec := f.edit(url.Values{"ac": {"unsubscribe"}, "s": {stream}}); rec.Code != 200 {
		t.Fatalf("unsubscribe = %d", rec.Code)
	}
	if got := f.feedCount(t); got != 0 {
		t.Fatalf("%d feeds left", got)
	}
}

func TestSubscriptionEditValidation(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	cases := map[string]struct {
		fields url.Values
		status int
	}{
		"subscribe without s":      {url.Values{"ac": {"subscribe"}}, 400},
		"subscribe bare url":       {url.Values{"ac": {"subscribe"}, "s": {"https://example.com/feed.xml"}}, 400},
		"subscribe empty url":      {url.Values{"ac": {"subscribe"}, "s": {"feed/"}}, 400},
		"edit without s":           {url.Values{"ac": {"edit"}}, 400},
		"unsubscribe without s":    {url.Values{"ac": {"unsubscribe"}}, 400},
		"edit bare url":            {url.Values{"ac": {"edit"}, "s": {"https://example.com/feed.xml"}}, 400},
		"unsubscribe bare url":     {url.Values{"ac": {"unsubscribe"}, "s": {"https://example.com/feed.xml"}}, 400},
		"edit unknown feed":        {url.Values{"ac": {"edit"}, "s": {"feed/https://nope.example/rss"}}, 404},
		"unsubscribe unknown feed": {url.Values{"ac": {"unsubscribe"}, "s": {"feed/https://nope.example/rss"}}, 404},
		"unknown action":           {url.Values{"ac": {"explode"}}, 400},
		"missing action":           {url.Values{"s": {"feed/x"}}, 422},
		"bad http2 flag":           {url.Values{"ac": {"edit"}, "s": {"feed/https://example.com/feed.xml"}, "http2_disabled": {"yes"}}, 422},
	}
	for name, c := range cases {
		if rec := f.edit(c.fields); rec.Code != c.status {
			t.Errorf("%s = %d, want %d (%s)", name, rec.Code, c.status, rec.Body)
		}
	}
	if got := f.feedCount(t); got != 1 {
		t.Fatalf("a refused request changed the feeds: %d", got)
	}
}

// populatedFeed has every editable field set, so a test can see which survive.
func (f *grFixture) populatedFeed(t *testing.T) (feedURL string, before *models.Feed) {
	t.Helper()
	ctx := context.Background()
	category, err := models.CreateCategory(ctx, f.database, 1, "Tech")
	if err != nil {
		t.Fatal(err)
	}
	feedURL = "https://example.com/full.xml"
	_, err = models.CreateFeed(ctx, f.database, models.CreateFeedParams{
		CategoryID: category.ID, URL: feedURL, Title: ptr("Old Title"), Description: ptr("Old description"),
		SiteURL: ptr("https://old.example.com"), CustomUserAgent: ptr("OldAgent/1.0"),
		HTTP2Disabled: true, CustomReferrer: ptr("https://old.example.com/"),
	})
	if err != nil {
		t.Fatal(err)
	}
	return feedURL, f.feed(t, feedURL)
}

func TestSubscriptionEditKeepsFieldsTheFormLeavesOut(t *testing.T) {
	f := newGReader(t)
	feedURL, before := f.populatedFeed(t)
	if rec := f.edit(url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "t": {"New Title"}}); rec.Code != 200 {
		t.Fatal(rec.Code)
	}
	after := f.feed(t, feedURL)
	if deref(after.Title) != "New Title" || after.CategoryID != before.CategoryID ||
		deref(after.Description) != "Old description" || deref(after.SiteURL) != "https://old.example.com" ||
		deref(after.CustomUserAgent) != "OldAgent/1.0" || !after.HTTP2Disabled ||
		deref(after.CustomReferrer) != "https://old.example.com/" {
		t.Fatalf("after = %+v", after)
	}
}

func TestSubscriptionEditReplacesEveryExtensionField(t *testing.T) {
	f := newGReader(t)
	feedURL, _ := f.populatedFeed(t)
	rec := f.edit(url.Values{
		"ac": {"edit"}, "s": {"feed/" + feedURL}, "description": {"New description"},
		"site_url": {"https://new.example.com"}, "custom_user_agent": {"NewAgent/2.0"},
		"http2_disabled": {"false"}, "custom_referrer": {"https://new.example.com/"},
	})
	if rec.Code != 200 {
		t.Fatal(rec.Code)
	}
	after := f.feed(t, feedURL)
	if deref(after.Title) != "Old Title" || deref(after.Description) != "New description" ||
		deref(after.SiteURL) != "https://new.example.com" || deref(after.CustomUserAgent) != "NewAgent/2.0" ||
		after.HTTP2Disabled || deref(after.CustomReferrer) != "https://new.example.com/" {
		t.Fatalf("after = %+v", after)
	}
}

func TestSubscriptionEditWithAnEmptyReferrerClearsIt(t *testing.T) {
	f := newGReader(t)
	feedURL, _ := f.populatedFeed(t)
	f.edit(url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "custom_referrer": {""}})
	if got := f.feed(t, feedURL).CustomReferrer; got != nil {
		t.Fatalf("referrer = %q", *got)
	}
}

func (f *grFixture) categoryNameOf(t *testing.T, feedURL string) string {
	t.Helper()
	found, err := models.FindCategoryByIDAndUser(context.Background(), f.database, f.feed(t, feedURL).CategoryID, 1)
	if err != nil || found == nil {
		t.Fatalf("category: %v %v", found, err)
	}
	return found.Name
}

func TestSubscriptionEditMovesAFeedByLabel(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	const feedURL = "https://example.com/feed.xml"
	if rec := f.edit(url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/NewCat"}}); rec.Code != 200 {
		t.Fatal(rec.Code)
	}
	if got := f.categoryNameOf(t, feedURL); got != "NewCat" {
		t.Fatalf("category = %q", got)
	}
	// A label that exists is reused, not duplicated.
	f.edit(url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/Tech"}})
	f.edit(url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/NewCat"}})
	categories, _ := models.ListCategoriesByUser(context.Background(), f.database, 1)
	names := map[string]int{}
	for _, category := range categories {
		names[category.Name]++
	}
	if names["NewCat"] != 1 || names["Tech"] != 1 {
		t.Fatalf("categories = %v", names)
	}
}

// stubFeedServer serves the RSS fixture at every path.
func stubFeedServer(t *testing.T) string {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = w.Write([]byte(rssFixture))
	}))
	t.Cleanup(server.Close)
	return server.URL
}

func (f *grFixture) removeCategories(t *testing.T) {
	t.Helper()
	categories, _ := models.ListCategoriesByUser(context.Background(), f.database, 1)
	for _, category := range categories {
		if err := models.DeleteCategory(context.Background(), f.database, category.ID, 1); err != nil {
			t.Fatal(err)
		}
	}
}

func TestSubscribeCreatesTheFeed(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	if rec := f.edit(url.Values{"ac": {"subscribe"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/Tech"}}); rec.Code != 200 {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
	if got := f.categoryNameOf(t, feedURL); got != "Tech" {
		t.Fatalf("category = %q", got)
	}
	if rec := f.edit(url.Values{"ac": {"subscribe"}, "s": {"feed/" + feedURL}}); rec.Code != http.StatusConflict {
		t.Fatalf("duplicate = %d", rec.Code)
	}
}

func TestSubscribeTitleOverridesTheDiscoveredOne(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	f.edit(url.Values{"ac": {"subscribe"}, "s": {"feed/" + feedURL}, "t": {"My Own Title"}})
	if got := deref(f.feed(t, feedURL).Title); got != "My Own Title" {
		t.Fatalf("title = %q", got)
	}
}

func TestSubscribeAndQuickAddLandInTheFirstCategory(t *testing.T) {
	f := newGReader(t)
	if _, err := models.CreateCategory(context.Background(), f.database, 1, "Extra"); err != nil {
		t.Fatal(err)
	}
	before, _ := models.ListCategoriesByUser(context.Background(), f.database, 1)
	subscribed, quickAdded := stubFeedServer(t), stubFeedServer(t)
	f.edit(url.Values{"ac": {"subscribe"}, "s": {"feed/" + subscribed}})
	rec := f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {quickAdded}})
	body := jsonOf(t, rec)
	if body["numResults"] != float64(1) || body["streamId"] != "feed/"+quickAdded || body["query"] != quickAdded {
		t.Fatalf("quickadd = %v", body)
	}
	for _, feedURL := range []string{subscribed, quickAdded} {
		if got := f.feed(t, feedURL).CategoryID; got != before[0].ID {
			t.Errorf("%s in category %d, want %d", feedURL, got, before[0].ID)
		}
	}
	if after, _ := models.ListCategoriesByUser(context.Background(), f.database, 1); len(after) != len(before) {
		t.Fatalf("categories %d -> %d", len(before), len(after))
	}
}

func TestSubscribeAndQuickAddRecreateUncategorized(t *testing.T) {
	f := newGReader(t)
	f.removeCategories(t)
	subscribed := stubFeedServer(t)
	f.edit(url.Values{"ac": {"subscribe"}, "s": {"feed/" + subscribed}})
	if got := f.categoryNameOf(t, subscribed); got != "Uncategorized" {
		t.Fatalf("subscribe category = %q", got)
	}
	f.removeCategories(t)
	quickAdded := stubFeedServer(t)
	f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {quickAdded}})
	if got := f.categoryNameOf(t, quickAdded); got != "Uncategorized" {
		t.Fatalf("quickadd category = %q", got)
	}
}

func TestQuickAddRefusesEmptyAndDuplicateURLs(t *testing.T) {
	f := newGReader(t)
	if rec := f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {"  "}}); rec.Code != 400 {
		t.Fatalf("empty = %d", rec.Code)
	}
	if rec := f.browserPost(grBase+"subscription/quickadd", url.Values{}); rec.Code != 422 {
		t.Fatalf("missing = %d", rec.Code)
	}
	feedURL := stubFeedServer(t)
	f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {feedURL}})
	if rec := f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {feedURL}}); rec.Code != http.StatusConflict {
		t.Fatalf("duplicate = %d", rec.Code)
	}
}

func TestSubscribedExportAndImport(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	for target, want := range map[string]string{
		"feed/https://example.com/feed.xml": "true", "feed/https://other.example/": "false",
	} {
		if got := f.browserGet(grBase + "subscribed?s=" + url.QueryEscape(target)).Body.String(); got != want {
			t.Errorf("subscribed %s = %q", target, got)
		}
	}
	if rec := f.browserGet(grBase + "subscribed?s=https://example.com/"); rec.Code != 400 {
		t.Errorf("bad prefix = %d", rec.Code)
	}
	rec := f.browserGet(grBase + "subscription/export")
	if rec.Header().Get("Content-Type") != "application/xml; charset=utf-8" ||
		rec.Header().Get("Content-Disposition") != `attachment; filename="subscriptions.opml"` ||
		!strings.Contains(rec.Body.String(), "https://example.com/feed.xml") {
		t.Fatalf("export = %v %s", rec.Header(), rec.Body)
	}
	imported := httptest.NewRequest("POST", grBase+"subscription/import", strings.NewReader("<opml"))
	if rec := f.do(f.asBrowser(imported)); rec.Code != 400 {
		t.Fatalf("bad opml = %d", rec.Code)
	}
}

// --- streams and tags ---

func TestStreamContentsPagesWithAContinuation(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 3)
	const reading = "stream/contents/user/-/state/com.google/reading-list"
	all := jsonOf(t, f.browserGet(grBase+reading))
	if got := len(all["items"].([]any)); got != 3 {
		t.Fatalf("%d items", got)
	}
	page := jsonOf(t, f.browserGet(grBase+reading+"?n=2"))
	if len(page["items"].([]any)) != 2 || page["continuation"] == nil {
		t.Fatalf("page = %v", page)
	}
	next := jsonOf(t, f.browserGet(grBase+reading+"?n=2&c="+url.QueryEscape(page["continuation"].(string))))
	if len(next["items"].([]any)) != 1 || next["continuation"] != nil {
		t.Fatalf("next = %v", next)
	}
	if got := len(jsonOf(t, f.browserGet(grBase+reading+"?n=-2"))["items"].([]any)); got != 0 {
		t.Fatalf("n=-2 returned %d items", got)
	}
	starred := jsonOf(t, f.browserGet(grBase+"stream/contents/user/-/state/com.google/starred"))
	if len(starred["items"].([]any)) != 0 {
		t.Fatalf("starred = %v", starred)
	}
}

func TestStreamContentsContinuationVisitsBackDatedEntries(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 5)
	// Entries stored later but published earlier must still be reached.
	for i := range 2 {
		guid, published := fmt.Sprint("old-", i), time.Date(2000, 1, 1, 0, i, 0, 0, time.UTC)
		if _, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{
			FeedID: feedID, GUID: guid, Title: &guid, PublishedAt: &published,
		}); err != nil {
			t.Fatal(err)
		}
	}
	const reading = "stream/contents/user/-/state/com.google/reading-list"
	first := jsonOf(t, f.browserGet(grBase+reading+"?n=5"))
	cursor, _ := first["continuation"].(string)
	if !strings.Contains(cursor, "|") {
		t.Fatalf("continuation %q is not composite", cursor)
	}
	second := jsonOf(t, f.browserGet(grBase+reading+"?n=5&c="+url.QueryEscape(cursor)))
	if got := len(first["items"].([]any)) + len(second["items"].([]any)); got != 7 {
		t.Fatalf("visited %d entries, want 7", got)
	}
}

func TestStreamContentsOfAFeedURLWithASchemeSlashes(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 1)
	for _, path := range []string{
		"stream/contents/feed/https://example.com/feed.xml",
		"stream/contents/feed/https%3A%2F%2Fexample.com%2Ffeed.xml",
	} {
		rec := f.browserGet(grBase + path)
		if rec.Code != 200 {
			t.Errorf("%s = %d %s", path, rec.Code, rec.Header().Get("Location"))
			continue
		}
		if got := len(decodeBody(t, rec)["items"].([]any)); got != 1 {
			t.Errorf("%s: %d items", path, got)
		}
	}
	if rec := f.browserGet(grBase + "stream/contents/feed/https://unknown.example/"); rec.Code != 404 {
		t.Errorf("unknown feed = %d", rec.Code)
	}
}

func TestStreamItemIDsListsRefsAndPages(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	ids := f.entries(t, feedID, 3)
	body := jsonOf(t, f.browserGet(grBase+"stream/items/ids?s=user/-/state/com.google/reading-list&n=10"))
	refs := body["itemRefs"].([]any)
	if len(refs) != 3 {
		t.Fatalf("refs = %v", refs)
	}
	if got := refs[0].(map[string]any)["id"]; got != fmt.Sprint(ids[2]) && got != fmt.Sprint(ids[0]) {
		t.Fatalf("id %v is not a decimal entry id of %v", got, ids)
	}
	page := jsonOf(t, f.browserGet(grBase+"stream/items/ids?n=2"))
	if len(page["itemRefs"].([]any)) != 2 || page["continuation"] == nil {
		t.Fatalf("page = %v", page)
	}
}

func TestEditTagMarksReadStarsAndRefusesUnknownIDs(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	ids := f.entries(t, feedID, 3)
	const reading = "user/-/state/com.google/reading-list"
	rec := f.browserPost(grBase+"edit-tag", url.Values{"i": {itemRef(ids[0])}, "a": {"user/-/state/com.google/read"}})
	if rec.Code != 200 || rec.Header().Get("X-Rdrs-Affected") != "1" || rec.Body.String() != "OK" {
		t.Fatalf("mark read = %d %v %q", rec.Code, rec.Header(), rec.Body)
	}
	if got := f.unreadCount(t, reading); got != 2 {
		t.Fatalf("unread = %v", got)
	}
	// Already read: nothing to change.
	rec = f.browserPost(grBase+"edit-tag", url.Values{
		"i": {itemRef(ids[0]), itemRef(ids[1]), itemRef(ids[2])}, "a": {"user/-/state/com.google/read"}})
	if got := rec.Header().Get("X-Rdrs-Affected"); got != "2" {
		t.Fatalf("batch affected = %q", got)
	}
	f.browserPost(grBase+"edit-tag", url.Values{"i": {itemRef(ids[0])}, "r": {"user/-/state/com.google/read"}})
	before := f.unreadCount(t, reading)
	rec = f.browserPost(grBase+"edit-tag", url.Values{
		"i": {itemRef(ids[1]), itemRef(999999)}, "a": {"user/-/state/com.google/read"}})
	if rec.Code != 404 || f.unreadCount(t, reading) != before {
		t.Fatalf("unknown id = %d, unread %v -> %v", rec.Code, before, f.unreadCount(t, reading))
	}
	if rec := f.browserPost(grBase+"edit-tag", url.Values{"a": {"user/-/state/com.google/read"}}); rec.Code != 400 {
		t.Fatalf("no ids = %d", rec.Code)
	}
}

func TestEditTagStarsAndUnstars(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	ids := f.entries(t, feedID, 2)
	const starred = "stream/contents/user/-/state/com.google/starred"
	f.browserPost(grBase+"edit-tag", url.Values{"i": {itemRef(ids[0])}, "a": {"user/-/state/com.google/starred"}})
	if got := len(jsonOf(t, f.browserGet(grBase+starred))["items"].([]any)); got != 1 {
		t.Fatalf("starred = %d", got)
	}
	f.browserPost(grBase+"edit-tag", url.Values{"i": {itemRef(ids[0])}, "r": {"user/-/state/com.google/starred"}})
	if got := len(jsonOf(t, f.browserGet(grBase+starred))["items"].([]any)); got != 0 {
		t.Fatalf("starred after unstar = %d", got)
	}
}

func TestMarkAllAsReadReportsHowManyChanged(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 5)
	fields := url.Values{"s": {"user/-/state/com.google/reading-list"}}
	for _, want := range []string{"5", "0"} {
		rec := f.browserPost(grBase+"mark-all-as-read", fields)
		if rec.Code != 200 || rec.Header().Get("X-Rdrs-Affected") != want || rec.Body.String() != "OK" {
			t.Fatalf("= %d affected %q body %q, want %s", rec.Code, rec.Header().Get("X-Rdrs-Affected"), rec.Body, want)
		}
	}
}

func TestMarkAllAsReadValidation(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	cases := map[string]struct {
		fields url.Values
		status int
	}{
		"missing s":        {url.Values{}, 422},
		"bad snapshot":     {url.Values{"s": {"user/-/state/com.google/reading-list"}, "snapshot": {"yesterday"}}, 400},
		"starred stream":   {url.Values{"s": {"user/-/state/com.google/starred"}}, 400},
		"unknown feed":     {url.Values{"s": {"feed/https://nope.example/"}}, 404},
		"unknown label":    {url.Values{"s": {"user/-/label/Nope"}}, 404},
		"known label":      {url.Values{"s": {"user/-/label/Tech"}}, 200},
		"snapshot and ts":  {url.Values{"s": {"user/-/state/com.google/reading-list"}, "snapshot": {"2026-01-01 00:00:00"}, "ts": {"1"}}, 200},
		"unparsable ts":    {url.Values{"s": {"user/-/state/com.google/reading-list"}, "ts": {"soon"}}, 200},
		"feed stream":      {url.Values{"s": {"feed/https://example.com/feed.xml"}}, 200},
		"malformed stream": {url.Values{"s": {"nonsense"}}, 400},
	}
	for name, c := range cases {
		if rec := f.browserPost(grBase+"mark-all-as-read", c.fields); rec.Code != c.status {
			t.Errorf("%s = %d, want %d (%s)", name, rec.Code, c.status, rec.Body)
		}
	}
}

func TestUnreadCountListsFeedsLabelsAndTotal(t *testing.T) {
	f := newGReader(t)
	body := jsonOf(t, f.browserGet(grBase+"unread-count"))
	if body["max"] != float64(1000) || len(body["unreadcounts"].([]any)) != 2 {
		t.Fatalf("empty = %v", body)
	}
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 3)
	if got := f.unreadCount(t, "user/-/state/com.google/reading-list"); got != 3 {
		t.Fatalf("total = %v", got)
	}
	if f.unreadCount(t, "feed/https://example.com/feed.xml") != 3 || f.unreadCount(t, "user/-/label/Tech") != 3 {
		t.Fatal("feed or label count wrong")
	}
}

func TestItemContentsByIDAcceptsGetAndPost(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	ids := f.entries(t, feedID, 2)
	empty := jsonOf(t, f.browserGet(grBase+"stream/items/contents"))
	if len(empty["items"].([]any)) != 0 || empty["id"] != "user/-/state/com.google/reading-list" {
		t.Fatalf("empty = %v", empty)
	}
	got := jsonOf(t, f.browserGet(grBase+"stream/items/contents?i="+url.QueryEscape(itemRef(ids[0]))+"&i="+fmt.Sprint(ids[1])))
	if len(got["items"].([]any)) != 2 {
		t.Fatalf("GET = %v", got)
	}
	posted := jsonOf(t, f.browserPost(grBase+"stream/items/contents", url.Values{"i": {itemRef(ids[0])}}))
	item := posted["items"].([]any)[0].(map[string]any)
	if item["id"] != itemRef(ids[0]) || item["title"] != "Title"[:0]+"Entry 0" || item["_content"] == nil {
		t.Fatalf("item = %v", item)
	}
	if rec := f.browserGet(grBase + "stream/items/contents?i=zz"); rec.Code != 400 {
		t.Fatalf("bad id = %d", rec.Code)
	}
}

func TestStreamItemCountAnswersPlainText(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	f.entries(t, feedID, 3)
	rec := f.browserGet(grBase + "stream/items/count?s=user/-/state/com.google/reading-list")
	if rec.Body.String() != "3" || !strings.HasPrefix(rec.Header().Get("Content-Type"), "text/plain") {
		t.Fatalf("= %q %v", rec.Body, rec.Header())
	}
}

func TestTagListRenameAndDisable(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	tags := jsonOf(t, f.browserGet(grBase+"tag/list"))["tags"].([]any)
	// Four state tags, then the admin's Uncategorized and Tech folders.
	if len(tags) != 6 || tags[4].(map[string]any)["type"] != "folder" || tags[5].(map[string]any)["type"] != "folder" {
		t.Fatalf("tags = %v", tags)
	}
	if rec := f.browserPost(grBase+"rename-tag", url.Values{"s": {"user/-/label/Tech"}, "dest": {"user/-/label/Technology"}}); rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("rename = %d", rec.Code)
	}
	if f.categoryNameOf(t, "https://example.com/feed.xml") != "Technology" {
		t.Fatal("the label was not renamed")
	}
	if rec := f.browserPost(grBase+"disable-tag", url.Values{"s": {"user/-/label/Technology"}}); rec.Code != 200 {
		t.Fatalf("disable = %d", rec.Code)
	}
	if got := f.feedCount(t); got != 0 {
		t.Fatalf("%d feeds survived deleting their label", got)
	}
}

func TestTagOperationsValidation(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	cases := []struct {
		name, path string
		fields     url.Values
		status     int
	}{
		{"disable no tag", "disable-tag", url.Values{}, 400},
		{"disable state tag", "disable-tag", url.Values{"s": {"user/-/state/com.google/read"}}, 400},
		{"disable unknown", "disable-tag", url.Values{"s": {"user/-/label/Nope"}}, 404},
		{"disable via t", "disable-tag", url.Values{"t": {"user/-/label/Nope"}}, 404},
		{"rename no source", "rename-tag", url.Values{"dest": {"user/-/label/B"}}, 400},
		{"rename no dest", "rename-tag", url.Values{"s": {"user/-/label/Tech"}}, 400},
		{"rename state source", "rename-tag", url.Values{"s": {"user/-/state/com.google/read"}, "dest": {"user/-/label/B"}}, 400},
		{"rename state dest", "rename-tag", url.Values{"s": {"user/-/label/Tech"}, "dest": {"user/-/state/com.google/read"}}, 400},
		{"rename unknown", "rename-tag", url.Values{"s": {"user/-/label/Nope"}, "dest": {"user/-/label/B"}}, 404},
		{"rename same creates", "rename-tag", url.Values{"s": {"user/-/label/Fresh"}, "dest": {"user/-/label/Fresh"}}, 200},
	}
	for _, c := range cases {
		if rec := f.browserPost(grBase+c.path, c.fields); rec.Code != c.status {
			t.Errorf("%s = %d, want %d (%s)", c.name, rec.Code, c.status, rec.Body)
		}
	}
}

func TestUserInfoAndStaticEndpoints(t *testing.T) {
	f := newGReader(t)
	info := jsonOf(t, f.browserGet(grBase+"user-info"))
	if info["userName"] != "root" || info["userEmail"] != "root@localhost" || info["userId"] != info["userProfileId"] {
		t.Fatalf("info = %v", info)
	}
	for path, want := range map[string]string{
		"preference/list": `{"prefs":[]}`, "preference/stream/list": `{"streamprefs":{}}`, "friend/list": `{"friends":[]}`,
	} {
		if got := f.browserGet(grBase + path).Body.String(); got != want {
			t.Errorf("%s = %q", path, got)
		}
	}
}

func TestPrefixedRoutesServeTheSameHandlers(t *testing.T) {
	f := newGReader(t)
	if rec := f.browserGet(greaderPrefix + "/reader/api/0/user-info"); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
}

// --- sidebar events ---

func TestGReaderWritesAnnounceSidebarChanges(t *testing.T) {
	f := newGReader(t)
	_, feedID := f.withFeed(t)
	ids := f.entries(t, feedID, 3)
	cases := []struct {
		name, path string
		fields     url.Values
	}{
		{"edit-tag", "edit-tag", url.Values{"i": {itemRef(ids[0])}, "a": {"user/-/state/com.google/read"}}},
		{"mark-all-as-read", "mark-all-as-read", url.Values{"s": {"user/-/state/com.google/reading-list"}}},
		{"rename-tag", "rename-tag", url.Values{"s": {"user/-/label/Tech"}, "dest": {"user/-/label/Technology"}}},
		{"disable-tag", "disable-tag", url.Values{"s": {"user/-/label/Technology"}}},
	}
	for _, c := range cases {
		subscription := f.app.Events.Subscribe()
		if rec := f.browserPost(grBase+c.path, c.fields); rec.Code != 200 {
			t.Fatalf("%s = %d %s", c.name, rec.Code, rec.Body)
		}
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		event, err := subscription.Next(ctx)
		cancel()
		subscription.Close()
		if err != nil || event.Kind != events.Sidebar || event.UserID != 1 {
			t.Errorf("%s: event %+v, err %v", c.name, event, err)
		}
	}
}

// --- CSRF ---

func (f *grFixture) unsubscribeFields() url.Values {
	return url.Values{"ac": {"unsubscribe"}, "s": {"feed/https://example.com/feed.xml"}}
}

func TestSameSiteForgedPostCannotUnsubscribe(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	request := grRequest("POST", grBase+"subscription/edit", f.unsubscribeFields())
	request.AddCookie(f.session)
	request.Header.Set("Sec-Fetch-Site", "same-site")
	request.Header.Set("Origin", "https://other.example.com")
	request.Host = "app.example.com"
	if rec := f.do(request); rec.Code != http.StatusForbidden {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.feedCount(t); got != 1 {
		t.Fatalf("a forged POST unsubscribed the feed: %d", got)
	}
}

func TestCookieAuthenticatedPostNeedsTheCSRFToken(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	for _, prefix := range []string{"", greaderPrefix} {
		request := grRequest("POST", prefix+grBase+"subscription/edit", f.unsubscribeFields())
		request.AddCookie(f.session)
		request.Header.Set("Sec-Fetch-Site", "same-origin")
		if rec := f.do(request); rec.Code != http.StatusForbidden {
			t.Errorf("%q = %d", prefix, rec.Code)
		}
	}
	if got := f.feedCount(t); got != 1 {
		t.Fatalf("a POST without the token unsubscribed the feed: %d", got)
	}
}

func TestBearerClientNeedsNoCSRFToken(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	token := f.clientLogin(t)
	rec := f.do(withToken(grRequest("POST", grBase+"subscription/edit", f.unsubscribeFields()), token))
	if rec.Code != 200 {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
}

func TestClientLoginIsExemptFromTheCSRFGuard(t *testing.T) {
	f := newGReader(t)
	request := grRequest("POST", "/accounts/ClientLogin", url.Values{"Email": {"root"}, "Passwd": {testPassword}})
	request.AddCookie(f.session)
	request.Header.Set("Sec-Fetch-Site", "same-origin")
	if rec := f.do(request); rec.Code != 200 {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
}
