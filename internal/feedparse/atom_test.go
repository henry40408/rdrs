package feedparse

import (
	"strings"
	"testing"
)

const atomOpen = `<feed xmlns="http://www.w3.org/2005/Atom">`

func atomEntryWith(inner string) string { return atomOpen + `<entry>` + inner + `</entry></feed>` }

func TestAtomFeedLevelFields(t *testing.T) {
	feed := mustParse(t, atomOpen+`<id>urn:feed</id><title>T</title><subtitle type="text"> S </subtitle>
	<updated>2024-03-04T08:00:00Z</updated><link href="/home" rel="alternate"/><link rel="self"/>
	<icon>https://a.example/i.png</icon><logo>%%</logo></feed>`)
	if feed.ID != "urn:feed" || *feed.Description != "S" || feed.Updated == nil {
		t.Errorf("feed = %+v", feed)
	}
	if len(feed.Links) != 1 || *feed.Links[0].Rel != "alternate" {
		t.Errorf("a link without href must be skipped, got %+v", feed.Links)
	}
	if *feed.Icon != "https://a.example/i.png" {
		t.Errorf("icon = %q", *feed.Icon)
	}
}

func TestAtomEntryFields(t *testing.T) {
	feed := mustParse(t, atomEntryWith(`<id>e1</id><title>Hello</title><updated>2024-03-04T08:00:00Z</updated>
	<published>2024-03-03T08:00:00Z</published><summary>Sum</summary><link href="https://a.example/1"/>
	<link rel="nofollow"/><author><name>Ann</name><email>ann@a.example</email></author><author/>
	<content type="text/plain">inline body</content>`))
	entry := feed.Entries[0]
	if entry.ID != "e1" || *entry.Title != "Hello" || *entry.Summary != "Sum" {
		t.Errorf("entry = %+v", entry)
	}
	if entry.Updated == nil || entry.Published == nil || !entry.Published.Before(*entry.Updated) {
		t.Errorf("updated %v published %v", entry.Updated, entry.Published)
	}
	if len(entry.Links) != 1 || len(entry.Authors) != 1 || *entry.Authors[0].Email != "ann@a.example" {
		t.Errorf("links %+v authors %+v", entry.Links, entry.Authors)
	}
	if *entry.Content != "inline body" {
		t.Errorf("content = %q", *entry.Content)
	}
}

func TestAtomDocumentWithEntryRoot(t *testing.T) {
	feed := mustParse(t, `<entry xmlns="http://www.w3.org/2005/Atom"><id>solo</id><title>One</title></entry>`)
	if feed.Type != TypeAtom || len(feed.Entries) != 1 || feed.Entries[0].ID != "solo" {
		t.Errorf("feed = %+v", feed)
	}
}

func TestAtomPersonKeepsAnEmailOnly(t *testing.T) {
	feed := mustParse(t, atomEntryWith(`<author><email>only@a.example</email></author><author><email/></author>`))
	authors := feed.Entries[0].Authors
	if len(authors) != 1 || authors[0].Name != nil || *authors[0].Email != "only@a.example" {
		t.Errorf("authors = %+v", authors)
	}
}

func TestAtomContentRules(t *testing.T) {
	cases := map[string]struct {
		content string
		want    string // an error fragment, or "" for success
	}{
		"source with a media type":       {`<content type="image/png" src="https://a.example/x.png"/>`, ""},
		"source without a type":          {`<content src="https://a.example/x"/>`, ""},
		"source with a bad type":         {`<content type="nonsense" src="https://a.example/x"/>`, "unsupported content type nonsense"},
		"source with a body":             {`<content type="text/plain" src="https://a.example/x">body</content>`, "missing non-empty atom:content with src"},
		"inline xml type":                {`<content type="application/xml">x</content>`, "unsupported content type application/xml"},
		"inline invalid media type":      {`<content type="not a type">x</content>`, "unsupported content type"},
		"inline media type without body": {`<content type="application/json"></content>`, "missing content.inline"},
		"inline media type":              {`<content type="application/json">{}</content>`, ""},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			_, err := Parse([]byte(atomEntryWith(c.content)))
			if c.want == "" {
				if err != nil {
					t.Errorf("error = %v", err)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Errorf("error = %v, want %q", err, c.want)
			}
		})
	}
}

func TestAtomUnsupportedTypesFailEveryTextElement(t *testing.T) {
	for _, element := range []string{`<title type="x/y">t</title>`, `<summary type="x/y">t</summary>`, `<subtitle type="x/y">t</subtitle>`} {
		doc := atomOpen + `<entry>` + element + `</entry></feed>`
		if element[1:9] == "subtitle" {
			doc = atomOpen + element + `</feed>`
		}
		if _, err := Parse([]byte(doc)); err == nil {
			t.Errorf("%s: want an error", element)
		}
	}
}
