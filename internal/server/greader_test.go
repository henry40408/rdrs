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

func (f *grFixture) clientLoginAs(email, password string) *httptest.ResponseRecorder {
	return f.do(grRequest("POST", "/accounts/ClientLogin", url.Values{"Email": {email}, "Passwd": {password}}))
}

func TestClientLoginTrimsUsernameWhitespace(t *testing.T) {
	f := newGReader(t)
	if rec := f.clientLoginAs("root ", testPassword); rec.Code != http.StatusOK {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
}

func TestGReaderClientLoginSuccess(t *testing.T) {
	f := newGReader(t)
	rec := f.clientLoginAs("root", testPassword)
	body := rec.Body.String()
	if rec.Code != http.StatusOK || !strings.Contains(body, "SID=") || !strings.Contains(body, "LSID=") ||
		!strings.Contains(body, "Auth="+models.APITokenPrefix) {
		t.Fatalf("ClientLogin = %d %q", rec.Code, body)
	}
}

func TestClientLoginInvalidPassword(t *testing.T) {
	f := newGReader(t)
	if rec := f.clientLoginAs("root", "wrong"); rec.Code != http.StatusUnauthorized {
		t.Fatalf("= %d", rec.Code)
	}
}

func TestGReaderClientLoginNonexistentUser(t *testing.T) {
	f := newGReader(t)
	if rec := f.clientLoginAs("nobody", testPassword); rec.Code != http.StatusUnauthorized {
		t.Fatalf("= %d", rec.Code)
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

func TestWebSessionCookieStillWorksForGreader(t *testing.T) {
	f := newGReader(t)
	if rec := f.browserGet(grBase + "subscription/list"); rec.Code != http.StatusOK {
		t.Fatalf("the session no longer works: %d", rec.Code)
	}
}

func TestWebSessionTokenIsRejectedInTheAuthorizationHeader(t *testing.T) {
	f := newGReader(t)
	for name, token := range map[string]string{"bare": f.sessionToken(), "signed": f.session.Value} {
		rec := f.do(withToken(grRequest("GET", grBase+"subscription/list", nil), token))
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("%s = %d", name, rec.Code)
		}
	}
	// The session survives: the header path rejected it, it was not deleted.
	if got := f.count(t, "SELECT COUNT(*) FROM session"); got == 0 {
		t.Fatal("the session was deleted instead of rejected")
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

// attempts fires n failed ClientLogins under a prefix and returns the last status.
func (f *grFixture) failedLogins(prefix string, n int) int {
	status := 0
	for range n {
		status = f.do(grRequest("POST", prefix+"/accounts/ClientLogin",
			url.Values{"Email": {"root"}, "Passwd": {"wrong"}})).Code
	}
	return status
}

func TestClientLoginRateLimited(t *testing.T) {
	f := newGReader(t)
	f.app.Limiter = auth.NewRateLimiter(5, 60)
	if got := f.failedLogins("", 5); got != http.StatusUnauthorized {
		t.Fatalf("5th attempt = %d", got)
	}
	if got := f.failedLogins("", 1); got != http.StatusTooManyRequests {
		t.Errorf("6th attempt = %d, want it throttled", got)
	}
}

func TestClientLoginRateLimitAppliesToGreaderPhpPrefix(t *testing.T) {
	f := newGReader(t)
	f.app.Limiter = auth.NewRateLimiter(5, 60)
	if got := f.failedLogins(greaderPrefix, 5); got != http.StatusUnauthorized {
		t.Fatalf("5th attempt = %d", got)
	}
	if got := f.failedLogins(greaderPrefix, 1); got != http.StatusTooManyRequests {
		t.Errorf("6th attempt = %d, want it throttled", got)
	}
}

// Go-only: both prefixes draw on one bucket.
func TestClientLoginPrefixesShareOneRateLimitBucket(t *testing.T) {
	f := newGReader(t)
	f.app.Limiter = auth.NewRateLimiter(5, 60)
	f.failedLogins("", 3)
	f.failedLogins(greaderPrefix, 2)
	for _, prefix := range []string{"", greaderPrefix} {
		if got := f.failedLogins(prefix, 1); got != http.StatusTooManyRequests {
			t.Errorf("%q after the budget = %d", prefix, got)
		}
	}
}

// --- subscriptions ---

func TestGReaderSubscriptionListEmpty(t *testing.T) {
	f := newGReader(t)
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))["subscriptions"].([]any)
	if len(list) != 0 {
		t.Fatalf("subscriptions = %v", list)
	}
}

func TestGReaderSubscriptionListWithFeeds(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))["subscriptions"].([]any)
	if len(list) != 1 {
		t.Fatalf("subscriptions = %v", list)
	}
	first := list[0].(map[string]any)
	categories := first["categories"].([]any)
	if !strings.HasPrefix(first["id"].(string), "feed/") || first["title"] != "Example" ||
		len(categories) != 1 || categories[0].(map[string]any)["label"] != "Tech" {
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

func TestSubscriptionEditRename(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	rec := f.edit(url.Values{"ac": {"edit"}, "s": {"feed/https://example.com/feed.xml"}, "t": {"Renamed"}})
	if rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("rename = %d %q", rec.Code, rec.Body)
	}
	list := jsonOf(t, f.browserGet(grBase+"subscription/list"))["subscriptions"].([]any)
	if list[0].(map[string]any)["title"] != "Renamed" {
		t.Fatalf("not renamed: %v", list)
	}
}

func TestSubscriptionEditUnsubscribe(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	rec := f.edit(url.Values{"ac": {"unsubscribe"}, "s": {"feed/https://example.com/feed.xml"}})
	if rec.Code != 200 {
		t.Fatalf("unsubscribe = %d", rec.Code)
	}
	if got := f.feedCount(t); got != 0 {
		t.Fatalf("%d feeds left", got)
	}
}

// wantEditStatus posts a subscription/edit form and checks the status code.
func (f *grFixture) wantEditStatus(t *testing.T, fields url.Values, want int) {
	t.Helper()
	if rec := f.edit(fields); rec.Code != want {
		t.Fatalf("= %d, want %d (%s)", rec.Code, want, rec.Body)
	}
}

func TestSubscriptionEditAnswersOk(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	f.wantEditStatus(t, url.Values{"ac": {"edit"}, "s": {"feed/https://example.com/feed.xml"}}, 200)
}

func TestSubscriptionEditFeedNotFound(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"edit"}, "s": {"feed/https://nope.example/rss"}}, 404)
}

func TestSubscriptionEditRejectsAStreamIdWithoutFeedPrefix(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	f.wantEditStatus(t, url.Values{"ac": {"edit"}, "s": {"https://example.com/feed.xml"}}, 400)
}

func TestSubscriptionEditRequiresAStreamId(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"edit"}}, 400)
}

func TestSubscriptionUnsubscribeOfAnUnknownFeedIsNotFound(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"unsubscribe"}, "s": {"feed/https://nope.example/rss"}}, 404)
}

// The remaining refusals have no Rust counterpart of their own; they keep the
// validation table that guards the unmapped branches.
func TestSubscriptionEditValidation(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	cases := map[string]struct {
		fields url.Values
		status int
	}{
		"unsubscribe without s": {url.Values{"ac": {"unsubscribe"}}, 400},
		"unsubscribe bare url":  {url.Values{"ac": {"unsubscribe"}, "s": {"https://example.com/feed.xml"}}, 400},
		"unknown action":        {url.Values{"ac": {"explode"}}, 400},
		"missing action":        {url.Values{"s": {"feed/x"}}, 422},
		"bad http2 flag":        {url.Values{"ac": {"edit"}, "s": {"feed/https://example.com/feed.xml"}, "http2_disabled": {"yes"}}, 422},
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

func TestSubscriptionEditAddLabelMovesCategory(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	const feedURL = "https://example.com/feed.xml"
	f.wantEditStatus(t, url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/NewCat"}}, 200)
	if got := f.categoryNameOf(t, feedURL); got != "NewCat" {
		t.Fatalf("category = %q", got)
	}
}

func TestSubscriptionEditExistingLabelReusesItsCategory(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	const feedURL = "https://example.com/feed.xml"
	target, err := models.CreateCategory(context.Background(), f.database, 1, "Target")
	if err != nil {
		t.Fatal(err)
	}
	f.wantEditStatus(t, url.Values{"ac": {"edit"}, "s": {"feed/" + feedURL}, "a": {"user/-/label/Target"}}, 200)
	if got := f.feed(t, feedURL).CategoryID; got != target.ID {
		t.Fatalf("category = %d, want %d", got, target.ID)
	}
	categories, _ := models.ListCategoriesByUser(context.Background(), f.database, 1)
	count := 0
	for _, category := range categories {
		if category.Name == "Target" {
			count++
		}
	}
	if count != 1 {
		t.Fatalf("%d Target categories", count)
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

// subscribe posts a subscribe for feedURL, with optional extra fields.
func (f *grFixture) subscribe(feedURL string, extra url.Values) *httptest.ResponseRecorder {
	fields := url.Values{"ac": {"subscribe"}, "s": {"feed/" + feedURL}}
	for key, values := range extra {
		fields[key] = values
	}
	return f.edit(fields)
}

func (f *grFixture) quickAdd(feedURL string) *httptest.ResponseRecorder {
	return f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {feedURL}})
}

func TestQuickaddSuccess(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	body := jsonOf(t, f.quickAdd(feedURL))
	if body["numResults"] != float64(1) || body["streamId"] != "feed/"+feedURL || body["query"] != feedURL {
		t.Fatalf("quickadd = %v", body)
	}
}

func TestSubscriptionSubscribeSuccess(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	if rec := f.subscribe(feedURL, nil); rec.Code != 200 {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
	f.feed(t, feedURL)
}

func TestSubscriptionSubscribeWithLabel(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	if rec := f.subscribe(feedURL, url.Values{"a": {"user/-/label/Tech"}}); rec.Code != 200 {
		t.Fatalf("= %d %s", rec.Code, rec.Body)
	}
	if got := f.categoryNameOf(t, feedURL); got != "Tech" {
		t.Fatalf("category = %q", got)
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

// withExtraCategory adds a second category and returns the id of the first one
// and the category count, which a subscription must leave alone.
func (f *grFixture) withExtraCategory(t *testing.T) (firstID int64, count int) {
	t.Helper()
	if _, err := models.CreateCategory(context.Background(), f.database, 1, "Extra"); err != nil {
		t.Fatal(err)
	}
	categories, _ := models.ListCategoriesByUser(context.Background(), f.database, 1)
	return categories[0].ID, len(categories)
}

func (f *grFixture) wantFeedInFirstCategory(t *testing.T, feedURL string, firstID int64, count int) {
	t.Helper()
	if got := f.feed(t, feedURL).CategoryID; got != firstID {
		t.Errorf("category %d, want %d", got, firstID)
	}
	if after, _ := models.ListCategoriesByUser(context.Background(), f.database, 1); len(after) != count {
		t.Errorf("categories %d -> %d", count, len(after))
	}
}

func TestQuickaddLandsInTheFirstCategory(t *testing.T) {
	f := newGReader(t)
	firstID, count := f.withExtraCategory(t)
	feedURL := stubFeedServer(t)
	if rec := f.quickAdd(feedURL); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
	f.wantFeedInFirstCategory(t, feedURL, firstID, count)
}

func TestSubscriptionSubscribeLandsInTheFirstCategory(t *testing.T) {
	f := newGReader(t)
	firstID, count := f.withExtraCategory(t)
	feedURL := stubFeedServer(t)
	if rec := f.subscribe(feedURL, nil); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
	f.wantFeedInFirstCategory(t, feedURL, firstID, count)
}

func TestQuickaddRecreatesUncategorizedForAUserWithNone(t *testing.T) {
	f := newGReader(t)
	f.removeCategories(t)
	feedURL := stubFeedServer(t)
	if rec := f.quickAdd(feedURL); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.categoryNameOf(t, feedURL); got != "Uncategorized" {
		t.Fatalf("category = %q", got)
	}
}

func TestSubscriptionSubscribeRecreatesUncategorizedForAUserWithNone(t *testing.T) {
	f := newGReader(t)
	f.removeCategories(t)
	feedURL := stubFeedServer(t)
	if rec := f.subscribe(feedURL, nil); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.categoryNameOf(t, feedURL); got != "Uncategorized" {
		t.Fatalf("category = %q", got)
	}
}

func TestSubscriptionSubscribeBadPrefix(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"subscribe"}, "s": {"https://example.com/feed.xml"}}, 400)
}

func TestSubscriptionSubscribeDuplicate(t *testing.T) {
	f := newGReader(t)
	feedURL := stubFeedServer(t)
	f.subscribe(feedURL, nil)
	if rec := f.subscribe(feedURL, nil); rec.Code != http.StatusConflict {
		t.Fatalf("duplicate = %d", rec.Code)
	}
}

func TestSubscriptionSubscribeEmptyUrl(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"subscribe"}, "s": {"feed/"}}, 400)
}

func TestSubscriptionSubscribeMissingStreamId(t *testing.T) {
	f := newGReader(t)
	f.wantEditStatus(t, url.Values{"ac": {"subscribe"}}, 400)
}

// Quickadd refusals have no Rust counterpart; they extend the subscribe ones.
func TestQuickAddRefusesEmptyAndDuplicateURLs(t *testing.T) {
	f := newGReader(t)
	if rec := f.browserPost(grBase+"subscription/quickadd", url.Values{"quickadd": {"  "}}); rec.Code != 400 {
		t.Fatalf("empty = %d", rec.Code)
	}
	if rec := f.browserPost(grBase+"subscription/quickadd", url.Values{}); rec.Code != 422 {
		t.Fatalf("missing = %d", rec.Code)
	}
	feedURL := stubFeedServer(t)
	f.quickAdd(feedURL)
	if rec := f.quickAdd(feedURL); rec.Code != http.StatusConflict {
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

const readingListStream = grBase + "stream/contents/" + readingList

func (f *grFixture) streamItems(t *testing.T, path string) []any {
	t.Helper()
	return jsonOf(t, f.browserGet(path))["items"].([]any)
}

func TestStreamContentsNegativeNDoesNotReturnEverything(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 5)
	if got := len(f.streamItems(t, readingListStream+"?n=-2")); got != 0 {
		t.Fatalf("n=-2 returned %d items, a negative n must not bypass the limit", got)
	}
}

func TestStreamContentsReadingList(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 3)
	items := f.streamItems(t, readingListStream)
	if len(items) != 3 {
		t.Fatalf("%d items", len(items))
	}
	first := items[0].(map[string]any)
	if _, ok := first["id"].(string); !ok {
		t.Errorf("no id: %v", first)
	}
	if _, ok := first["title"].(string); !ok {
		t.Errorf("no title: %v", first)
	}
}

func TestStreamContentsStarredEmpty(t *testing.T) {
	f := newGReader(t)
	if items := f.streamItems(t, grBase+"stream/contents/user/-/state/com.google/starred"); len(items) != 0 {
		t.Fatalf("starred = %v", items)
	}
}

func TestStreamContentsWithLimit(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 5)
	page := jsonOf(t, f.browserGet(readingListStream+"?n=2"))
	if len(page["items"].([]any)) != 2 {
		t.Fatalf("page = %v", page)
	}
	if _, ok := page["continuation"].(string); !ok {
		t.Fatalf("no continuation: %v", page)
	}
}

// Go-only: following the continuation to the last page.
func TestStreamContentsPagesWithAContinuation(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 3)
	page := jsonOf(t, f.browserGet(readingListStream+"?n=2"))
	next := jsonOf(t, f.browserGet(readingListStream+"?n=2&c="+url.QueryEscape(page["continuation"].(string))))
	if len(next["items"].([]any)) != 1 || next["continuation"] != nil {
		t.Fatalf("next = %v", next)
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

const readState = "user/-/state/com.google/read"

// markRead posts edit-tag marking every id read.
func (f *grFixture) markRead(ids ...int64) *httptest.ResponseRecorder {
	refs := make([]string, len(ids))
	for i, id := range ids {
		refs[i] = itemRef(id)
	}
	return f.browserPost(grBase+"edit-tag", url.Values{"i": refs, "a": {readState}})
}

func (f *grFixture) feedWithEntries(t *testing.T, n int) []int64 {
	t.Helper()
	_, feedID := f.withFeed(t)
	return f.entries(t, feedID, n)
}

const readingList = "user/-/state/com.google/reading-list"

func TestEditTagAffectedCountExcludesAlreadyRead(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 3)
	if rec := f.markRead(ids[0]); rec.Code != 200 || rec.Header().Get("X-Rdrs-Affected") != "1" {
		t.Fatalf("first = %d %v", rec.Code, rec.Header())
	}
	rec := f.markRead(ids...)
	if rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("second = %d %q", rec.Code, rec.Body)
	}
	if got := rec.Header().Get("X-Rdrs-Affected"); got != "2" {
		t.Fatalf("affected = %q, already-read entries must not be counted again", got)
	}
}

func TestEditTagMarkRead(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 1)
	if rec := f.markRead(ids[0]); rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("= %d %q", rec.Code, rec.Body)
	}
	if got := f.unreadCount(t, readingList); got != 0 {
		t.Fatalf("unread = %v", got)
	}
}

func TestEditTagMarkReadMultiple(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 3)
	if rec := f.markRead(ids...); rec.Code != 200 || rec.Body.String() != "OK" {
		t.Fatalf("= %d %q", rec.Code, rec.Body)
	}
	if got := f.unreadCount(t, readingList); got != 0 {
		t.Fatalf("unread = %v", got)
	}
}

func TestEditTagMarkReadRejectsUnknownEntry(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 2)
	if rec := f.markRead(append(ids, 999_999)...); rec.Code != http.StatusNotFound {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.unreadCount(t, readingList); got != 2 {
		t.Fatalf("unread = %v, a refused batch must change nothing", got)
	}
}

// Go-only: un-reading and a request with no ids.
func TestEditTagUnreadsAndRequiresIDs(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 3)
	f.markRead(ids...)
	f.browserPost(grBase+"edit-tag", url.Values{"i": {itemRef(ids[0])}, "r": {readState}})
	if got := f.unreadCount(t, readingList); got != 1 {
		t.Fatalf("unread = %v", got)
	}
	if rec := f.browserPost(grBase+"edit-tag", url.Values{"a": {readState}}); rec.Code != 400 {
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

func TestMarkAllAsRead(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 5)
	if got := f.unreadCount(t, readingList); got != 5 {
		t.Fatalf("before = %v", got)
	}
	if rec := f.browserPost(grBase+"mark-all-as-read", url.Values{"s": {readingList}}); rec.Code != 200 {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.unreadCount(t, readingList); got != 0 {
		t.Fatalf("after = %v", got)
	}
}

func TestMarkAllAsReadReportsAffectedCount(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 5)
	fields := url.Values{"s": {readingList}}
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

func TestUnreadCountEmpty(t *testing.T) {
	f := newGReader(t)
	body := jsonOf(t, f.browserGet(grBase+"unread-count"))
	if body["max"] != float64(1000) || len(body["unreadcounts"].([]any)) != 2 {
		t.Fatalf("empty = %v", body)
	}
}

func TestUnreadCountWithEntries(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 3)
	if got := f.unreadCount(t, readingList); got != 3 {
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

// wantSidebarEvent posts a write and waits for the sidebar event it announces.
func (f *grFixture) wantSidebarEvent(t *testing.T, path string, fields url.Values) {
	t.Helper()
	subscription := f.app.Events.Subscribe()
	defer subscription.Close()
	if rec := f.browserPost(grBase+path, fields); rec.Code != 200 {
		t.Fatalf("%s = %d %s", path, rec.Code, rec.Body)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	event, err := subscription.Next(ctx)
	if err != nil || event.Kind != events.Sidebar || event.UserID != 1 {
		t.Errorf("%s: event %+v, err %v", path, event, err)
	}
}

func TestDisableTagEmitsSidebarEvent(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	f.wantSidebarEvent(t, "disable-tag", url.Values{"s": {"user/-/label/Tech"}})
}

func TestEditTagEmitsSidebarEvent(t *testing.T) {
	f := newGReader(t)
	ids := f.feedWithEntries(t, 1)
	f.wantSidebarEvent(t, "edit-tag", url.Values{"i": {itemRef(ids[0])}, "a": {readState}})
}

func TestMarkAllAsReadEmitsSidebarEvent(t *testing.T) {
	f := newGReader(t)
	f.feedWithEntries(t, 3)
	f.wantSidebarEvent(t, "mark-all-as-read", url.Values{"s": {readingList}})
}

func TestRenameTagEmitsSidebarEvent(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	f.wantSidebarEvent(t, "rename-tag", url.Values{"s": {"user/-/label/Tech"}, "dest": {"user/-/label/Technology"}})
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

// cookiePostWithoutToken unsubscribes through the session cookie alone, the way
// a same-origin page without the CSRF token header would.
func (f *grFixture) cookiePostWithoutToken(prefix string) *httptest.ResponseRecorder {
	request := grRequest("POST", prefix+grBase+"subscription/edit", f.unsubscribeFields())
	request.AddCookie(f.session)
	request.Header.Set("Sec-Fetch-Site", "same-origin")
	return f.do(request)
}

func TestCookieAuthenticatedGreaderPostRequiresTheCsrfToken(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	if rec := f.cookiePostWithoutToken(""); rec.Code != http.StatusForbidden {
		t.Fatalf("= %d", rec.Code)
	}
	if got := f.feedCount(t); got != 1 {
		t.Fatalf("a POST without the token unsubscribed the feed: %d", got)
	}
}

func TestGreaderPhpPrefixAlsoRequiresTheCsrfToken(t *testing.T) {
	f := newGReader(t)
	f.withFeed(t)
	if rec := f.cookiePostWithoutToken(greaderPrefix); rec.Code != http.StatusForbidden {
		t.Fatalf("= %d", rec.Code)
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
