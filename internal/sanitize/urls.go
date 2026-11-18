package sanitize

import "strings"

// The Rust sanitiser relied on the WHATWG URL parser (the `url` crate) to
// decide which attribute values are acceptable and to normalise the URLs it
// writes. Go's net/url follows RFC 3986, so whatwg.go reimplements the part
// that matters.

// urlAttrAllowed mirrors ammonia: a value is kept when it parses as an
// http(s) URL, or when it is relative (a parse failure that needs a base).
// Any other scheme, or a malformed http(s) URL, is dropped.
func urlAttrAllowed(value string) bool {
	input := trimURLInput(value)
	scheme, _, hasScheme := splitScheme(input)
	if !hasScheme {
		return true
	}
	if scheme != "http" && scheme != "https" {
		return false
	}
	_, ok := parseURL(input, nil)
	return ok
}

// trimURLInput strips leading and trailing C0 controls and spaces, then tabs
// and newlines anywhere, as the URL standard does before parsing.
func trimURLInput(s string) string {
	s = strings.TrimFunc(s, func(r rune) bool { return r <= 0x20 })
	return strings.NewReplacer("\t", "", "\n", "", "\r", "").Replace(s)
}

// splitScheme returns the lowercased scheme when s starts with one.
func splitScheme(s string) (scheme, rest string, ok bool) {
	if s == "" || !isASCIIAlpha(s[0]) {
		return "", s, false
	}
	for i := 1; i < len(s); i++ {
		c := s[i]
		switch {
		case c == ':':
			return strings.ToLower(s[:i]), s[i+1:], true
		case isASCIIAlpha(c), c >= '0' && c <= '9', c == '+', c == '-', c == '.':
		default:
			return "", s, false
		}
	}
	return "", s, false
}

func isASCIIAlpha(c byte) bool { return c|0x20 >= 'a' && c|0x20 <= 'z' }

// parseBase parses a document base; the empty string means there is none.
func parseBase(base string) *specialURL {
	if base == "" {
		return nil
	}
	u, _ := parseURL(base, nil)
	return u
}

// joinURL resolves ref against base the way `Url::join` does; ok is false
// when the base is unusable or the result is not a valid URL.
func joinURL(base *specialURL, ref string) (string, bool) {
	if base == nil {
		return "", false
	}
	u, ok := parseURL(ref, base)
	if !ok {
		return "", false
	}
	return u.String(), true
}
