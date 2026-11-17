package feedparse

import (
	"bytes"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/henry40408/rdrs/internal/sanitize"
)

// namespace is one of the XML namespaces the parsers recognise. Everything
// else is nsUnknown, so foreign elements are never mistaken for feed fields.
type namespace int

const (
	nsUnknown namespace = iota
	nsAtom
	nsRSS
	nsContent
	nsDublinCore
	nsMediaRSS
	nsItunes
	nsPodcast
	nsCommentAPI
)

var namespaceByURI = map[string]namespace{
	"http://purl.org/rss/1.0/":                   nsRSS,
	"http://www.w3.org/2005/Atom":                nsAtom,
	"http://purl.org/rss/1.0/modules/content/":   nsContent,
	"http://purl.org/dc/elements/1.1/":           nsDublinCore,
	"http://search.yahoo.com/mrss/":              nsMediaRSS,
	"http://www.itunes.com/dtds/podcast-1.0.dtd": nsItunes,
	"https://podcastindex.org/namespace/1.0":     nsPodcast,
	"http://wellformedweb.org/CommentAPI/":       nsCommentAPI,
}

type attr struct{ name, value string }

// node is a text run or a child element.
type node struct {
	text    string
	element *element
}

// element is the parsed form of one XML element. Like the Rust parser, only
// the text that directly follows the start tag counts as the element's text.
type element struct {
	name string
	// declared is false when the document gave the element no namespace, so
	// the feed's default applies.
	declared bool
	space    namespace
	attrs    []attr
	nodes    []node
	base     string
	offset   int64
}

func (e *element) attr(name string) (string, bool) {
	for _, a := range e.attrs {
		if a.name == name {
			return a.value, true
		}
	}
	return "", false
}

// text is the character data right after the start tag, if any.
func (e *element) text() (string, bool) {
	if len(e.nodes) == 0 || e.nodes[0].element != nil {
		return "", false
	}
	return e.nodes[0].text, true
}

func (e *element) children() []*element {
	var out []*element
	for _, n := range e.nodes {
		if n.element != nil {
			out = append(out, n.element)
		}
	}
	return out
}

// textOrMarkup is the text, or the serialised children of an element that
// holds markup instead; some feeds put HTML straight into a text element.
func (e *element) textOrMarkup() string {
	if s, ok := e.text(); ok {
		return s
	}
	var b strings.Builder
	for _, n := range e.nodes {
		writeNode(&b, n)
	}
	return b.String()
}

func writeNode(b *strings.Builder, n node) {
	if n.element == nil {
		b.WriteString(minimalEscape(n.text))
		return
	}
	writeStart(b, n.element)
	for _, c := range n.element.nodes {
		writeNode(b, c)
	}
	b.WriteString("</" + n.element.name + ">")
}

// writeStart does not escape attribute values: the Rust parser did not.
func writeStart(b *strings.Builder, e *element) {
	b.WriteString("<" + e.name)
	for _, a := range e.attrs {
		b.WriteString(" " + a.name + `="` + a.value + `"`)
	}
	b.WriteString(">")
}

func minimalEscape(s string) string {
	return strings.NewReplacer("<", "&lt;", "&", "&amp;", "\r", "&#13;").Replace(s)
}

// xhtml serialises the children and drops the single wrapping <div> that
// valid XHTML content carries.
func (e *element) xhtml() string {
	var b strings.Builder
	firstElement := true
	divOpen, divClose := -1, -1
	for _, n := range e.nodes {
		if n.element == nil {
			if strings.TrimLeft(n.text, " \t\n\r\f") != "" {
				firstElement = false
				divOpen, divClose = -1, -1
			}
			b.WriteString(minimalEscape(n.text))
			continue
		}
		if firstElement && n.element.name != "div" {
			firstElement = false
		} else if !firstElement {
			divOpen, divClose = -1, -1
		}
		writeStart(&b, n.element)
		if firstElement {
			firstElement = false
			divOpen = b.Len()
		}
		for _, c := range n.element.nodes {
			writeNode(&b, c)
		}
		closeStart := b.Len()
		b.WriteString("</" + n.element.name + ">")
		if divOpen >= 0 {
			divClose = closeStart
		}
	}
	out := b.String()
	if divOpen < 0 || divClose < 0 {
		return out
	}
	return out[divOpen:divClose]
}

var errNoRoot = errors.New("no root element")

// parseTree reads the document up to the end of its root element. Anything
// after the root is never looked at, as in the streaming Rust parser.
func parseTree(data []byte) (*element, error) {
	dec := xml.NewDecoder(bytes.NewReader(data))
	dec.Strict = false
	// The caller has already converted the text to UTF-8.
	dec.CharsetReader = func(_ string, input io.Reader) (io.Reader, error) { return input, nil }
	b := treeBuilder{dec: dec}
	for {
		tok, err := dec.RawToken()
		if err == io.EOF {
			if b.root == nil {
				return nil, errNoRoot
			}
			return nil, xmlError("unclosed tag")
		}
		if err != nil {
			return nil, xmlError(err.Error())
		}
		done, err := b.handle(tok)
		if err != nil {
			return nil, err
		}
		if done {
			return b.root, nil
		}
	}
}

func xmlError(detail string) error {
	return fmt.Errorf("unable to parse XML: Parser error: %s", detail)
}

type treeBuilder struct {
	dec   *xml.Decoder
	root  *element
	stack []*element
	// scopes holds the namespace declarations of each open element.
	scopes []map[string]string
}

func (b *treeBuilder) handle(tok xml.Token) (done bool, err error) {
	switch t := tok.(type) {
	case xml.StartElement:
		return false, b.start(t)
	case xml.EndElement:
		return b.end(t)
	case xml.CharData:
		b.text(string(t))
	}
	return false, nil
}

func (b *treeBuilder) text(s string) {
	if len(b.stack) == 0 || s == "" {
		return
	}
	parent := b.stack[len(b.stack)-1]
	if n := len(parent.nodes); n > 0 && parent.nodes[n-1].element == nil {
		parent.nodes[n-1].text += s
		return
	}
	parent.nodes = append(parent.nodes, node{text: s})
}

func (b *treeBuilder) start(t xml.StartElement) error {
	scope := map[string]string{}
	attrs := make([]attr, 0, len(t.Attr))
	for _, a := range t.Attr {
		name := a.Name.Local
		if a.Name.Space != "" {
			name = a.Name.Space + ":" + name
		}
		switch {
		case a.Name.Space == "" && a.Name.Local == "xmlns":
			scope[""] = a.Value
		case a.Name.Space == "xmlns":
			scope[a.Name.Local] = a.Value
		}
		attrs = append(attrs, attr{name: name, value: a.Value})
	}
	b.scopes = append(b.scopes, scope)
	el := &element{name: lastSegment(t.Name.Local), attrs: attrs, offset: b.dec.InputOffset()}
	el.space, el.declared = b.resolve(t.Name.Space)
	if len(b.stack) > 0 {
		parent := b.stack[len(b.stack)-1]
		parent.nodes = append(parent.nodes, node{element: el})
		el.base = parent.base
	}
	applyXMLBase(el)
	if b.root == nil {
		b.root = el
	}
	b.stack = append(b.stack, el)
	return nil
}

func (b *treeBuilder) end(t xml.EndElement) (bool, error) {
	if len(b.stack) == 0 {
		return false, xmlError("unexpected end tag")
	}
	open := b.stack[len(b.stack)-1]
	if open.name != lastSegment(t.Name.Local) {
		return false, xmlError("mismatched end tag " + t.Name.Local)
	}
	b.stack = b.stack[:len(b.stack)-1]
	b.scopes = b.scopes[:len(b.scopes)-1]
	return len(b.stack) == 0, nil
}

// resolve maps a prefix to a namespace. An unbound or undeclared prefix is
// not declared, so the feed's default namespace applies to the element.
func (b *treeBuilder) resolve(prefix string) (namespace, bool) {
	for i := len(b.scopes) - 1; i >= 0; i-- {
		if uri, ok := b.scopes[i][prefix]; ok {
			if uri == "" {
				return nsUnknown, false
			}
			return namespaceByURI[uri], true
		}
	}
	return nsUnknown, false
}

func lastSegment(name string) string {
	if i := strings.LastIndexByte(name, ':'); i >= 0 {
		return name[i+1:]
	}
	return name
}

// applyXMLBase updates the element's base URL from its xml:base attribute. A
// relative value with no base to resolve against is ignored.
func applyXMLBase(el *element) {
	value, ok := el.attr("xml:base")
	if !ok {
		return
	}
	if resolved, ok := sanitize.ResolveLink(value, el.base); ok {
		el.base = resolved
	}
}
