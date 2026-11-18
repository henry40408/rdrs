// Package textutil holds the text helpers shared by feed storage and views.
package textutil

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// StripToPlainText strips HTML to plain text, inserting a space at tag
// boundaries so display snippets stay readable.
func StripToPlainText(raw string) string {
	return stripHTML(raw, true)
}

// StripToSearchText is StripToPlainText without a separator at tag
// boundaries, so "超<b>少女</b>" stays contiguous. It builds entry.content_text.
func StripToSearchText(raw string) string {
	return stripHTML(raw, false)
}

// isASCIISpace matches the bytes the Rust build treats as whitespace: it
// tests each byte as a Latin-1 char, but a UTF-8 lead byte is never 0x85 or
// 0xA0, so only ASCII whitespace can match.
func isASCIISpace(b byte) bool {
	return b == ' ' || (b >= '\t' && b <= '\r')
}

// hasPrefixFold is an ASCII-only case-insensitive prefix test; EqualFold would
// also fold non-ASCII letters such as U+017F into "s".
func hasPrefixFold(haystack, needle string) bool {
	if len(haystack) < len(needle) {
		return false
	}
	for i := 0; i < len(needle); i++ {
		if lowerASCII(haystack[i]) != lowerASCII(needle[i]) {
			return false
		}
	}
	return true
}

func lowerASCII(b byte) byte {
	if b >= 'A' && b <= 'Z' {
		return b + 'a' - 'A'
	}
	return b
}

// indexFold finds an ASCII needle in haystack, ignoring ASCII case.
func indexFold(haystack, needle string) int {
	for pos := 0; pos+len(needle) <= len(haystack); pos++ {
		if hasPrefixFold(haystack[pos:], needle) {
			return pos
		}
	}
	return -1
}

type stripper struct {
	raw      string
	tagGap   bool
	out      strings.Builder
	lastGap  bool
	inTag    bool
	skipTill string
}

func stripHTML(raw string, tagGap bool) string {
	s := &stripper{raw: raw, tagGap: tagGap, lastGap: true}
	s.run()
	return strings.TrimFunc(s.out.String(), unicode.IsSpace)
}

func (s *stripper) gap() {
	if s.tagGap && !s.lastGap {
		s.out.WriteByte(' ')
		s.lastGap = true
	}
}

func (s *stripper) run() {
	for i := 0; i < len(s.raw); {
		next, ok := s.step(i)
		if !ok {
			return
		}
		i = next
	}
}

// step consumes input at i and returns the next offset. It reports false when
// the rest of the input is swallowed (an unterminated script, style or comment).
func (s *stripper) step(i int) (int, bool) {
	if s.skipTill != "" {
		return s.skipPast(i)
	}
	b := s.raw[i]
	switch {
	case b == '<':
		return s.openAngle(i)
	case b == '>' && s.inTag:
		s.inTag = false
		s.gap()
		return i + 1, true
	case s.inTag:
		return i + 1, true
	case isASCIISpace(b):
		if !s.lastGap {
			s.out.WriteByte(' ')
			s.lastGap = true
		}
		return i + 1, true
	}
	_, size := utf8.DecodeRuneInString(s.raw[i:])
	s.out.WriteString(s.raw[i : i+size])
	s.lastGap = false
	return i + size, true
}

func (s *stripper) skipPast(i int) (int, bool) {
	pos := indexFold(s.raw[i:], s.skipTill)
	if pos < 0 {
		return 0, false
	}
	next := i + pos + len(s.skipTill)
	s.skipTill = ""
	s.inTag = false
	s.gap()
	return next, true
}

func (s *stripper) openAngle(i int) (int, bool) {
	rest := s.raw[i:]
	switch {
	case hasPrefixFold(rest, "<script"):
		s.skipTill = "</script>"
		return i + 1, true
	case hasPrefixFold(rest, "<style"):
		s.skipTill = "</style>"
		return i + 1, true
	case hasPrefixFold(rest, "<!--"):
		end := strings.Index(s.raw[i+4:], "-->")
		if end < 0 {
			return 0, false
		}
		s.gap()
		return i + 4 + end + 3, true
	}
	s.inTag = true
	return i + 1, true
}
