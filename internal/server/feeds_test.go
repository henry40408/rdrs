package server

import (
	"bytes"
	"context"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
)

// withFeed adds a category and a feed for the signed-in admin.
func (f *signedInFixture) withFeed(t *testing.T) (categoryID, feedID int64) {
	t.Helper()
	ctx := context.Background()
	category, err := models.CreateCategory(ctx, f.database, 1, "Tech")
	if err != nil {
		t.Fatal(err)
	}
	title := "Example"
	feed, err := models.CreateFeed(ctx, f.database, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/feed.xml", Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	return category.ID, feed.ID
}

func (f *signedInFixture) flashOf(rec *httptest.ResponseRecorder) string {
	request := httptest.NewRequest(http.MethodGet, "/feeds", nil)
	request.AddCookie(f.session)
	if c := cookieNamed(rec, flash.CookieName); c != nil {
		request.AddCookie(c)
	}
	return f.do(request).Body.String()
}

func TestFeedFormsFlashTheirOutcome(t *testing.T) {
	f := newSignedIn(t)
	_, feedID := f.withFeed(t)
	id := "/feeds/" + itoa(feedID)
	cases := []struct {
		name, path string
		fields     url.Values
		location   string
		text       string
	}{
		{"empty url", "/feeds", url.Values{"url": {"  "}, "category_id": {"1"}}, "/feeds", "Feed URL cannot be empty"},
		{"foreign category", "/feeds", url.Values{"url": {"https://a.example/"}, "category_id": {"99"}}, "/feeds", "Invalid category"},
		{"edit empty url", id + "/edit", url.Values{"url": {""}, "category_id": {"1"}}, id + "/edit", "Feed URL cannot be empty"},
		{"edit private host", id + "/edit", url.Values{"url": {"http://127.0.0.1/x"}, "category_id": {"1"}}, id + "/edit",
			"Feed URL must be an http(s) address that does not point to a private or local host"},
		{"edit keeps back", id + "/edit", url.Values{"url": {""}, "category_id": {"1"}, "return_to": {"/feeds?sort=unread"}},
			id + "/edit?return_to=%2Ffeeds%3Fsort%3Dunread", "Feed URL cannot be empty"},
		{"delete unknown", "/feeds/99/delete", url.Values{}, "/feeds", "Feed not found."},
		{"refresh unknown", "/feeds/99/refresh", url.Values{}, "/feeds", "Feed not found"},
		{"metadata unknown", "/feeds/99/fetch-metadata", url.Values{}, "/feeds", "Feed not found"},
		{"delete", id + "/delete", url.Values{}, "/feeds", "Feed deleted."},
	}
	for _, c := range cases {
		rec := f.postForm(c.path, c.fields)
		if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != c.location {
			t.Errorf("%s: %d %q, want 303 %q", c.name, rec.Code, rec.Header().Get("Location"), c.location)
			continue
		}
		if body := f.flashOf(rec); !strings.Contains(body, c.text) {
			t.Errorf("%s: page lacks flash %q", c.name, c.text)
		}
	}
}

func TestFeedFormWithoutFieldsIs422(t *testing.T) {
	f := newSignedIn(t)
	rec := f.postForm("/feeds", url.Values{"category_id": {"1"}})
	if rec.Code != http.StatusUnprocessableEntity {
		t.Errorf("status = %d, want 422", rec.Code)
	}
}

func TestEditingAFeedKeepsUnsentFieldsAndClearsBlankOnes(t *testing.T) {
	f := newSignedIn(t)
	categoryID, feedID := f.withFeed(t)
	rec := f.postForm("/feeds/"+itoa(feedID)+"/edit", url.Values{
		"url": {"https://example.com/new.xml"}, "category_id": {itoa(categoryID)},
		"title": {"  "}, "description": {" About "}, "http2_disabled": {"on"},
	})
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/feeds" {
		t.Fatalf("edit = %d %q", rec.Code, rec.Header().Get("Location"))
	}
	feed, err := models.FindFeedByID(context.Background(), f.database, feedID)
	if err != nil || feed == nil {
		t.Fatal(err)
	}
	if feed.URL != "https://example.com/new.xml" || valueOr(feed.Title) != "Example" ||
		valueOr(feed.Description) != "About" || !feed.HTTP2Disabled {
		t.Errorf("feed = %+v", feed)
	}
}

func TestFeedPagesOfAnotherUsersFeedAreNotFound(t *testing.T) {
	f := newSignedIn(t)
	if rec := f.get("/feeds/99/edit"); rec.Code != http.StatusNotFound {
		t.Errorf("edit page = %d, want 404", rec.Code)
	}
	if rec := f.get("/api/feeds/99/icon"); rec.Code != http.StatusNotFound {
		t.Errorf("icon = %d, want 404", rec.Code)
	}
	_, feedID := f.withFeed(t)
	rec := f.get("/api/feeds/" + itoa(feedID) + "/icon")
	if rec.Code != http.StatusNotFound || !strings.Contains(rec.Body.String(), "Icon not found") {
		t.Errorf("missing icon = %d %s", rec.Code, rec.Body)
	}
}

func TestFeedIconIsPrivatelyCached(t *testing.T) {
	f := newSignedIn(t)
	_, feedID := f.withFeed(t)
	_, err := f.database.ExecContext(context.Background(),
		`INSERT INTO image (entity_type, entity_id, data, content_type, fetched_at) VALUES ('feed', $1, $2, 'image/png', CURRENT_TIMESTAMP)`,
		feedID, []byte("png"))
	if err != nil {
		t.Fatal(err)
	}
	rec := f.get("/api/feeds/" + itoa(feedID) + "/icon")
	if rec.Code != http.StatusOK || rec.Header().Get("Cache-Control") != "private, max-age=86400" ||
		rec.Header().Get("Content-Type") != "image/png" || rec.Body.String() != "png" {
		t.Errorf("icon = %d %v %q", rec.Code, rec.Header(), rec.Body)
	}
}

func (f *signedInFixture) multipartImport(fields map[string]string, header string) *httptest.ResponseRecorder {
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	for name, value := range fields {
		_ = writer.WriteField(name, value)
	}
	_ = writer.Close()
	request := httptest.NewRequest(http.MethodPost, "/feeds/import", &body)
	request.Header.Set("Content-Type", writer.FormDataContentType())
	if header != "" {
		request.Header.Set("X-CSRF-Token", header)
	}
	request.AddCookie(f.session)
	return f.do(request)
}

func TestOPMLImportChecksTheCSRFTokenItself(t *testing.T) {
	f := newSignedIn(t)
	if rec := f.multipartImport(map[string]string{"content": "<opml/>"}, ""); rec.Code != http.StatusForbidden {
		t.Errorf("no token = %d, want 403", rec.Code)
	}
	if rec := f.multipartImport(map[string]string{"content": "<opml/>", "_csrf": "bad"}, f.csrf); rec.Code != http.StatusForbidden {
		t.Errorf("bad field overriding a good header = %d, want 403", rec.Code)
	}
	rec := f.multipartImport(map[string]string{"content": "  "}, f.csrf)
	if rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/feeds/import" {
		t.Errorf("empty content = %d %q", rec.Code, rec.Header().Get("Location"))
	}
	rec = f.multipartImport(map[string]string{"content": "<nope", "_csrf": f.csrf}, "")
	if rec.Code != http.StatusSeeOther || !strings.Contains(f.flashOf(rec), "Failed to parse OPML") {
		t.Errorf("bad opml = %d", rec.Code)
	}
}

func TestFeedsPageNormalisesItsQuery(t *testing.T) {
	f := newSignedIn(t)
	f.withFeed(t)
	body := f.get("/feeds?sort=%22x&filter=%3Cb%3E&category=abc").Body.String()
	if strings.Contains(body, "<b>") || !strings.Contains(body, "sort=title&amp;filter=all") {
		t.Errorf("query not normalised")
	}
}

func TestReferrerSuggestionsAreDeduplicatedOrigins(t *testing.T) {
	site := " https://example.com/blog "
	got := referrerSuggestions(&site, "https://example.com:8443/feed.xml")
	want := []string{"https://example.com/", "https://example.com:8443/"}
	if strings.Join(got, ",") != strings.Join(want, ",") {
		t.Errorf("got %v, want %v", got, want)
	}
	if got := referrerSuggestions(nil, "ftp://x/y"); len(got) != 0 {
		t.Errorf("ftp = %v", got)
	}
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }
