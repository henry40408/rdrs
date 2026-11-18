package textutil

import (
	"strconv"
	"strings"
	"unicode/utf8"
)

// entityWindow is how many bytes after '&' may start the terminating ';'.
const entityWindow = 32

// DecodeHTMLEntities decodes the five named references the feeds produce and
// decimal or hex numeric references. Anything else is kept as written. It
// undoes the residual layer of a double-encoded title, so it must not be
// applied to HTML that goes through the sanitizer. html.UnescapeString is not
// used: it knows 2000 more names and accepts references without a ';'.
func DecodeHTMLEntities(s string) string {
	if !strings.Contains(s, "&") {
		return s
	}
	var out strings.Builder
	out.Grow(len(s))
	for i := 0; i < len(s); {
		if s[i] != '&' {
			_, size := utf8.DecodeRuneInString(s[i:])
			out.WriteString(s[i : i+size])
			i += size
			continue
		}
		if decoded, end, ok := decodeEntityAt(s, i); ok {
			out.WriteRune(decoded)
			i = end
			continue
		}
		out.WriteByte('&')
		i++
	}
	return out.String()
}

// decodeEntityAt tries the reference starting at s[at] == '&'. end is the
// index after its ';'.
func decodeEntityAt(s string, at int) (decoded rune, end int, ok bool) {
	semi := findSemicolon(s[at+1:])
	if semi < 0 {
		return 0, 0, false
	}
	semi += at + 1
	decoded, ok = decodeEntityBody(s[at+1 : semi])
	return decoded, semi + 1, ok
}

// findSemicolon returns the offset of ';' when it starts within the first
// entityWindow bytes, or -1.
func findSemicolon(s string) int {
	for offset, r := range s {
		if offset >= entityWindow {
			return -1
		}
		if r == ';' {
			return offset
		}
	}
	return -1
}

func decodeEntityBody(body string) (rune, bool) {
	switch body {
	case "amp":
		return '&', true
	case "lt":
		return '<', true
	case "gt":
		return '>', true
	case "quot":
		return '"', true
	case "apos":
		return '\'', true
	}
	number, ok := strings.CutPrefix(body, "#")
	if !ok {
		return 0, false
	}
	digits, base := number, 10
	if len(number) > 0 && (number[0] == 'x' || number[0] == 'X') {
		digits, base = number[1:], 16
	}
	// Rust's integer parser accepts one leading plus sign.
	digits = strings.TrimPrefix(digits, "+")
	if digits == "" || digits[0] == '+' {
		return 0, false
	}
	code, err := strconv.ParseUint(digits, base, 32)
	if err != nil || !utf8.ValidRune(rune(code)) {
		return 0, false
	}
	return rune(code), true
}
