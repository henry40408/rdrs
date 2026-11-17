package sanitize

import "net/url"

// ResolveLink returns raw as the `url` crate would serialise it: normalised
// when it is an absolute http(s) URL, joined to base when it is relative.
// ok is false when neither applies, and the caller keeps the text as it is.
func ResolveLink(raw string, base *url.URL) (resolved string, ok bool) {
	if u, parsed := parseSpecial(raw); parsed {
		return normalizeURL(u)
	}
	if base == nil {
		return "", false
	}
	if u, err := url.Parse(trimURLInput(raw)); err != nil || u.Scheme != "" {
		return "", false
	}
	return joinURL(base, raw)
}
