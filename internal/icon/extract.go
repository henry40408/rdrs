package icon

import (
	"net/url"
	"strings"
)

// asciiLower leaves bytes above 0x7f alone, so offsets in the result are
// offsets in the input.
func asciiLower(s string) string {
	b := []byte(s)
	for i, c := range b {
		if 'A' <= c && c <= 'Z' {
			b[i] = c + 'a' - 'A'
		}
	}
	return string(b)
}

var iconRelPatterns = []string{
	`rel="icon"`,
	`rel='icon'`,
	`rel="shortcut icon"`,
	`rel='shortcut icon'`,
}

// extractFavicon finds the href of the first <link rel="icon"> by plain text
// search, not by parsing, as the Rust version did: a page that confuses a
// parser still yields its icon. It returns "" when none is found.
func extractFavicon(page string, base *url.URL) string {
	lower := asciiLower(page)
	for _, pattern := range iconRelPatterns {
		at := strings.Index(lower, pattern)
		if at < 0 {
			continue
		}
		tagStart := strings.LastIndex(lower[:at], "<link")
		if tagStart < 0 {
			return ""
		}
		tagEnd := strings.IndexByte(lower[tagStart:], '>')
		if tagEnd < 0 {
			return ""
		}
		href, ok := extractHref(page[tagStart : tagStart+tagEnd+1])
		if ok {
			return resolveURL(href, base)
		}
	}
	return ""
}

// extractHref reads a quoted href. Once the attribute is found, an unclosed
// quote ends the search without trying the other quote style.
func extractHref(tag string) (string, bool) {
	lower := asciiLower(tag)
	for _, prefix := range []string{`href="`, `href='`} {
		start := strings.Index(lower, prefix)
		if start < 0 {
			continue
		}
		valueStart := start + len(prefix)
		end := strings.IndexByte(tag[valueStart:], prefix[len(prefix)-1])
		if end < 0 {
			return "", false
		}
		return tag[valueStart : valueStart+end], true
	}
	return "", false
}

func resolveURL(href string, base *url.URL) string {
	if strings.HasPrefix(href, "http://") || strings.HasPrefix(href, "https://") {
		return href
	}
	if strings.HasPrefix(href, "//") {
		return base.Scheme + ":" + href
	}
	resolved, err := base.Parse(href)
	if err != nil {
		return ""
	}
	return resolved.String()
}
