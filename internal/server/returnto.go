package server

import (
	"net/url"
	"strings"
	"unicode"
)

// SafeReturnTo is raw as a same-origin path and query, if it is a rooted path
// that allow accepts after normalisation (/feeds/../admin is checked as
// /admin). Scheme-relative, backslash and control-character tricks are
// rejected, so the result can never leave this origin.
func SafeReturnTo(raw string, allow func(path string) bool) (string, bool) {
	if !strings.HasPrefix(raw, "/") || strings.HasPrefix(raw, "//") ||
		strings.Contains(raw, `\`) || strings.IndexFunc(raw, unicode.IsControl) >= 0 {
		return "", false
	}
	// A fragment never reaches the server, and drops out of the result.
	beforeFragment, _, _ := strings.Cut(raw, "#")
	rawPath, rawQuery, hasQuery := strings.Cut(beforeFragment, "?")
	path := encodeURLPart(removeDotSegments(rawPath), pathEncodeSet)
	if !allow(path) {
		return "", false
	}
	if !hasQuery {
		return path, true
	}
	return path + "?" + encodeURLPart(rawQuery, queryEncodeSet), true
}

// removeDotSegments resolves "." and ".." the way the WHATWG URL parser does,
// which also treats their percent-encoded forms as dots.
func removeDotSegments(path string) string {
	segments := strings.Split(path, "/")[1:]
	resolved := make([]string, 0, len(segments))
	for i, segment := range segments {
		last := i == len(segments)-1
		switch dotKind(segment) {
		case singleDot:
			if last {
				resolved = append(resolved, "")
			}
		case doubleDot:
			if len(resolved) > 0 {
				resolved = resolved[:len(resolved)-1]
			}
			if last {
				resolved = append(resolved, "")
			}
		default:
			resolved = append(resolved, segment)
		}
	}
	return "/" + strings.Join(resolved, "/")
}

type segmentKind int

const (
	plainSegment segmentKind = iota
	singleDot
	doubleDot
)

func dotKind(segment string) segmentKind {
	switch strings.ToLower(segment) {
	case ".", "%2e":
		return singleDot
	case "..", ".%2e", "%2e.", "%2e%2e":
		return doubleDot
	}
	return plainSegment
}

// encodeSet says which ASCII bytes are percent-encoded; non-ASCII always is.
type encodeSet func(b byte) bool

func pathEncodeSet(b byte) bool {
	return queryEncodeSet(b) || b == '?' || b == '`' || b == '{' || b == '}'
}

func queryEncodeSet(b byte) bool {
	return b <= ' ' || b == '"' || b == '#' || b == '<' || b == '>' || b == '\'' || b == 0x7f
}

const upperHex = "0123456789ABCDEF"

func encodeURLPart(part string, inSet encodeSet) string {
	var out strings.Builder
	for i := 0; i < len(part); i++ {
		b := part[i]
		if b < 0x80 && !inSet(b) {
			out.WriteByte(b)
			continue
		}
		out.WriteByte('%')
		out.WriteByte(upperHex[b>>4])
		out.WriteByte(upperHex[b&0xf])
	}
	return out.String()
}

// ReturnToQuery is returnTo as a key=value query string, for links that carry
// it along.
func ReturnToQuery(returnTo string) string {
	return url.Values{"return_to": {returnTo}}.Encode()
}

// FeedsList is the bare feeds list, where feed forms land without a usable
// return_to.
const FeedsList = "/feeds"

// FeedsListReturn is the (filtered) feeds list a feed form came from, else
// FeedsList.
func FeedsListReturn(raw string) string {
	target, ok := SafeReturnTo(raw, func(path string) bool { return path == FeedsList })
	if !ok {
		return FeedsList
	}
	return target
}

// loginExcluded are the signed-out flows and machine endpoints that sign-in
// must not land on: they would loop or render nothing useful.
var loginExcluded = []string{"/login", "/logout", "/setup", "/invite", "/api", "/reader", "/static"}

// IsLoginDestination reports whether sign-in may send the user to path.
func IsLoginDestination(path string) bool {
	for _, prefix := range loginExcluded {
		rest, found := strings.CutPrefix(path, prefix)
		if found && (rest == "" || strings.HasPrefix(rest, "/")) {
			return false
		}
	}
	return true
}

// loginNext is the validated "next" page of the login form and query, or "".
func loginNext(raw string) string {
	next, _ := SafeReturnTo(raw, IsLoginDestination)
	return next
}
