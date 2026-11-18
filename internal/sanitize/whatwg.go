package sanitize

import (
	"net/netip"
	"strconv"
	"strings"

	"golang.org/x/net/idna"
)

// specialURL is an http or https URL parsed the way the URL standard (and so
// the Rust `url` crate) does. Only these two schemes matter here: they are
// the only ones the sanitiser keeps, and entry links feed an ID hash, so the
// serialisation must match byte for byte.
type specialURL struct {
	scheme   string
	user     string
	password string
	host     string
	port     string // empty when absent or the scheme default
	segments []string
	query    string
	hasQuery bool
	fragment string
	hasFrag  bool
}

// The percent-encode sets of url 2.5.8. Controls, space handling and every
// non-ASCII byte are decided in encodeSet.
const (
	fragmentSet = ` "<>` + "`"
	querySet    = ` "#<>`
	specialQ    = querySet + `'`
	pathSet     = fragmentSet + `#?{}`
	userinfoSet = pathSet + `/:;=@[\]^|`
)

// parseURL parses raw against base, which may be nil. It fails for any
// scheme other than http and https.
func parseURL(raw string, base *specialURL) (*specialURL, bool) {
	input := trimURLInput(raw)
	scheme, rest, hasScheme := splitScheme(input)
	if !hasScheme {
		if base == nil {
			return nil, false
		}
		return parseRelative(input, base)
	}
	if scheme != "http" && scheme != "https" {
		return nil, false
	}
	if base != nil && base.scheme == scheme && !strings.HasPrefix(rest, "//") {
		return parseRelative(rest, base)
	}
	return parseAuthority(scheme, strings.TrimLeft(rest, `/\`))
}

func isSlash(c byte) bool { return c == '/' || c == '\\' }

func parseRelative(input string, base *specialURL) (*specialURL, bool) {
	if len(input) >= 2 && isSlash(input[0]) && isSlash(input[1]) {
		return parseAuthority(base.scheme, strings.TrimLeft(input, `/\`))
	}
	u := *base
	u.hasFrag, u.fragment = false, ""
	if input == "" {
		return &u, true
	}
	if isSlash(input[0]) {
		u.hasQuery, u.query = false, ""
		u.segments = nil
		u.addPath(u.splitTail(input[1:]))
		return &u, true
	}
	if input[0] == '#' || input[0] == '?' {
		u.splitTail(input)
		return &u, true
	}
	u.hasQuery, u.query = false, ""
	u.segments = append([]string(nil), base.segments[:len(base.segments)-1]...)
	u.addPath(u.splitTail(input))
	return &u, true
}

// parseAuthority parses everything after the scheme's slashes.
func parseAuthority(scheme, rest string) (*specialURL, bool) {
	end := strings.IndexAny(rest, `/\?#`)
	if end < 0 {
		end = len(rest)
	}
	authority, tail := rest[:end], rest[end:]
	u := &specialURL{scheme: scheme}
	if at := strings.LastIndexByte(authority, '@'); at >= 0 {
		credentials := authority[:at]
		authority = authority[at+1:]
		user, password, _ := strings.Cut(credentials, ":")
		u.user = encodeSet(user, userinfoSet)
		u.password = encodeSet(password, userinfoSet)
	}
	host, port, ok := splitHostPort(authority)
	if !ok {
		return nil, false
	}
	if u.host, ok = parseHost(host); !ok {
		return nil, false
	}
	if u.port, ok = parsePort(port, scheme); !ok {
		return nil, false
	}
	if tail != "" && isSlash(tail[0]) {
		tail = tail[1:]
	}
	u.addPath(u.splitTail(tail))
	return u, true
}

// splitTail takes the fragment and the query off tail, stores them and
// returns the path that is left.
func (u *specialURL) splitTail(tail string) string {
	if i := strings.IndexByte(tail, '#'); i >= 0 {
		u.fragment, u.hasFrag = encodeSet(tail[i+1:], fragmentSet), true
		tail = tail[:i]
	}
	if i := strings.IndexByte(tail, '?'); i >= 0 {
		u.query, u.hasQuery = encodeSet(tail[i+1:], specialQ), true
		tail = tail[:i]
	}
	return tail
}

// addPath appends the segments of path, resolving dot segments.
func (u *specialURL) addPath(path string) {
	pieces := splitSegments(path)
	for i, piece := range pieces {
		last := i == len(pieces)-1
		switch {
		case isDoubleDot(piece):
			if len(u.segments) > 0 {
				u.segments = u.segments[:len(u.segments)-1]
			}
			if last {
				u.segments = append(u.segments, "")
			}
		case isSingleDot(piece):
			if last {
				u.segments = append(u.segments, "")
			}
		default:
			u.segments = append(u.segments, encodeSet(piece, pathSet))
		}
	}
}

func splitSegments(path string) []string {
	var pieces []string
	start := 0
	for i := 0; i < len(path); i++ {
		if isSlash(path[i]) {
			pieces = append(pieces, path[start:i])
			start = i + 1
		}
	}
	return append(pieces, path[start:])
}

func isSingleDot(s string) bool {
	return s == "." || strings.EqualFold(s, "%2e")
}

func isDoubleDot(s string) bool {
	switch strings.ToLower(s) {
	case "..", ".%2e", "%2e.", "%2e%2e":
		return true
	}
	return false
}

// encodeSet percent-encodes controls, non-ASCII bytes and the bytes in set.
func encodeSet(s, set string) string {
	const upperHex = "0123456789ABCDEF"
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c > 0x20 && c < 0x7F && strings.IndexByte(set, c) < 0 {
			b.WriteByte(c)
			continue
		}
		b.WriteByte('%')
		b.WriteByte(upperHex[c>>4])
		b.WriteByte(upperHex[c&15])
	}
	return b.String()
}

func parsePort(port, scheme string) (string, bool) {
	if port == "" {
		return "", true
	}
	n, err := strconv.ParseUint(port, 10, 32)
	if err != nil || n > 65535 {
		return "", false
	}
	if (scheme == "http" && n == 80) || (scheme == "https" && n == 443) {
		return "", true
	}
	return strconv.FormatUint(n, 10), true
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
	host, port, _ = strings.Cut(authority, ":")
	return host, port, true
}

const forbiddenHostBytes = " #%/:<>?@[\\]^|\x7f"

func parseHost(host string) (string, bool) {
	if strings.HasPrefix(host, "[") {
		addr, err := netip.ParseAddr(strings.Trim(host, "[]"))
		if err != nil || !addr.Is6() || addr.Zone() != "" {
			return "", false
		}
		return "[" + addr.String() + "]", true
	}
	decoded := percentDecodeLenient(host)
	if !isASCII(decoded) || hasACELabel(decoded) {
		ascii, err := idna.Lookup.ToASCII(decoded)
		if err != nil {
			return "", false
		}
		decoded = ascii
	}
	decoded = strings.ToLower(decoded)
	if decoded == "" || strings.ContainsAny(decoded, forbiddenHostBytes) || hasC0Control(decoded) {
		return "", false
	}
	if endsInNumber(decoded) {
		return parseIPv4(decoded)
	}
	return decoded, true
}

func hasACELabel(host string) bool {
	for _, label := range strings.Split(strings.ToLower(host), ".") {
		if strings.HasPrefix(label, "xn--") {
			return true
		}
	}
	return false
}

func hasC0Control(s string) bool {
	return strings.IndexFunc(s, func(r rune) bool { return r < 0x20 }) >= 0
}

func isASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] >= 0x80 {
			return false
		}
	}
	return true
}

func hostLabels(host string) []string {
	labels := strings.Split(host, ".")
	if len(labels) > 1 && labels[len(labels)-1] == "" {
		labels = labels[:len(labels)-1]
	}
	return labels
}

// endsInNumber is the URL standard's test for treating a host as IPv4.
func endsInNumber(host string) bool {
	labels := hostLabels(host)
	last := labels[len(labels)-1]
	if last == "" {
		return false
	}
	if strings.Trim(last, "0123456789") == "" {
		return true
	}
	_, ok := parseIPv4Number(last)
	return ok
}

func parseIPv4Number(s string) (uint64, bool) {
	base := 10
	switch {
	case len(s) >= 2 && (s[:2] == "0x" || s[:2] == "0X"):
		s, base = s[2:], 16
	case len(s) >= 2 && s[0] == '0':
		s, base = s[1:], 8
	}
	if s == "" {
		return 0, true
	}
	n, err := strconv.ParseUint(s, base, 64)
	return n, err == nil
}

func parseIPv4(host string) (string, bool) {
	labels := hostLabels(host)
	if len(labels) > 4 {
		return "", false
	}
	numbers := make([]uint64, len(labels))
	for i, label := range labels {
		n, ok := parseIPv4Number(label)
		if !ok {
			return "", false
		}
		numbers[i] = n
	}
	last := numbers[len(numbers)-1]
	for _, n := range numbers[:len(numbers)-1] {
		if n > 255 {
			return "", false
		}
	}
	limit := uint64(1) << (8 * (5 - len(numbers)))
	if last >= limit {
		return "", false
	}
	ipv4 := last
	for i, n := range numbers[:len(numbers)-1] {
		ipv4 += n << (8 * (3 - i))
	}
	return strconv.FormatUint(ipv4>>24, 10) + "." + strconv.FormatUint(ipv4>>16&255, 10) + "." +
		strconv.FormatUint(ipv4>>8&255, 10) + "." + strconv.FormatUint(ipv4&255, 10), true
}

// String serialises the URL.
func (u *specialURL) String() string {
	var b strings.Builder
	b.WriteString(u.scheme + "://")
	if u.user != "" || u.password != "" {
		b.WriteString(u.user)
		if u.password != "" {
			b.WriteString(":" + u.password)
		}
		b.WriteByte('@')
	}
	b.WriteString(u.host)
	if u.port != "" {
		b.WriteString(":" + u.port)
	}
	b.WriteString("/" + strings.Join(u.segments, "/"))
	if u.hasQuery {
		b.WriteString("?" + u.query)
	}
	if u.hasFrag {
		b.WriteString("#" + u.fragment)
	}
	return b.String()
}
