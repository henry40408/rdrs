package sanitize

import "golang.org/x/net/html"

// policy is an allowlist in the shape ammonia 4 uses: unknown elements are
// unwrapped (children kept), script and style are dropped with their content,
// attributes are kept only when allowed, and comments are removed.
type policy struct {
	tags    map[string]bool
	linkRel string // added to every <a> when non-empty
}

// dropWithContent are removed together with everything inside them.
var dropWithContent = map[string]bool{"script": true, "style": true}

// genericAttrs are allowed on every kept element.
var genericAttrs = map[string]bool{"lang": true, "title": true}

// tagAttrs are ammonia's defaults for the tags this package can allow.
var tagAttrs = map[string]map[string]bool{
	"a":          {"href": true, "hreflang": true},
	"blockquote": {"cite": true},
	"img":        {"align": true, "alt": true, "height": true, "src": true, "width": true},
	"ol":         {"start": true},
	"table":      {"align": true, "char": true, "charoff": true, "summary": true},
	"tbody":      {"align": true, "char": true, "charoff": true},
	"thead":      {"align": true, "char": true, "charoff": true},
	"tr":         {"align": true, "char": true, "charoff": true},
	"td":         {"align": true, "char": true, "charoff": true, "colspan": true, "headers": true, "rowspan": true},
	"th":         {"align": true, "char": true, "charoff": true, "colspan": true, "headers": true, "rowspan": true, "scope": true},
}

func tagSet(names ...string) map[string]bool {
	set := make(map[string]bool, len(names))
	for _, n := range names {
		set[n] = true
	}
	return set
}

var articlePolicy = policy{
	tags: tagSet("p", "br", "a", "strong", "em", "b", "i", "ul", "ol", "li",
		"blockquote", "pre", "code", "img", "h1", "h2", "h3", "h4", "h5", "h6",
		"div", "span", "figure", "figcaption", "table", "thead", "tbody", "tr", "th", "td"),
	linkRel: "noopener noreferrer",
}

// summaryPolicy allows inline prose only: no images, tables, headings or
// links. A link in a box the UI presents as trustworthy is a phishing
// primitive.
var summaryPolicy = policy{
	tags:    tagSet("p", "br", "strong", "em", "b", "i", "ul", "ol", "li", "code"),
	linkRel: "noopener noreferrer",
}

// clean converts the children of root, an HTML container.
func (p policy) clean(root *html.Node) []*node {
	return p.cleanChildren(root, root)
}

// cleanChildren converts parent's children. kept is the nearest ancestor that
// survives in the output; it decides whether a namespace switch is legal.
func (p policy) cleanChildren(parent, kept *html.Node) []*node {
	var out []*node
	for c := parent.FirstChild; c != nil; c = c.NextSibling {
		switch c.Type {
		case html.TextNode:
			out = append(out, &node{text: c.Data})
		case html.ElementNode:
			out = append(out, p.cleanElement(c, kept)...)
		}
	}
	return out
}

func (p policy) cleanElement(e, kept *html.Node) []*node {
	if dropWithContent[e.Data] || !expectedNamespace(kept, e) {
		return nil
	}
	if !p.tags[e.Data] {
		return p.cleanChildren(e, kept)
	}
	return []*node{{tag: e.Data, attrs: p.cleanAttrs(e), children: p.cleanChildren(e, e)}}
}

func (p policy) cleanAttrs(e *html.Node) []attr {
	var attrs []attr
	seen := map[string]bool{}
	for _, a := range e.Attr {
		// html5ever keeps the first of a duplicated attribute.
		if a.Namespace != "" || seen[a.Key] {
			continue
		}
		seen[a.Key] = true
		if attrAllowed(e.Data, a) {
			attrs = append(attrs, attr{key: a.Key, val: a.Val})
		}
	}
	if e.Data == "a" && p.linkRel != "" {
		attrs = append(attrs, attr{key: "rel", val: p.linkRel})
	}
	return attrs
}

func attrAllowed(tag string, a html.Attribute) bool {
	if !genericAttrs[a.Key] && !tagAttrs[tag][a.Key] {
		return false
	}
	if a.Key == "href" || a.Key == "src" {
		return urlAttrAllowed(a.Val)
	}
	return true
}
