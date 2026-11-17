package sanitize

// ResolveLink returns raw as the `url` crate would serialise it: normalised
// when it is an absolute http(s) URL, joined to base when it is relative.
// base is the empty string when there is none. ok is false when neither
// applies, and the caller keeps the text as it is.
func ResolveLink(raw, base string) (resolved string, ok bool) {
	if u, parsed := parseURL(raw, nil); parsed {
		return u.String(), true
	}
	return joinURL(parseBase(base), raw)
}
