package opml

import (
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"strings"
)

// Feed is one subscription found in an OPML file.
type Feed struct {
	Title   *string
	XMLURL  string
	HTMLURL *string
}

// Outline is a category and the feeds filed under it.
type Outline struct {
	CategoryName string
	Feeds        []Feed
}

// ParseError is an OPML file that cannot be used; its text is shown to users.
type ParseError struct{ Detail string }

func (e *ParseError) Error() string { return "Invalid OPML format: " + e.Detail }

const uncategorized = "Uncategorized"

type attributes struct {
	text, title, xmlURL, htmlURL *string
}

func outlineAttributes(start xml.StartElement) attributes {
	var found attributes
	for _, attr := range start.Attr {
		if attr.Name.Space != "" {
			continue
		}
		value := attr.Value
		switch strings.ToLower(attr.Name.Local) {
		case "text":
			found.text = &value
		case "title":
			found.title = &value
		case "xmlurl":
			found.xmlURL = &value
		case "htmlurl":
			found.htmlURL = &value
		}
	}
	return found
}

func firstOf(a, b *string) *string {
	if a != nil {
		return a
	}
	return b
}

func (a attributes) feed() Feed {
	return Feed{Title: firstOf(a.title, a.text), XMLURL: *a.xmlURL, HTMLURL: a.htmlURL}
}

// parser folds the events of the document into outlines. Nested categories
// are flattened: the innermost open one names the feeds.
type parser struct {
	outlines []Outline
	category *string
	feeds    []Feed
	inBody   bool
	depth    int
}

// addFeed files the feed under the open category, or on its own under
// "Uncategorized".
func (p *parser) addFeed(feed Feed) {
	if p.category != nil {
		p.feeds = append(p.feeds, feed)
		return
	}
	p.outlines = append(p.outlines, Outline{CategoryName: uncategorized, Feeds: []Feed{feed}})
}

// flush closes the open category; one without feeds is dropped.
func (p *parser) flush() {
	category := p.category
	p.category = nil
	if category == nil || len(p.feeds) == 0 {
		return
	}
	p.outlines = append(p.outlines, Outline{CategoryName: *category, Feeds: p.feeds})
	p.feeds = nil
}

func (p *parser) start(element xml.StartElement) {
	if element.Name.Space == "" && element.Name.Local == "body" {
		p.inBody = true
		return
	}
	if !p.inBody || !isOutline(element.Name) {
		return
	}
	found := outlineAttributes(element)
	if found.xmlURL != nil {
		p.addFeed(found.feed())
		return
	}
	p.flush()
	p.category = firstOf(found.text, found.title)
	p.depth++
}

// empty is a self-closing element. Without xmlUrl it is an empty category,
// which is ignored.
func (p *parser) empty(element xml.StartElement) {
	if !p.inBody || !isOutline(element.Name) {
		return
	}
	if found := outlineAttributes(element); found.xmlURL != nil {
		p.addFeed(found.feed())
	}
}

func (p *parser) end(name xml.Name) {
	if name.Space == "" && name.Local == "body" {
		p.inBody = false
		return
	}
	if !isOutline(name) || p.depth == 0 {
		return
	}
	p.depth--
	if p.depth == 0 {
		p.flush()
	}
}

func isOutline(name xml.Name) bool {
	return name.Space == "" && name.Local == "outline"
}

// Parse reads the feeds out of an OPML document. Like the Rust reader it is
// forgiving: stray text, unknown entities and unclosed elements are accepted,
// but an end tag that does not match its start tag is an error.
func Parse(content string) ([]Outline, error) {
	p := &parser{}
	if err := p.read(content); err != nil {
		return nil, err
	}
	p.flush()
	if len(p.outlines) == 0 {
		return nil, &ParseError{Detail: "No feeds found in OPML"}
	}
	return p.outlines, nil
}

func (p *parser) read(content string) error {
	decoder := xml.NewDecoder(strings.NewReader(content))
	decoder.Strict = false
	// The declared encoding is ignored: the text is already a Go string.
	decoder.CharsetReader = func(_ string, input io.Reader) (io.Reader, error) { return input, nil }
	var open []xml.Name
	for {
		token, err := decoder.RawToken()
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return syntaxError(decoder, err)
		}
		switch t := token.(type) {
		case xml.StartElement:
			if selfClosing(content, decoder.InputOffset()) {
				p.empty(t)
				// RawToken reports <a/> as a start followed by its end.
				if _, err := decoder.RawToken(); err != nil {
					return syntaxError(decoder, err)
				}
				continue
			}
			open = append(open, t.Name)
			p.start(t)
		case xml.EndElement:
			if len(open) == 0 || open[len(open)-1] != t.Name {
				return syntaxError(decoder, fmt.Errorf("unmatched end tag </%s>", t.Name.Local))
			}
			open = open[:len(open)-1]
			p.end(t.Name)
		}
	}
}

// selfClosing says whether the start tag that ends at offset ended in "/>".
func selfClosing(content string, offset int64) bool {
	return offset >= 2 && content[offset-2:offset] == "/>"
}

func syntaxError(decoder *xml.Decoder, err error) error {
	return &ParseError{Detail: fmt.Sprintf("Error at position %d: %v", decoder.InputOffset(), err)}
}
