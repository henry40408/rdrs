package sanitize

import (
	"strconv"
	"strings"

	"golang.org/x/net/html"
)

// The pre-passes read attributes the allowlist is about to strip. Each one is
// fronted by a cheap substring gate so documents that cannot be affected skip
// the work.

// lazySrcAttrs carry the real image URL of lazy-loaded images, by priority.
var lazySrcAttrs = []string{"data-src", "data-lazy-src", "data-original"}

// containsFold is an ASCII case-insensitive substring test, since HTML names
// are case-insensitive. needle must be lowercase.
func containsFold(haystack, needle string) bool {
	if needle == "" {
		return true
	}
	for i := 0; i+len(needle) <= len(haystack); i++ {
		if asciiFold(haystack[i:i+len(needle)], needle) {
			return true
		}
	}
	return false
}

// asciiFold compares ASCII-case-insensitively; unlike strings.EqualFold it
// does not fold Unicode (the Kelvin sign would match "k").
func asciiFold(s, lowerNeedle string) bool {
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' {
			c += 'a' - 'A'
		}
		if c != lowerNeedle[i] {
			return false
		}
	}
	return true
}

func ariaHiddenGate(content string) bool { return containsFold(content, "aria-hidden") }

func lazyGate(content string) bool {
	for _, a := range lazySrcAttrs {
		if containsFold(content, a) {
			return true
		}
	}
	return false
}

// imageGate matches <image> too, which the parser reads as <img>.
func imageGate(content string) bool {
	return containsFold(content, "<img") || containsFold(content, "<image")
}

func dimensionGate(content string) bool {
	return imageGate(content) &&
		(containsFold(content, "style") || containsFold(content, "data-original-"))
}

// emojiSrcPrefix is where WordPress serves the pictures it puts in feeds in
// place of emoji characters.
const emojiSrcPrefix = "https://s.w.org/images/core/emoji/"

// prepare parses content once and applies the pre-passes in order:
// aria-hidden, lazy images, emoji images, image dimensions.
func prepare(content string) *html.Node {
	root := parseFragment(content)
	if ariaHiddenGate(content) {
		root = dropAriaHidden(root, content)
	}
	if lazyGate(content) {
		eachElement(root, "img", promoteLazyImage)
	}
	if imageGate(content) {
		eachElement(root, "img", restoreEmoji)
	}
	if dimensionGate(content) {
		eachElement(root, "img", harvestDimensions)
	}
	return root
}

// dropAriaHidden removes aria-hidden="true" subtrees. A document that would
// be left empty is returned untouched, otherwise a wholly hidden article
// would render blank.
func dropAriaHidden(root *html.Node, content string) *html.Node {
	pruned := parseFragment(content)
	removeAriaHidden(pruned)
	if strings.TrimSpace(content) != "" && !hasContent(pruned) {
		return root
	}
	return pruned
}

func removeAriaHidden(parent *html.Node) {
	for c := parent.FirstChild; c != nil; {
		next := c.NextSibling
		if c.Type == html.ElementNode && strings.EqualFold(attrValue(c, "aria-hidden"), "true") {
			parent.RemoveChild(c)
		} else {
			removeAriaHidden(c)
		}
		c = next
	}
}

// hasContent reports whether anything but whitespace text would be written.
func hasContent(parent *html.Node) bool {
	for c := parent.FirstChild; c != nil; c = c.NextSibling {
		if c.Type == html.TextNode && strings.TrimSpace(c.Data) == "" {
			continue
		}
		return true
	}
	return false
}

// promoteLazyImage puts the first usable lazy URL into src when src is
// missing or a data: placeholder.
func promoteLazyImage(img *html.Node) {
	current, hasSrc := lookupAttr(img, "src")
	if hasSrc && !strings.HasPrefix(current, "data:") {
		return
	}
	for _, name := range lazySrcAttrs {
		real, ok := lookupAttr(img, name)
		if !ok || real == "" || strings.HasPrefix(real, "data:") {
			continue
		}
		if hasSrc {
			setAttr(img, "src", real)
			return
		}
		renameAttr(img, name, "src")
		return
	}
}

// restoreEmoji turns an emoji image back into its character. The picture is
// sized by an inline style the allowlist strips, so it would otherwise render
// as a full-size block image.
func restoreEmoji(img *html.Node) {
	alt := attrValue(img, "alt")
	if alt == "" || !strings.HasPrefix(attrValue(img, "src"), emojiSrcPrefix) {
		return
	}
	// A span holding only the picture spaced it from the text, as in a list
	// that uses emoji for bullets. Its style is stripped too.
	if img.Parent.Data == "span" && img.PrevSibling == nil && img.NextSibling == nil {
		alt += " "
	}
	img.Type, img.Data, img.DataAtom, img.Attr = html.TextNode, alt, 0, nil
}

// harvestDimensions gives an img that has neither width nor height a positive
// integer pair taken from data-original-* or the inline style.
func harvestDimensions(img *html.Node) {
	if _, ok := lookupAttr(img, "width"); ok {
		return
	}
	if _, ok := lookupAttr(img, "height"); ok {
		return
	}
	style := attrValue(img, "style")
	width := dimensionHint(img, "data-original-width", style, "width")
	height := dimensionHint(img, "data-original-height", style, "height")
	// A harvested 0 would collapse the box and suppress the 16/9 fallback.
	if !isPositive(width) || !isPositive(height) {
		return
	}
	setAttr(img, "width", width)
	setAttr(img, "height", height)
}

func dimensionHint(img *html.Node, dataAttr, style, property string) string {
	if v, ok := lookupAttr(img, dataAttr); ok && v != "" && allDigits(v) {
		return v
	}
	return styleDimension(style, property)
}

func allDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

func isPositive(s string) bool {
	n, err := strconv.ParseUint(s, 10, 32)
	return err == nil && n > 0
}

// styleDimension reads the integer of a `width:NNpx` or `height:NNpx`
// declaration. A matching declaration without a colon ends the search, as in
// the Rust implementation.
func styleDimension(style, property string) string {
	for _, decl := range strings.Split(style, ";") {
		key, value, hasColon := strings.Cut(decl, ":")
		if !strings.EqualFold(strings.TrimSpace(key), property) {
			continue
		}
		if !hasColon {
			return ""
		}
		value = strings.TrimSpace(value)
		end := 0
		for end < len(value) && value[end] >= '0' && value[end] <= '9' {
			end++
		}
		if end > 0 {
			return value[:end]
		}
	}
	return ""
}
