package feedparse

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"testing"
	"time"
)

// vectorEntry and vectorFeed are what the Rust generator wrote for each
// fixture, so comparing them checks every field the sync reads.
type vectorEntry struct {
	GUID      string   `json:"guid"`
	Title     *string  `json:"title"`
	Link      *string  `json:"link"`
	Links     []string `json:"links"`
	Content   *string  `json:"content"`
	Summary   *string  `json:"summary"`
	Author    *string  `json:"author"`
	Published *string  `json:"published"`
	Updated   *string  `json:"updated"`
}

type vectorFeed struct {
	Error       string        `json:"error"`
	FeedType    string        `json:"feed_type"`
	ID          string        `json:"id"`
	Title       *string       `json:"title"`
	Description *string       `json:"description"`
	Updated     *string       `json:"updated"`
	Links       []string      `json:"links"`
	Icon        *string       `json:"icon"`
	Logo        *string       `json:"logo"`
	Entries     []vectorEntry `json:"entries"`
}

func formatTime(t *time.Time) *string {
	if t == nil {
		return nil
	}
	return ptr(t.UTC().Format(time.RFC3339Nano))
}

func hrefs(links []Link) []string {
	out := []string{}
	for _, l := range links {
		out = append(out, l.Href)
	}
	return out
}

func describe(feed *Feed) vectorFeed {
	got := vectorFeed{
		FeedType: string(feed.Type), ID: feed.ID, Title: feed.Title, Description: feed.Description,
		Updated: formatTime(feed.Updated), Links: hrefs(feed.Links), Icon: feed.Icon, Logo: feed.Logo,
		Entries: []vectorEntry{},
	}
	for i := range feed.Entries {
		e := &feed.Entries[i]
		got.Entries = append(got.Entries, vectorEntry{
			GUID: e.ID, Title: e.Title, Link: e.Link(), Links: hrefs(e.Links), Content: e.Body(),
			Summary: e.Summary, Author: e.Author(), Published: formatTime(e.Published), Updated: formatTime(e.Updated),
		})
	}
	return got
}

func TestRustFeedVectors(t *testing.T) {
	files, err := filepath.Glob("../../testdata/vectors/feeds/*.xml")
	if err != nil || len(files) < 17 {
		t.Fatalf("fixtures: %v, %d files", err, len(files))
	}
	for _, file := range files {
		t.Run(filepath.Base(file), func(t *testing.T) {
			body, err := os.ReadFile(file)
			if err != nil {
				t.Fatal(err)
			}
			raw, err := os.ReadFile(strings.TrimSuffix(file, ".xml") + ".json")
			if err != nil {
				t.Fatal(err)
			}
			var want vectorFeed
			if err := json.Unmarshal(raw, &want); err != nil {
				t.Fatal(err)
			}
			feed, err := Parse(body)
			if want.Error != "" {
				if err == nil || err.Error() != want.Error {
					t.Fatalf("error = %v, want %q", err, want.Error)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			got := maskRandomIDs(describe(feed))
			want = maskRandomIDs(want)
			if !reflect.DeepEqual(got, want) {
				t.Errorf("first difference: %s", firstDifference(t, got, want))
			}
		})
	}
}

func firstDifference(t *testing.T, got, want vectorFeed) string {
	t.Helper()
	gotLines := strings.Split(marshal(t, got), "\n")
	wantLines := strings.Split(marshal(t, want), "\n")
	for i := range min(len(gotLines), len(wantLines)) {
		if gotLines[i] != wantLines[i] {
			return "line " + strings.TrimSpace(gotLines[i]) + " want " + strings.TrimSpace(wantLines[i]) +
				" (context: " + strings.TrimSpace(gotLines[max(i-3, 0)]) + ")"
		}
	}
	return "lengths differ"
}

func marshal(t *testing.T, v vectorFeed) string {
	t.Helper()
	out, err := json.MarshalIndent(v, "", " ")
	if err != nil {
		t.Fatal(err)
	}
	return string(out)
}

var randomID = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)

// maskRandomIDs hides the UUIDs of entries with neither guid nor link, which
// differ on every parse.
func maskRandomIDs(f vectorFeed) vectorFeed {
	mask := func(id string) string {
		if randomID.MatchString(id) {
			return "<uuid>"
		}
		return id
	}
	f.ID = mask(f.ID)
	entries := make([]vectorEntry, len(f.Entries))
	for i, e := range f.Entries {
		e.GUID = mask(e.GUID)
		entries[i] = e
	}
	f.Entries = entries
	return f
}
