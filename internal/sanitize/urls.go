package sanitize

import (
	"net/netip"
	"net/url"
	"strconv"
	"strings"

	"golang.org/x/net/idna"
)

// The Rust sanitiser relied on the WHATWG URL parser (the `url` crate) to
// decide which attribute values are acceptable and to normalise the URLs it
// writes. Go's net/url follows RFC 3986, so the few places that matter are
// emulated here.

// urlAttrAllowed mirrors ammonia: a value is kept when it parses as an
// http(s) URL, or when it is relative (a parse failure that needs a base).
// Any other scheme, or a malformed http(s) URL, is dropped.
func urlAttrAllowed(value string) bool {
	input := trimURLInput(value)
	scheme, rest, hasScheme := splitScheme(input)
	if !hasScheme {
		return true
	}
	if scheme != "http" && scheme != "https" {
		return false
	}
	return validSpecialAuthority(rest)
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

// validSpecialAuthority checks what follows `http:` has a usable host and port.
func validSpecialAuthority(rest string) bool {
	rest = strings.TrimLeft(rest, `/\`)
	end := strings.IndexAny(rest, `/\?#`)
	if end >= 0 {
		rest = rest[:end]
	}
	if at := strings.LastIndexByte(rest, '@'); at >= 0 {
		rest = rest[at+1:]
	}
	host, port, ok := splitHostPort(rest)
	return ok && validPort(port) && validHost(host)
}

func splitHostPort(authority string) (host, port string, ok bool) {
	if strings.HasPrefix(authority, "[") {
		closing := strings.IndexByte(authority, ']')
		if closing < 0 {
			return "", "", false
		}
		host, tail := authority[:closing+1], authority[closing+1:]
		if tail == "" {
			return host, "", true
		}
		if tail[0] != ':' {
			return "", "", false
		}
		return host, tail[1:], true
	}
	if colon := strings.IndexByte(authority, ':'); colon >= 0 {
		return authority[:colon], authority[colon+1:], true
	}
	return authority, "", true
}

func validPort(port string) bool {
	if port == "" {
		return true
	}
	n, err := strconv.ParseUint(port, 10, 32)
	return err == nil && n <= 65535
}

func validHost(host string) bool {
	if strings.HasPrefix(host, "[") {
		_, err := netip.ParseAddr(strings.Trim(host, "[]"))
		return err == nil
	}
	decoded, err := url.PathUnescape(host)
	if err != nil || decoded == "" {
		return false
	}
	return !strings.ContainsAny(decoded, " #%/:<>?@[\\]^|\x7f") && !hasC0Control(decoded)
}

func hasC0Control(s string) bool {
	return strings.IndexFunc(s, func(r rune) bool { return r < 0x20 }) >= 0
}

// normalizeURL approximates the `url` crate's serialisation: lowercase
// host, IDNA, default port removed, dot segments resolved, "/" for an empty
// path.
func normalizeURL(u *url.URL) (string, bool) {
	out := (&url.URL{}).ResolveReference(u)
	if out.Host != "" {
		host, ok := normalizeHost(out)
		if !ok {
			return "", false
		}
		out.Host = host
		if out.Path == "" {
			out.Path = "/"
		}
	}
	return out.String(), true
}

func normalizeHost(u *url.URL) (string, bool) {
	name := strings.ToLower(u.Hostname())
	if !isASCII(name) {
		ascii, err := idna.Lookup.ToASCII(name)
		if err != nil {
			return "", false
		}
		name = ascii
	}
	if strings.Contains(name, ":") {
		name = "[" + name + "]"
	}
	port := u.Port()
	if port == "" || (port == "80" && u.Scheme == "http") || (port == "443" && u.Scheme == "https") {
		return name, true
	}
	return name + ":" + port, true
}

func isASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] >= 0x80 {
			return false
		}
	}
	return true
}

// parseSpecial parses an absolute http(s) URL, treating `\` as `/` before
// the query like the URL standard does for special schemes.
func parseSpecial(raw string) (*url.URL, bool) {
	raw = trimURLInput(raw)
	cut := strings.IndexAny(raw, "?#")
	if cut < 0 {
		cut = len(raw)
	}
	raw = strings.ReplaceAll(raw[:cut], `\`, "/") + raw[cut:]
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" || u.Opaque != "" {
		return nil, false
	}
	return u, true
}

// joinURL resolves ref against base the way `Url::join` does; ok is false
// when the base is unusable or the result is not a valid URL.
func joinURL(base *url.URL, ref string) (string, bool) {
	if base == nil || base.Scheme == "" || base.Opaque != "" {
		return "", false
	}
	parsed, err := url.Parse(trimURLInput(ref))
	if err != nil {
		return "", false
	}
	return normalizeURL(base.ResolveReference(parsed))
}
