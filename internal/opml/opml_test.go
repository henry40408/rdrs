package opml

import (
	"context"
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

const rustExportOutput = `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
<head>
<title>RDRS Subscriptions</title>
</head>
<body>
<outline text="Tech" title="Tech">
<outline type="rss" text="A Feed" title="A Feed" xmlUrl="https://a.example/feed.xml" htmlUrl="https://a.example/"/>
<outline type="rss" text="https://b.example/rss?x=1&amp;y=2" title="https://b.example/rss?x=1&amp;y=2" xmlUrl="https://b.example/rss?x=1&amp;y=2"/>
</outline>
<outline text="News &amp; Views" title="News &amp; Views">
<outline type="rss" text="C &amp; D" title="C &amp; D" xmlUrl="https://c.example/atom" htmlUrl="https://c.example/?a=1&amp;b=2"/>
<outline type="rss" text="日本語" title="日本語" xmlUrl="https://e.example/日本語"/>
</outline>
<outline text="Q&quot;uote&apos;s &lt;b&gt;" title="Q&quot;uote&apos;s &lt;b&gt;">
<outline type="rss" text="It&apos;s &quot;quoted&quot; &lt;tag&gt;" title="It&apos;s &quot;quoted&quot; &lt;tag&gt;" xmlUrl="https://d.example/&quot;q&quot;"/>
</outline>
</body>
</opml>`

const rustEmptyExportOutput = `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
<head>
<title>RDRS Subscriptions</title>
</head>
<body>
</body>
</opml>`

func TestExportMatchesRust(t *testing.T) {
	categories := []models.Category{
		{ID: 1, Name: "Tech"},
		{ID: 2, Name: "News &amp; Views"},
		{ID: 3, Name: "Empty"},
		{ID: 4, Name: `Q"uote's <b>`},
	}
	feeds := []models.Feed{
		{CategoryID: 1, SiteURL: ptr("https://a.example/"), Title: ptr("A Feed"), URL: "https://a.example/feed.xml"},
		{CategoryID: 1, URL: "https://b.example/rss?x=1&y=2"},
		{CategoryID: 2, SiteURL: ptr("https://c.example/?a=1&b=2"), Title: ptr("C &amp; D"), URL: "https://c.example/atom"},
		{CategoryID: 4, Title: ptr(`It's "quoted" <tag>`), URL: `https://d.example/"q"`},
		{CategoryID: 2, Title: ptr("日本語"), URL: "https://e.example/日本語"},
	}
	if got := Export(categories, feeds); got != rustExportOutput {
		t.Fatalf("export differs:\n%s\nwant:\n%s", got, rustExportOutput)
	}
	if got := Export(nil, nil); got != rustEmptyExportOutput {
		t.Fatalf("empty export = %q, want %q", got, rustEmptyExportOutput)
	}
}

func ptr[T any](v T) *T { return &v }

// wantErr is empty when the input must parse into outlines.
var rustParseVectors = []struct {
	input    string
	outlines []Outline
	wantErr  string
}{
	{
		input: `<opml version="2.0"><body><outline text="Cat"><outline type="rss" text="F" xmlUrl="https://x/1"/></outline></body></opml>`,
		outlines: []Outline{
			{CategoryName: "Cat", Feeds: []Feed{
				{Title: ptr("F"), XMLURL: "https://x/1"},
			}},
		},
	},
	{
		input: `<opml><body><outline type="rss" text="Top" xmlUrl="https://x/top"/></body></opml>`,
		outlines: []Outline{
			{CategoryName: "Uncategorized", Feeds: []Feed{
				{Title: ptr("Top"), XMLURL: "https://x/top"},
			}},
		},
	},
	{
		input: `<opml><body><outline text="A"><outline text="B"><outline type="rss" text="F" xmlUrl="https://x/n" htmlUrl="https://x/"/></outline></outline></body></opml>`,
		outlines: []Outline{
			{CategoryName: "B", Feeds: []Feed{
				{Title: ptr("F"), XMLURL: "https://x/n", HTMLURL: ptr("https://x/")},
			}},
		},
	},
	{
		input: `<opml><body><outline title="OnlyTitle"><outline xmlUrl="https://x/notype"/></outline></body></opml>`,
		outlines: []Outline{
			{CategoryName: "OnlyTitle", Feeds: []Feed{
				{XMLURL: "https://x/notype"},
			}},
		},
	},
	{
		input: `<opml><body><outline text="Dup"><outline xmlUrl="https://x/1"/><outline xmlUrl="https://x/1"/></outline><outline text="Dup"><outline xmlUrl="https://x/2"/></outline></body></opml>`,
		outlines: []Outline{
			{CategoryName: "Dup", Feeds: []Feed{
				{XMLURL: "https://x/1"},
				{XMLURL: "https://x/1"},
			}},
			{CategoryName: "Dup", Feeds: []Feed{
				{XMLURL: "https://x/2"},
			}},
		},
	},
	{
		input: `<opml><body><outline text="E &amp;amp; F"><outline text="T &amp;amp; U" xmlUrl="https://x/?a=1&amp;amp;b=2"/></outline></body></opml>`,
		outlines: []Outline{
			{CategoryName: "E &amp; F", Feeds: []Feed{
				{Title: ptr("T &amp; U"), XMLURL: "https://x/?a=1&amp;b=2"},
			}},
		},
	},
	{
		input:   "<opml><body></body></opml>",
		wantErr: "Invalid OPML format: No feeds found in OPML",
	},
	{
		input:   "not xml at all",
		wantErr: "Invalid OPML format: No feeds found in OPML",
	},
	{
		input:   "",
		wantErr: "Invalid OPML format: No feeds found in OPML",
	},
}

func TestParseMatchesRust(t *testing.T) {
	for _, c := range rustParseVectors {
		got, err := Parse(c.input)
		if c.wantErr != "" {
			if err == nil || err.Error() != c.wantErr {
				t.Errorf("%q: err = %v, want %q", c.input, err, c.wantErr)
			}
			continue
		}
		if err != nil {
			t.Errorf("%q: %v", c.input, err)
			continue
		}
		if !reflect.DeepEqual(got, c.outlines) {
			t.Errorf("%q:\n got %+v\nwant %+v", c.input, got, c.outlines)
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
