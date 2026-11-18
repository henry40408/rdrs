package feedparse

import (
	"strings"
	"testing"
	"time"
)

func TestIsTokenAcceptsTildeAndRejectsSpace(t *testing.T) {
	tests := []struct {
		in   string
		want bool
	}{
		{"a~b", true}, {"a!b", true}, {"a b", false}, {" ", false}, {"a\x7fb", false}, {"aéb", false},
	}
	for _, tc := range tests {
		if got := isToken(tc.in); got != tc.want {
			t.Errorf("isToken(%q) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

func TestRDFItemWithoutLinkIsDropped(t *testing.T) {
	feed := mustParse(t, `<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/">`+
		`<channel><title>T</title></channel><item><title>no link</title></item>`+
		`<item><title>linked</title><link>https://e.com/a</link></item></rdf:RDF>`)
	if len(feed.Entries) != 1 || feed.Entries[0].Title == nil || *feed.Entries[0].Title != "linked" {
		t.Fatalf("entries = %+v", feed.Entries)
	}
}

func TestPublishedAtPrefersPublishedOverUpdated(t *testing.T) {
	published := time.Date(2024, 1, 2, 3, 4, 5, 0, time.UTC)
	updated := published.Add(time.Hour)
	feedTime := published.Add(2 * time.Hour)
	tests := []struct {
		name string
		e    Entry
		want *time.Time
	}{
		{"published wins", Entry{Published: &published, Updated: &updated}, &published},
		{"published alone", Entry{Published: &published}, &published},
		{"updated beats the feed", Entry{Updated: &updated}, &updated},
		{"feed is the last resort", Entry{}, &feedTime},
	}
	for _, tc := range tests {
		got := tc.e.PublishedAt(&feedTime)
		if got == nil || !got.Equal(*tc.want) {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
	if (&Entry{}).PublishedAt(nil) != nil {
		t.Error("no dates at all must give nil")
	}
}

func TestXHTMLKeepsContentThatIsNotASingleDiv(t *testing.T) {
	tests := []struct{ name, inner, want string }{
		{"text only", `hello`, `hello`},
		{"single div is unwrapped", `<div>x</div>`, `x`},
		{"non div element", `<p>x</p>`, `<p>x</p>`},
		{"two divs", `<div>a</div><div>b</div>`, `<div>a</div><div>b</div>`},
		{"text before div", `t<div>a</div>`, `t<div>a</div>`},
		{"div then text", `<div>a</div>t`, `<div>a</div>t`},
		{"div then blank then div", `<div>a</div> <div>b</div>`, `<div>a</div> <div>b</div>`},
	}
	for _, tc := range tests {
		root, err := parseTree([]byte(`<r>` + tc.inner + `</r>`))
		if err != nil {
			t.Fatal(err)
		}
		if got := root.xhtml(); got != tc.want {
			t.Errorf("%s: xhtml = %q, want %q", tc.name, got, tc.want)
		}
	}
}

func TestAdjacentTextTokensMergeIntoOneNode(t *testing.T) {
	root, err := parseTree([]byte(`<r>a<![CDATA[b]]>c<x/>d</r>`))
	if err != nil {
		t.Fatal(err)
	}
	if len(root.nodes) != 3 || root.nodes[0].text != "abc" || root.nodes[1].element == nil || root.nodes[2].text != "d" {
		t.Errorf("nodes = %+v", root.nodes)
	}
}

func TestExactlyMaxElementsIsAccepted(t *testing.T) {
	doc := `<rss version="2.0"><channel><title>T</title>` + strings.Repeat("<x/>", maxElements-3) + `</channel></rss>`
	if _, err := Parse([]byte(doc)); err != nil {
		t.Fatalf("err = %v", err)
	}
}

func TestLastSegmentStripsTheLeadingPrefix(t *testing.T) {
	for in, want := range map[string]string{"a:b": "b", ":b": "b", "b": "b", "a:b:c": "c"} {
		if got := lastSegment(in); got != want {
			t.Errorf("lastSegment(%q) = %q, want %q", in, got, want)
		}
	}
}
