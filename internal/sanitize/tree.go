package sanitize

import "strings"

// node is the sanitiser's output tree. It is separate from x/net/html's so
// attribute values can be flagged verbatim: the post-pass writes URLs with a
// bare `&` (only `"` escaped), while values from the allowlist step are
// escaped like html5ever does.
type node struct {
	tag      string // empty for a text node
	text     string
	attrs    []attr
	children []*node
}

type attr struct {
	key, val string
	// verbatim values are written as is, except for `"`.
	verbatim bool
}

func (n *node) isText() bool { return n.tag == "" }

// voidTags are the allowed elements that never have an end tag.
var voidTags = map[string]bool{"br": true, "img": true}

// rawAttr returns the attribute as it appears in serialised markup, which is
// what the original post-pass saw.
func (n *node) rawAttr(key string) (string, bool) {
	for _, a := range n.attrs {
		if a.key != key {
			continue
		}
		if a.verbatim {
			return a.val, true
		}
		return escapeAttr(a.val), true
	}
	return "", false
}

// setVerbatim replaces the attribute in place, or appends it.
func (n *node) setVerbatim(key, val string) {
	for i := range n.attrs {
		if n.attrs[i].key == key {
			n.attrs[i] = attr{key: key, val: val, verbatim: true}
			return
		}
	}
	n.attrs = append(n.attrs, attr{key: key, val: val, verbatim: true})
}

// render serialises the way html5ever does: void elements without a slash,
// text escaping `& < >` and U+00A0, attribute escaping `& "` and U+00A0.
func render(nodes []*node) string {
	var b strings.Builder
	for _, n := range nodes {
		writeNode(&b, n)
	}
	return b.String()
}

func writeNode(b *strings.Builder, n *node) {
	if n.isText() {
		b.WriteString(escapeText(n.text))
		return
	}
	b.WriteByte('<')
	b.WriteString(n.tag)
	for _, a := range n.attrs {
		b.WriteByte(' ')
		b.WriteString(a.key)
		b.WriteString(`="`)
		if a.verbatim {
			b.WriteString(strings.ReplaceAll(a.val, `"`, "&quot;"))
		} else {
			b.WriteString(escapeAttr(a.val))
		}
		b.WriteByte('"')
	}
	b.WriteByte('>')
	if voidTags[n.tag] {
		return
	}
	for _, c := range n.children {
		writeNode(b, c)
	}
	b.WriteString("</")
	b.WriteString(n.tag)
	b.WriteByte('>')
}

var (
	textEscaper = strings.NewReplacer("&", "&amp;", " ", "&nbsp;", "<", "&lt;", ">", "&gt;")
	attrEscaper = strings.NewReplacer("&", "&amp;", " ", "&nbsp;", `"`, "&quot;")
)

func escapeText(s string) string { return textEscaper.Replace(s) }
func escapeAttr(s string) string { return attrEscaper.Replace(s) }
