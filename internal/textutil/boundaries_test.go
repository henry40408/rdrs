package textutil

import "testing"

func TestASCIIHelpersCoverTheirFullRanges(t *testing.T) {
	for b := range 256 {
		wantSpace := b == ' ' || (b >= 9 && b <= 13)
		if got := isASCIISpace(byte(b)); got != wantSpace {
			t.Errorf("isASCIISpace(%#x) = %v", b, got)
		}
		wantLower := byte(b)
		if b >= 'A' && b <= 'Z' {
			wantLower += 'a' - 'A'
		}
		if got := lowerASCII(byte(b)); got != wantLower {
			t.Errorf("lowerASCII(%#x) = %#x, want %#x", b, got, wantLower)
		}
	}
}

func TestFoldedSearchMatchesAtTheEnds(t *testing.T) {
	if !hasPrefixFold("ABC", "abc") {
		t.Error("a haystack as long as the needle must match")
	}
	if hasPrefixFold("ab", "abc") {
		t.Error("a shorter haystack must not match")
	}
	for haystack, want := range map[string]int{"xxAB": 2, "AB": 0, "xA": -1, "": -1} {
		if got := indexFold(haystack, "ab"); got != want {
			t.Errorf("indexFold(%q) = %d, want %d", haystack, got, want)
		}
	}
}

func TestEmptyCommentIsStripped(t *testing.T) {
	if got := StripToPlainText("a<!---->b"); got != "a b" {
		t.Errorf("got %q", got)
	}
	if got := StripToSearchText("a<!---->b"); got != "ab" {
		t.Errorf("got %q", got)
	}
	if got := StripToPlainText("a\rb"); got != "a b" {
		t.Errorf("carriage return: got %q", got)
	}
}
