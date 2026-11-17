package feedparse

import (
	"fmt"
	"strings"

	"github.com/henry40408/rdrs/internal/sanitize"
)

func (p *parser) atomFeed(root *element) (*Feed, error) {
	feed := &Feed{Type: TypeAtom}
	for _, child := range root.children() {
		ns, name := p.tag(child)
		if ns != nsAtom {
			continue
		}
		var err error
		switch name {
		case "id":
			if text, ok := child.text(); ok {
				feed.ID = text
			}
		case "title":
			feed.Title, err = atomText(child)
		case "updated":
			feed.Updated = timestampOf(child)
		case "link":
			if link, ok := atomLink(child); ok {
				feed.Links = append(feed.Links, link)
			}
		case "icon":
			feed.Icon = atomImage(child)
		case "logo":
			feed.Logo = atomImage(child)
		case "subtitle":
			feed.Description, err = atomText(child)
		case "entry":
			var entry Entry
			if entry, err = p.atomEntry(child); err == nil {
				feed.Entries = append(feed.Entries, entry)
			}
		}
		if err != nil {
			return nil, err
		}
	}
	return feed, nil
}

// atomEntryDocument is a document whose root is a single <entry>.
func (p *parser) atomEntryDocument(root *element) (*Feed, error) {
	entry, err := p.atomEntry(root)
	if err != nil {
		return nil, err
	}
	return &Feed{Type: TypeAtom, Entries: []Entry{entry}}, nil
}

func (p *parser) atomEntry(el *element) (Entry, error) {
	var entry Entry
	for _, child := range el.children() {
		ns, name := p.tag(child)
		var err error
		switch {
		case ns == nsAtom && name == "id":
			if text, ok := child.text(); ok {
				entry.ID = text
			}
		case ns == nsAtom && name == "title":
			entry.Title, err = atomText(child)
		case ns == nsAtom && name == "updated":
			entry.Updated = timestampOf(child)
		case ns == nsAtom && name == "author":
			if person, ok := atomPerson(child); ok {
				entry.Authors = append(entry.Authors, person)
			}
		case ns == nsAtom && name == "content":
			entry.Content, err = atomContent(child)
		case ns == nsAtom && name == "link":
			if link, ok := atomLink(child); ok {
				entry.Links = append(entry.Links, link)
			}
		case ns == nsAtom && name == "summary":
			entry.Summary, err = atomText(child)
		case ns == nsAtom && (name == "published" || name == "pubDate"):
			entry.Published = timestampOf(child)
		case ns == nsCommentAPI && (name == "commentRss" || name == "commentRSS"):
			if link, ok := linkOf(child, TargetCommentsFeed); ok {
				entry.Links = append(entry.Links, link)
			}
		}
		if err != nil {
			return Entry{}, err
		}
	}
	return entry, nil
}

// atomLink needs an href.
func atomLink(e *element) (Link, bool) {
	href, ok := e.attr("href")
	if !ok {
		return Link{}, false
	}
	return newLink(href, e.base), true
}

// atomImage is <icon> or <logo>; the address is kept as written when it
// cannot be resolved.
func atomImage(e *element) *string {
	raw, ok := e.text()
	if !ok {
		return nil
	}
	if resolved, ok := sanitize.ResolveLink(raw, e.base); ok {
		return &resolved
	}
	return &raw
}

// atomPerson needs a name or an email.
func atomPerson(e *element) (Person, bool) {
	var person Person
	for _, child := range e.children() {
		text, hasText := child.text()
		switch child.name {
		case "name":
			if hasText {
				person.Name = &text
			}
		case "email":
			if hasText {
				person.Email = &text
			} else {
				person.Email = nil
			}
		}
	}
	return person, person.Name != nil || person.Email != nil
}

func unsupportedType(mime string, at int64) error {
	return fmt.Errorf("unable to parse feed: unsupported content type %s at position %d", mime, at)
}

func missingContent(what string, at int64) error {
	return fmt.Errorf("unable to parse feed: missing %s at position %d", what, at)
}

// atomText is <title>, <summary>, <subtitle> and <rights>: text, HTML or
// XHTML, trimmed. Any other type is an error, as it was in the Rust parser.
func atomText(e *element) (*string, error) {
	kind, _ := e.attr("type")
	switch kind {
	case "", "text", "html", "text/html":
		return ptr(newText(e.textOrMarkup())), nil
	case "xhtml":
		return ptr(newText(e.xhtml())), nil
	}
	return nil, unsupportedType(kind, e.offset)
}

// atomContent is <content>. Only the body matters to the sync; a src
// reference has none.
func atomContent(e *element) (*string, error) {
	kind, hasKind := e.attr("type")
	if _, hasSrc := e.attr("src"); hasSrc {
		return nil, checkContentSource(e, kind, hasKind)
	}
	switch kind {
	case "", "text", "html", "xhtml", "text/html":
		return atomText(e)
	}
	if strings.HasSuffix(kind, " +xml") || strings.HasSuffix(kind, "/xml") {
		// The Rust parser sent these through the text rules, which refuse
		// every type that is not text or HTML.
		return nil, unsupportedType(kind, e.offset)
	}
	if !validMediaType(kind) {
		return nil, unsupportedType(kind, e.offset)
	}
	body, ok := e.text()
	if !ok {
		return nil, missingContent("content.inline", e.offset)
	}
	return &body, nil
}

// checkContentSource validates a content element that points elsewhere with
// src: its type must be a media type and it must be empty.
func checkContentSource(e *element, kind string, hasKind bool) error {
	if hasKind && !validMediaType(kind) {
		return unsupportedType(kind, e.offset)
	}
	if _, ok := e.text(); ok {
		return missingContent("non-empty atom:content with src", e.offset)
	}
	return nil
}

// validMediaType accepts "type/subtype" with optional parameters.
func validMediaType(s string) bool {
	main, _, _ := strings.Cut(s, ";")
	kind, subtype, ok := strings.Cut(strings.TrimSpace(main), "/")
	return ok && isToken(kind) && isToken(subtype)
}

func isToken(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		if r > 0x7e || r <= 0x20 || strings.ContainsRune(`()<>@,;:\"/[]?={}`, r) {
			return false
		}
	}
	return true
}
