package opml

import (
	"context"
	"encoding/json"
	"log/slog"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
)

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

func readVectors(t *testing.T, name string, into any) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", name))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, into); err != nil {
		t.Fatal(err)
	}
}

func TestExportMatchesRust(t *testing.T) {
	var v struct {
		Categories []struct {
			ID   int64
			Name string
		}
		Feeds []struct {
			CategoryID int64   `json:"category_id"`
			SiteURL    *string `json:"site_url"`
			Title      *string
			URL        string
		}
		Output      string
		EmptyOutput string `json:"empty_output"`
	}
	readVectors(t, "opml_export.json", &v)
	var categories []models.Category
	for _, c := range v.Categories {
		categories = append(categories, models.Category{ID: c.ID, Name: c.Name})
	}
	var feeds []models.Feed
	for _, f := range v.Feeds {
		feeds = append(feeds, models.Feed{CategoryID: f.CategoryID, SiteURL: f.SiteURL, Title: f.Title, URL: f.URL})
	}
	if got := Export(categories, feeds); got != v.Output {
		t.Fatalf("export differs:\n%s\nwant:\n%s", got, v.Output)
	}
	if got := Export(nil, nil); got != v.EmptyOutput {
		t.Fatalf("empty export = %q, want %q", got, v.EmptyOutput)
	}
}

type outlineJSON struct {
	Category string `json:"category"`
	Feeds    []struct {
		HTMLURL *string `json:"html_url"`
		Title   *string `json:"title"`
		XMLURL  string  `json:"xml_url"`
	} `json:"feeds"`
}

func (o outlineJSON) toOutline() Outline {
	out := Outline{CategoryName: o.Category}
	for _, f := range o.Feeds {
		out.Feeds = append(out.Feeds, Feed{Title: f.Title, XMLURL: f.XMLURL, HTMLURL: f.HTMLURL})
	}
	return out
}

func TestParseMatchesRust(t *testing.T) {
	var cases []struct {
		Input    string
		Outlines []outlineJSON
		Error    *string
	}
	readVectors(t, "opml_parse_cases.json", &cases)
	for _, c := range cases {
		got, err := Parse(c.Input)
		if c.Error != nil {
			if err == nil || err.Error() != *c.Error {
				t.Errorf("%q: err = %v, want %q", c.Input, err, *c.Error)
			}
			continue
		}
		if err != nil {
			t.Errorf("%q: %v", c.Input, err)
			continue
		}
		var want []Outline
		for _, o := range c.Outlines {
			want = append(want, o.toOutline())
		}
		if !reflect.DeepEqual(got, want) {
			t.Errorf("%q:\n got %+v\nwant %+v", c.Input, got, want)
		}
	}
}

func shapeOf(outlines []Outline) map[string][]string {
	shape := map[string][]string{}
	for _, o := range outlines {
		for _, f := range o.Feeds {
			shape[o.CategoryName] = append(shape[o.CategoryName], f.XMLURL)
		}
	}
	return shape
}

func TestParseNestedAndStartFeeds(t *testing.T) {
	content := `<opml version="2.0">
  <body>
    <outline type="rss" text="Loose" xmlUrl="https://example.com/loose"/>
    <outline type="rss" text="Open" xmlUrl="https://example.com/open"></outline>
    <outline text="Empty category"/>
    <outline text="Outer">
      <outline type="rss" title="Before" xmlUrl="https://example.com/before"/>
      <outline text="Inner">
        <outline type="rss" text="Inside" xmlUrl="https://example.com/inside"/>
      </outline>
    </outline>
    <outline text="No feeds"></outline>
  </body>
</opml>`
	got, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	var order []string
	for _, o := range got {
		order = append(order, o.CategoryName+"="+o.Feeds[0].XMLURL)
	}
	want := []string{
		"Uncategorized=https://example.com/loose", "Uncategorized=https://example.com/open",
		"Outer=https://example.com/before", "Inner=https://example.com/inside",
	}
	if !reflect.DeepEqual(order, want) {
		t.Fatalf("order = %v, want %v", order, want)
	}
}

func TestEmptySelfClosingCategoryDoesNotCaptureFeeds(t *testing.T) {
	content := `<opml><body><outline text="A"><outline text="Sub"/><outline xmlUrl="https://x/1"/></outline></body></opml>`
	got, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	if want := map[string][]string{"A": {"https://x/1"}}; !reflect.DeepEqual(shapeOf(got), want) {
		t.Fatalf("shape = %v", shapeOf(got))
	}
}

func TestParsePrefersTextForCategoriesAndTitleForFeeds(t *testing.T) {
	content := `<opml version="2.0"><body>
<outline text="Category text" title="Category title">
<outline type="rss" text="Feed text" title="Feed title" xmlUrl="https://example.com/a"/>
<outline type="rss" text="Open text" title="Open title" xmlUrl="https://example.com/b"></outline>
</outline></body></opml>`
	got, err := Parse(content)
	if err != nil || len(got) != 1 || got[0].CategoryName != "Category text" {
		t.Fatalf("got = %+v, err = %v", got, err)
	}
	if *got[0].Feeds[0].Title != "Feed title" || *got[0].Feeds[1].Title != "Open title" {
		t.Fatalf("feeds = %+v", got[0].Feeds)
	}
}

func TestParseUnescapesAttributeEntitiesOnce(t *testing.T) {
	content := `<opml><body><outline text="News &amp; Views"><outline text="Tom &amp; Jerry" xmlUrl="https://x/?a=1&amp;b=2"/></outline></body></opml>`
	got, err := Parse(content)
	if err != nil {
		t.Fatal(err)
	}
	if got[0].CategoryName != "News & Views" || got[0].Feeds[0].XMLURL != "https://x/?a=1&b=2" || *got[0].Feeds[0].Title != "Tom & Jerry" {
		t.Fatalf("got = %+v", got)
	}
}

func TestParseAttributeNamesAreCaseInsensitiveButNamespacesAreNot(t *testing.T) {
	content := `<opml><body><outline TEXT="Cat"><outline XMLURL="https://x/1" x:xmlUrl="https://x/no"/></outline></body></opml>`
	got, err := Parse(content)
	if err != nil || got[0].CategoryName != "Cat" || got[0].Feeds[0].XMLURL != "https://x/1" {
		t.Fatalf("got = %+v, err = %v", got, err)
	}
}

func TestParseWithDefaultNamespaceAndEncodingDeclaration(t *testing.T) {
	content := `<?xml version="1.0" encoding="ISO-8859-1"?><opml xmlns="http://opml.org/spec2"><body><outline xmlUrl="https://x/1"/></body></opml>`
	if got, err := Parse(content); err != nil || len(got) != 1 {
		t.Fatalf("got = %+v, err = %v", got, err)
	}
}

func TestParseIgnoresOutlinesOutsideBody(t *testing.T) {
	content := `<opml><head><outline xmlUrl="https://x/head"/></head><body></body><outline xmlUrl="https://x/after"/></opml>`
	_, err := Parse(content)
	if err == nil || err.Error() != "Invalid OPML format: No feeds found in OPML" {
		t.Fatalf("err = %v", err)
	}
}

func TestParseErrors(t *testing.T) {
	for _, bad := range []string{`<not-opml></not-opml>`, `<opml><body></opml>`, `</body>`, `<opml><body><outline xmlUrl="a"></opml>`} {
		_, err := Parse(bad)
		if err == nil {
			t.Errorf("%q: want error", bad)
		}
	}
	_, err := Parse(`<opml><body></outline></body></opml>`)
	if err == nil || !strings.HasPrefix(err.Error(), "Invalid OPML format: Error at position ") {
		t.Fatalf("err = %v", err)
	}
}

func TestDescribe(t *testing.T) {
	cases := []struct {
		summary ImportSummary
		want    string
	}{
		{ImportSummary{FeedsAdded: 12}, "OPML imported: 12 feeds added."},
		{ImportSummary{FeedsAdded: 1}, "OPML imported: 1 feed added."},
		{ImportSummary{}, "OPML imported: 0 feeds added."},
		{ImportSummary{FeedsAdded: 2, FeedsSkipped: 3, FeedsFailed: 4}, "OPML imported: 2 feeds added, 3 already subscribed, 4 failed."},
		{ImportSummary{FeedsSkipped: 1}, "OPML imported: 0 feeds added, 1 already subscribed."},
		{ImportSummary{FeedsFailed: 1}, "OPML imported: 0 feeds added, 1 failed."},
	}
	for _, c := range cases {
		if got := c.summary.Describe(); got != c.want {
			t.Errorf("got %q, want %q", got, c.want)
		}
	}
}

func openDB(t *testing.T) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if err := migrate.Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	return database
}

func str(s string) *string { return &s }

func TestImportRefusesUnfetchableURLsAndSkipsDuplicates(t *testing.T) {
	ctx := context.Background()
	database := openDB(t)
	user, err := models.CreateUser(ctx, database, "opmluser", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	outlines := []Outline{{CategoryName: "Imported", Feeds: []Feed{
		{Title: str("Public"), XMLURL: "https://example.com/feed.xml", HTMLURL: str("https://example.com/")},
		{Title: str("Metadata service"), XMLURL: "http://169.254.169.254/latest/meta-data/"},
		{Title: str("Not a URL"), XMLURL: "file:///etc/passwd"},
		{Title: str("Again"), XMLURL: "https://example.com/feed.xml"},
	}}}
	params := ImportParams{DB: database, UserID: user.ID, Outlines: outlines, Fetcher: fetch.NewFetcher(fetch.Policy{})}
	summary, err := Import(ctx, params)
	if err != nil {
		t.Fatal(err)
	}
	want := ImportSummary{CategoriesCreated: 1, FeedsAdded: 1, FeedsSkipped: 1, FeedsFailed: 2}
	if summary != want {
		t.Fatalf("summary = %+v, want %+v", summary, want)
	}
	stored, err := models.ListFeedsByUser(ctx, database, user.ID)
	if err != nil || len(stored) != 1 || stored[0].URL != "https://example.com/feed.xml" || *stored[0].Title != "Public" {
		t.Fatalf("stored = %+v, err = %v", stored, err)
	}
	again, err := Import(ctx, params)
	if err != nil || again.CategoriesCreated != 0 || again.FeedsSkipped != 2 || again.FeedsFailed != 2 {
		t.Fatalf("again = %+v, err = %v", again, err)
	}
}
