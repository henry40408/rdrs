package sanitize

import (
	"log/slog"
	"strings"

	"golang.org/x/net/html"
	"golang.org/x/net/html/atom"
)

// parseFragment parses as the children of a <div>, like ammonia, and returns
// a container holding them.
func parseFragment(content string) *html.Node {
	context := &html.Node{Type: html.ElementNode, DataAtom: atom.Div, Data: "div"}
	nodes, err := html.ParseFragment(strings.NewReader(content), context)
	root := &html.Node{Type: html.ElementNode, DataAtom: atom.Div, Data: "div"}
	if err != nil {
		// x/net/html refuses more than 511 open elements; ammonia has no such
		// limit. Keep the content as escaped text rather than lose it.
		slog.Warn("html nesting too deep, keeping content as text", "event", "sanitize.too_deep", "error", err)
		root.AppendChild(&html.Node{Type: html.TextNode, Data: content})
		return root
	}
	for _, n := range nodes {
		root.AppendChild(n)
	}
	return root
}

// eachElement calls fn for every element called name below parent.
func eachElement(parent *html.Node, name string, fn func(*html.Node)) {
	for c := parent.FirstChild; c != nil; c = c.NextSibling {
		if c.Type != html.ElementNode {
			continue
		}
		if c.Data == name {
			fn(c)
		}
		eachElement(c, name, fn)
	}
}

func lookupAttr(n *html.Node, key string) (string, bool) {
	for _, a := range n.Attr {
		if a.Namespace == "" && a.Key == key {
			return a.Val, true
		}
	}
	return "", false
}

func attrValue(n *html.Node, key string) string {
	v, _ := lookupAttr(n, key)
	return v
}

// setAttr replaces the attribute in place, or appends it.
func setAttr(n *html.Node, key, val string) {
	for i := range n.Attr {
		if n.Attr[i].Namespace == "" && n.Attr[i].Key == key {
			n.Attr[i].Val = val
			return
		}
	}
	n.Attr = append(n.Attr, html.Attribute{Key: key, Val: val})
}

// renameAttr keeps the attribute's position.
func renameAttr(n *html.Node, from, to string) {
	for i := range n.Attr {
		if n.Attr[i].Namespace == "" && n.Attr[i].Key == from {
			n.Attr[i].Key = to
			return
		}
	}
}
