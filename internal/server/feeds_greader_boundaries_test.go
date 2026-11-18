package server

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/views"
)

func TestFeedUpdatedDisplayTreatsTheLastFreshDayAsFresh(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 6, 10, 12, 0, 0, 0, time.UTC)
	cases := []struct {
		name string
		age  time.Duration
		want string
	}{
		{"exactly the freshness limit", freshMaxDays * 24 * time.Hour, "No date info"},
		{"one second past the limit still rounds down", freshMaxDays*24*time.Hour + time.Second, "No date info"},
		{"a whole day past the limit", (freshMaxDays + 1) * 24 * time.Hour, "Never"},
	}
	for _, tc := range cases {
		fetchedAt := now.Add(-tc.age)
		got, _ := feedUpdatedDisplay(models.Feed{FetchedAt: &fetchedAt}, now)
		if got != tc.want {
			t.Errorf("%s: %q, want %q", tc.name, got, tc.want)
		}
	}
	if got, _ := feedUpdatedDisplay(models.Feed{}, now); got != "Never" {
		t.Errorf("never fetched: %q", got)
	}
}

func TestFeedRowsWithTheSameTitleKeepTheirOrder(t *testing.T) {
	t.Parallel()
	rows := []views.FeedRow{
		{ID: 1, Title: "News"}, {ID: 2, Title: "news"}, {ID: 3, Title: "NEWS"},
	}
	got := filterAndSortRows(rows, 0, "all", "title")
	for i, row := range got {
		if row.ID != int64(i+1) {
			t.Fatalf("order = %+v, want the input order", got)
		}
	}
}

func TestSortIDPadsToEightHexDigits(t *testing.T) {
	t.Parallel()
	cases := map[int64]string{
		1:           "00000001",
		255:         "000000ff",
		0x1234567:   "01234567",
		0x12345678:  "12345678",
		0x123456789: "123456789",
	}
	for id, want := range cases {
		if got := sortID(id); got != want {
			t.Errorf("sortID(%d) = %q, want %q", id, got, want)
		}
	}
}

func TestMarkAllLimitsOnlyBoundTheAgeOfAPastTimestamp(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 6, 10, 12, 0, 0, 0, time.UTC)
	usec := func(offset time.Duration) string {
		return strconv.FormatInt(now.Add(offset).UnixMicro(), 10)
	}
	cases := []struct {
		name     string
		ts       string
		wantDays *int64
	}{
		{"now is not older than anything", usec(0), nil},
		{"the future is not older than anything", usec(time.Hour), nil},
		{"one second ago is zero whole days", usec(-time.Second), ptr(int64(0))},
		{"three days and a bit", usec(-72*time.Hour - time.Minute), ptr(int64(3))},
		{"unparsable", "soon", nil},
	}
	for _, tc := range cases {
		limits, err := markAllLimits(url.Values{"ts": {tc.ts}}, now)
		if err != nil {
			t.Fatalf("%s: %v", tc.name, err)
		}
		switch {
		case tc.wantDays == nil && limits.OlderThanDays != nil:
			t.Errorf("%s: days = %d, want none", tc.name, *limits.OlderThanDays)
		case tc.wantDays != nil && (limits.OlderThanDays == nil || *limits.OlderThanDays != *tc.wantDays):
			t.Errorf("%s: days = %v, want %d", tc.name, limits.OlderThanDays, *tc.wantDays)
		}
	}
}

func TestEditTagRoutesEachStateToItsOwnChange(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name        string
		add, remove *greader.Stream
		want        int
	}{
		{"nothing", nil, nil, 0},
		{"add starred", &greader.Stream{Kind: greader.Starred}, nil, 1},
		{"add kept-unread", &greader.Stream{Kind: greader.KeptUnread}, nil, 1},
		{"remove read", nil, &greader.Stream{Kind: greader.Read}, 1},
		{"remove starred", nil, &greader.Stream{Kind: greader.Starred}, 1},
		{"add read is a no-op", &greader.Stream{Kind: greader.Read}, nil, 0},
	}
	for _, tc := range cases {
		if got := len(tagChanges(tc.add, tc.remove)); got != tc.want {
			t.Errorf("%s: %d changes, want %d", tc.name, got, tc.want)
		}
	}
}

func TestKeptUnreadBringsBackAReadEntryButStarringDoesNot(t *testing.T) {
	app, data := seededEntryApp(t)
	app.markRead(data.entryIDs[0])
	app.starEntry(data.entryIDs[0])
	if got := app.unreadCount(); got != 4 {
		t.Fatalf("unread after read + star = %d, want 4", got)
	}
	app.editTag(true, "user/-/state/com.google/kept-unread", data.entryIDs[0])
	if got := app.unreadCount(); got != 5 {
		t.Errorf("unread after kept-unread = %d, want 5", got)
	}
}

func TestItemTimestampIsThePublishedSecondInMicroseconds(t *testing.T) {
	app := defaultITestApp(t)
	published := time.Date(2026, 3, 1, 12, 0, 7, 0, time.UTC)
	data := setupEntryDataWith(t, app, func(int) (time.Time, *string) { return published, nil })
	app.Login("testuser")
	item := app.itemOf(data.entryIDs[0])
	want := strconv.FormatInt(published.Unix()*1_000_000, 10)
	if item["timestampUsec"] != want || item["published"] != float64(published.Unix()) {
		t.Errorf("timestampUsec = %v, published = %v, want %s and %d", item["timestampUsec"], item["published"], want, published.Unix())
	}
}

func TestItemIDsContinueOnlyWhenMoreRemain(t *testing.T) {
	app, _ := seededEntryApp(t)
	for _, tc := range []struct {
		count        int
		wantContinue bool
	}{{4, true}, {5, false}, {6, false}} {
		body := app.getJSONBody(fmt.Sprintf("/reader/api/0/stream/items/ids?s=%s&n=%d", readingTag, tc.count))
		if _, got := body["continuation"]; got != tc.wantContinue {
			t.Errorf("n=%d over 5 entries: continuation present = %v, want %v", tc.count, got, tc.wantContinue)
		}
	}
}

// primeSidebar caches a chrome for the user so a later Bust is observable.
func primeSidebar(t *testing.T, app *itestApp, userID int64) {
	t.Helper()
	app.App.Sidebar.InsertIfCurrent(userID, app.App.Sidebar.BeginRead(userID), cache.Chrome{TotalUnread: 9})
	if app.App.Sidebar.Get(userID) == nil {
		t.Fatal("the cache refused the seed")
	}
}

func TestSubscribingBustsTheSidebarOnlyOnSuccess(t *testing.T) {
	app := authedApp(t)
	categoryID := libraryOwner{app: app, userID: 1}.category(t, "Tech")
	feedURL := mockFeedServer(t)
	subscribe := func() {
		wantSeeOther(t, app.postPairs("/feeds", "url", feedURL, "category_id", fmt.Sprint(categoryID)), "/feeds")
	}

	primeSidebar(t, app, 1)
	subscribe()
	if app.App.Sidebar.Get(1) != nil {
		t.Error("a new subscription left the sidebar stale")
	}
	primeSidebar(t, app, 1)
	subscribe()
	if app.App.Sidebar.Get(1) == nil {
		t.Error("a refused duplicate busted the sidebar")
	}
}

// changingFeedServer serves one item whose content is whatever the counter says.
func changingFeedServer(t *testing.T, version *atomic.Int64) string {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		fmt.Fprintf(w, `<?xml version="1.0"?><rss version="2.0"><channel><title>T</title><link>https://e</link>
<item><guid>g1</guid><title>One</title><link>https://e/1</link><description>version %d</description></item>
</channel></rss>`, version.Load())
	}))
	t.Cleanup(server.Close)
	return server.URL
}

func TestRefreshBustsTheSidebarOnlyWhenEntriesChanged(t *testing.T) {
	app := authedApp(t)
	var version atomic.Int64
	_, feedID := insertTestFeed(t, app, "Tech", changingFeedServer(t, &version))
	refresh := func() {
		wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/refresh", feedID)), "/feeds")
	}
	steps := []struct {
		name       string
		bumpFirst  bool
		wantBusted bool
	}{
		{"a new entry", false, true},
		{"nothing changed", false, false},
		{"an updated entry", true, true},
	}
	for _, step := range steps {
		if step.bumpFirst {
			version.Add(1)
		}
		primeSidebar(t, app, 1)
		refresh()
		if busted := app.App.Sidebar.Get(1) == nil; busted != step.wantBusted {
			t.Errorf("%s: busted = %v, want %v", step.name, busted, step.wantBusted)
		}
	}
}

func TestFetchMetadataReportsSuccessAndFailureAsSuch(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", mockFeedServer(t))
	response := app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/fetch-metadata", feedID))
	if got := flashText(t, response); !strings.Contains(got, "Metadata fetched.") || strings.Contains(got, "Failed") {
		t.Errorf("flash = %s", got)
	}
}

// truncatedMultipart is a multipart body whose last part is cut off inside its
// headers, so the reader meets an unexpected end, not the closing boundary.
func truncatedMultipart(fields [][2]string) *http.Request {
	const boundary = "BOUNDARY"
	var body strings.Builder
	for _, field := range fields {
		fmt.Fprintf(&body, "--%s\r\nContent-Disposition: form-data; name=%q\r\n\r\n%s\r\n", boundary, field[0], field[1])
	}
	fmt.Fprintf(&body, "--%s\r\nContent-Disp", boundary)
	request := httptest.NewRequest(http.MethodPost, "/feeds/import", strings.NewReader(body.String()))
	request.Header.Set("Content-Type", "multipart/form-data; boundary="+boundary)
	return request
}

func TestOPMLUploadCutShortIsAcceptedOnlyWhenSomethingWasRead(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name   string
		fields [][2]string
		want   bool
	}{
		{"content read before the cut", [][2]string{{"content", "<opml/>"}}, true},
		{"token read before the cut", [][2]string{{"_csrf", "token"}}, true},
		{"nothing usable read", [][2]string{{"other", "x"}}, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			upload, ok := readOPMLUpload(truncatedMultipart(tc.fields))
			if ok != tc.want {
				t.Errorf("ok = %v, want %v (upload %+v)", ok, tc.want, upload)
			}
		})
	}
}
