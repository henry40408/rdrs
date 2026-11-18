package feedparse

import (
	"bytes"
	"errors"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/datetime"
	"github.com/henry40408/rdrs/internal/sanitize"
)

// ErrNoFeed is returned when the document is no feed at all, for example an
// HTML page.
var ErrNoFeed = errors.New("unable to parse feed: no root element")

// sniffLimit is how much of the start of the document is searched for the
// character that tells XML from JSON.
const sniffLimit = 8192

// Parse reads a feed from text that is already UTF-8 (see Decode). Entries
// and the feed itself get an id when the document gave none.
func Parse(data []byte) (*Feed, error) {
	head := data[:min(len(data), sniffLimit)]
	i := bytes.IndexAny(head, "<{")
	if i < 0 {
		return nil, ErrNoFeed
	}
	var feed *Feed
	var err error
	if head[i] == '<' {
		feed, err = parseXML(data)
	} else {
		feed, err = parseJSON(data)
	}
	if err != nil {
		return nil, err
	}
	assignMissingIDs(feed)
	return feed, nil
}

// parser carries what element handlers share.
type parser struct {
	defaultNS namespace
}

// tag is the element's namespace, with the feed default for an element the
// document left unqualified, and its local name.
func (p *parser) tag(e *element) (namespace, string) {
	if !e.declared {
		return p.defaultNS, e.name
	}
	return e.space, e.name
}

func parseXML(data []byte) (*Feed, error) {
	root, err := parseTree(data)
	if errors.Is(err, errNoRoot) {
		return nil, ErrNoFeed
	}
	if err != nil {
		return nil, err
	}
	version, hasVersion := root.attr("version")
	switch {
	case root.name == "feed":
		return (&parser{defaultNS: nsAtom}).atomFeed(root)
	case root.name == "entry":
		return (&parser{defaultNS: nsAtom}).atomEntryDocument(root)
	case root.name == "rss" && hasVersion && (version == "2.0" || version == ""):
		return (&parser{defaultNS: nsRSS}).rss(root, TypeRSS2)
	case root.name == "rss" && hasVersion && (version == "0.91" || version == "0.92"):
		return (&parser{defaultNS: nsRSS}).rss(root, TypeRSS0)
	case root.name == "RDF":
		return (&parser{defaultNS: nsRSS}).rdf(root)
	}
	return nil, ErrNoFeed
}

func newText(s string) string { return strings.TrimSpace(s) }

func ptr[T any](v T) *T { return &v }

// timestamp parses with the app's loose parser; an unreadable value is no
// value, so one bad date does not discard the entry.
func timestamp(s string) *time.Time {
	t, ok := datetime.ParseTimestamp(s)
	if !ok {
		return nil
	}
	return &t
}

// timestampOf is timestamp for an element's own text.
func timestampOf(e *element) *time.Time {
	text, ok := e.text()
	if !ok {
		return nil
	}
	return timestamp(text)
}

// newLink resolves href against the element's xml:base the way the url crate
// does, falling back to the text as written.
func newLink(href, base string) Link {
	if resolved, ok := sanitize.ResolveLink(href, base); ok {
		return Link{Href: strings.TrimSpace(resolved)}
	}
	return Link{Href: strings.TrimSpace(href)}
}

// linkOf handles <link> whose text is the address.
func linkOf(e *element, target Target) (Link, bool) {
	text, ok := e.text()
	if !ok {
		return Link{}, false
	}
	link := newLink(text, e.base)
	link.Target = target
	return link, true
}

// titleOf handles <title>, <description> and the like in RSS: trimmed text.
func titleOf(e *element) *string {
	text, ok := e.text()
	if !ok {
		return nil
	}
	return ptr(newText(text))
}
