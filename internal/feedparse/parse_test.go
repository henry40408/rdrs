package feedparse

import (
	"strings"
	"testing"

	"golang.org/x/text/encoding/simplifiedchinese"
)

func mustParse(t *testing.T, doc string) *Feed {
	t.Helper()
	feed, err := Parse([]byte(doc))
	if err != nil {
		t.Fatal(err)
	}
	return feed
}

func TestJSONFeed(t *testing.T) {
	feed := mustParse(t, `{"version":"https://jsonfeed.org/version/1.1","title":" T ","home_page_url":"https://j.example",
	"authors":[{"name":"Ann"}],"favicon":"https://j.example/f.ico","icon":"https://j.example/i.png",
	"items":[{"id":"1","url":"https://j.example/1","content_text":" plain ","date_published":"2024-03-04T08:00:00Z"},
	{"url":"https://j.example/2","title":"Two","content_html":"<p>x</p>","content_text":"alt","authors":[{"name":"Bob"}]}]}`)
	if feed.Type != TypeJSON || *feed.Title != "T" || feed.Links[0].Href != "https://j.example/" {
		t.Fatalf("feed = %+v", feed)
	}
	first, second := feed.Entries[0], feed.Entries[1]
	if *first.Body() != "plain" || *first.Author() != "Ann" || first.Published == nil {
		t.Errorf("first = %+v", first)
	}
	if *second.Body() != "<p>x</p>" || *second.Summary != "alt" || *second.Author() != "Bob" {
		t.Errorf("second = %+v", second)
	}
	if second.ID != GenerateID(second.Links, second.Title) {
		t.Errorf("generated id = %q", second.ID)
	}
}

func TestJSONFeedErrors(t *testing.T) {
	cases := map[string]string{
		`{"version":"https://jsonfeed.org/version/2","title":"t","items":[]}`: "unsupported version: https://jsonfeed.org/version/2",
		`{"title":"t","items":[]}`:                                 "missing field `version`",
		`{"version":"https://jsonfeed.org/version/1","title":"t"}`: "missing field `items`",
		`{broken`: "unable to parse JSON",
	}
	for doc, want := range cases {
		if _, err := Parse([]byte(doc)); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("Parse(%q) error = %v, want %q", doc, err, want)
		}
	}
}

func TestNotAFeed(t *testing.T) {
	for _, doc := range []string{"", "plain text", "<html><body/></html>", `<rss><channel/></rss>`, `<rss version="1.0"/>`} {
		if _, err := Parse([]byte(doc)); err != ErrNoFeed {
			t.Errorf("Parse(%q) error = %v", doc, err)
		}
	}
}

func TestMalformedXMLIsAnError(t *testing.T) {
	for _, doc := range []string{
		`<rss version="2.0"><channel><item></channel></rss>`,
		`<rss version="2.0"><channel>`,
	} {
		if _, err := Parse([]byte(doc)); err == nil || !strings.HasPrefix(err.Error(), "unable to parse XML") {
			t.Errorf("Parse(%q) error = %v", doc, err)
		}
	}
}

func TestTrailingGarbageAfterRootIsIgnored(t *testing.T) {
	feed := mustParse(t, `<rss version="2.0"><channel><title>T</title></channel></rss><<<garbage`)
	if *feed.Title != "T" {
		t.Errorf("title = %v", feed.Title)
	}
}

func TestAtomTextTypes(t *testing.T) {
	feed := mustParse(t, `<feed xmlns="http://www.w3.org/2005/Atom"><title type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml">Hi <b>there</b></div></title>
	<entry><id> keep spaces </id><title type="html">&lt;i&gt;x&lt;/i&gt;</title><content type="xhtml"><p>a</p><p>b</p></content><author><name>N</name></author></entry></feed>`)
	if *feed.Title != "Hi <b>there</b>" {
		t.Errorf("title = %q", *feed.Title)
	}
	entry := feed.Entries[0]
	if entry.ID != " keep spaces " || *entry.Title != "<i>x</i>" || *entry.Content != "<p>a</p><p>b</p>" || *entry.Author() != "N" {
		t.Errorf("entry = %+v", entry)
	}
}

func TestAtomUnknownTypeIsAnError(t *testing.T) {
	_, err := Parse([]byte(`<feed xmlns="http://www.w3.org/2005/Atom"><title type="weird">x</title></feed>`))
	if err == nil || !strings.Contains(err.Error(), "unsupported content type weird") {
		t.Errorf("error = %v", err)
	}
}

func TestUndeclaredPrefixUsesTheFeedNamespace(t *testing.T) {
	feed := mustParse(t, `<rss version="2.0"><channel><x:title>Prefixed</x:title><item><title>t</title></item></channel></rss>`)
	if feed.Title == nil || *feed.Title != "Prefixed" {
		t.Errorf("title = %v", feed.Title)
	}
}

func TestRelativeLinkResolvesAgainstXMLBase(t *testing.T) {
	feed := mustParse(t, `<feed xmlns="http://www.w3.org/2005/Atom" xml:base="https://b.example/dir/"><entry><link href="../x?y=1"/><id>1</id></entry></feed>`)
	if got := feed.Entries[0].Links[0].Href; got != "https://b.example/x?y=1" {
		t.Errorf("href = %q", got)
	}
}

func TestCommentsLinkNeverBecomesTheEntryLink(t *testing.T) {
	feed := mustParse(t, `<rss version="2.0"><channel><item><comments>https://c.example/p</comments><link>https://c.example/p</link><guid>g</guid></item></channel></rss>`)
	links := feed.Entries[0].Links
	if len(links) != 1 || links[0].Target != TargetNone {
		t.Errorf("links = %+v", links)
	}
}

func TestPersonSplitsAddressFromName(t *testing.T) {
	cases := map[string][2]string{
		"a@b.com (Name One)": {"(Name One)", "a@b.com"},
		"Name Two <a@b.com>": {"Name Two", "a@b.com"},
		"just a name":        {"just a name", ""},
		"a@b.com":            {"", "a@b.com"},
	}
	for raw, want := range cases {
		person := parsePerson(raw)
		name, email := "", ""
		if person.Name != nil {
			name = *person.Name
		}
		if person.Email != nil {
			email = *person.Email
		}
		if name != want[0] || email != want[1] {
			t.Errorf("parsePerson(%q) = %q, %q; want %q, %q", raw, name, email, want[0], want[1])
		}
	}
}

func TestDecodeUsesTheDeclaredCharset(t *testing.T) {
	gbk, err := simplifiedchinese.GBK.NewEncoder().String(`<?xml version="1.0" encoding="gbk"?><rss version="2.0"><channel><title>中文標題</title></channel></rss>`)
	if err != nil {
		t.Fatal(err)
	}
	for _, header := range []string{"", "text/xml; charset=gbk"} {
		decoded, err := Decode([]byte(gbk), header)
		if err != nil {
			t.Fatal(err)
		}
		if feed := mustParse(t, string(decoded)); *feed.Title != "中文標題" {
			t.Errorf("header %q: title = %q", header, *feed.Title)
		}
	}
}

func TestDecodeDropsTheByteOrderMark(t *testing.T) {
	decoded, err := Decode([]byte("\xef\xbb\xbf<rss/>"), "")
	if err != nil || string(decoded) != "<rss/>" {
		t.Errorf("decoded = %q, %v", decoded, err)
	}
}

func TestPublishedAtFallsBackToTheFeedTimestamp(t *testing.T) {
	feed := mustParse(t, `<rss version="2.0"><channel><lastBuildDate>Tue, 05 Mar 2024 10:20:30 GMT</lastBuildDate><item><guid>g</guid></item></channel></rss>`)
	at := feed.Entries[0].PublishedAt(feed.Timestamp())
	if at == nil || at.Format("2006-01-02T15:04:05Z") != "2024-03-05T10:20:30Z" {
		t.Errorf("at = %v", at)
	}
	if (&Entry{}).PublishedAt(nil) != nil {
		t.Error("expected nil without any date")
	}
}
