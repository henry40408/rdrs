package feedparse

import (
	"strings"
	"testing"
)

const jsonFeedHead = `{"version":"https://jsonfeed.org/version/1.1","title":"T",`

func TestJSONFeedItemDetails(t *testing.T) {
	feed := mustParse(t, jsonFeedHead+`"description":" D ","feed_url":"https://j.example/feed.json","author":{"name":"Feed Author"},
	"items":[{"id":"1","url":"https://j.example/1","external_url":"https://other.example/1","summary":"Sum",
	"content_html":"<p>h</p>","content_text":"plain","date_modified":"2024-03-04T08:00:00Z",
	"attachments":[{"url":"https://j.example/a.mp3","mime_type":"audio/mpeg"}]},
	{"id":"2","content_text":"text only","authors":[{"name":"Ann"},{"name":"Ann"},{}]}]}`)
	if *feed.Description != "D" || len(feed.Links) != 1 {
		t.Errorf("feed = %+v", feed)
	}
	first, second := feed.Entries[0], feed.Entries[1]
	if len(first.Links) != 3 || first.Links[2].Href != "https://j.example/a.mp3" {
		t.Errorf("links = %+v", first.Links)
	}
	if *first.Content != "<p>h</p>" || *first.Summary != "Sum" || first.Updated == nil {
		t.Errorf("first = %+v", first)
	}
	if *first.Author() != "Feed Author" {
		t.Errorf("an item without authors inherits the feed's, got %+v", first.Authors)
	}
	if *second.Content != "text only" || len(second.Authors) != 1 || *second.Authors[0].Name != "Ann" {
		t.Errorf("second = %+v", second)
	}
}

func TestJSONFeedPlainTextBecomesTheSummaryWhenHTMLIsPresent(t *testing.T) {
	feed := mustParse(t, jsonFeedHead+`"items":[{"id":"1","content_html":"<p>h</p>","content_text":" plain "}]}`)
	if got := feed.Entries[0]; *got.Content != "<p>h</p>" || *got.Summary != "plain" {
		t.Errorf("entry = %+v", got)
	}
}

func TestJSONFeedAttachmentNeedsURLAndMimeType(t *testing.T) {
	_, err := Parse([]byte(jsonFeedHead + `"items":[{"id":"1","attachments":[{"url":"https://j.example/a"}]}]}`))
	if err == nil || !strings.Contains(err.Error(), "missing field in attachment") {
		t.Errorf("error = %v", err)
	}
}
