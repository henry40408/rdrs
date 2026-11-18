package server

import (
	"fmt"
	"strings"
	"testing"
)

func TestParseURLLinesAcceptsExactlyTheMaximum(t *testing.T) {
	t.Parallel()
	var lines []string
	for i := 0; i < maxSummarizerURLs; i++ {
		lines = append(lines, fmt.Sprintf("https://e.com/%d", i))
	}
	urls, err := parseURLLines(strings.Join(lines, "\n"))
	if err != nil || len(urls) != maxSummarizerURLs {
		t.Fatalf("%d URLs: got %d, %v", maxSummarizerURLs, len(urls), err)
	}
}

func TestParseURLLinesCountsDistinctURLsOnly(t *testing.T) {
	t.Parallel()
	var lines []string
	for i := 0; i < maxSummarizerURLs; i++ {
		lines = append(lines, fmt.Sprintf("https://e.com/%d", i))
	}
	lines = append(lines, lines[0])
	if _, err := parseURLLines(strings.Join(lines, "\n")); err != nil {
		t.Fatalf("a repeated line counted twice: %v", err)
	}
}

func TestParseHTTPURL(t *testing.T) {
	t.Parallel()
	cases := []struct {
		raw  string
		want bool
	}{
		{"https://example.com/a", true},
		{"http://example.com", true},
		{"ftp://example.com/a", true}, // the scheme check is the caller's
		{"mailto:someone@example.com", true},
		{"http:///path", false},
		{"https://", false},
		{"example.com/a", false},
		{"nonsense", false},
		{"", false},
		{"http://exa mple.com/", false},
		{"http://[::1", false},
	}
	for _, tc := range cases {
		parsed, ok := parseHTTPURL(tc.raw)
		if ok != tc.want {
			t.Errorf("%q ok = %v, want %v", tc.raw, ok, tc.want)
		}
		if ok && parsed == nil {
			t.Errorf("%q: ok with a nil URL", tc.raw)
		}
		if !ok && parsed != nil {
			t.Errorf("%q: refused with a URL", tc.raw)
		}
	}
}
