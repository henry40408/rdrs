package server

import (
	"reflect"
	"testing"
)

func TestAsciiLowerStopsAtTheEdgesOfTheCapitals(t *testing.T) {
	t.Parallel()
	// '@' and '[' are the neighbours of 'A' and 'Z'.
	if got := asciiLower("@AZ[az"); got != "@az[az" {
		t.Errorf("got %q", got)
	}
	if got := asciiLower("ÀÉ"); got != "ÀÉ" {
		t.Errorf("non-ASCII letters changed: %q", got)
	}
}

func snippetOf(text string, terms []string, maxRunes int) string {
	html := "<p>" + text + "</p>"
	return buildSnippet(&html, terms, maxRunes)
}

func TestBuildSnippetKeepsATextThatFitsExactly(t *testing.T) {
	t.Parallel()
	if got := snippetOf("0123456789", []string{"nomatch"}, 10); got != "0123456789" {
		t.Errorf("exactly the limit = %q", got)
	}
	if got := snippetOf("0123456789a", []string{"nomatch"}, 10); got != "0123456789…" {
		t.Errorf("one over the limit = %q", got)
	}
}

func TestBuildSnippetWithoutAMatchIsTheLeadingWindow(t *testing.T) {
	t.Parallel()
	if got := snippetOf("abcd efgh ijkl", []string{"zzz"}, 5); got != "abcd…" {
		t.Errorf("got %q", got)
	}
	if got := snippetOf("abcd efgh ijkl", nil, 5); got != "abcd…" {
		t.Errorf("no terms = %q", got)
	}
}

func TestBuildSnippetWindowEdgesAreExact(t *testing.T) {
	t.Parallel()
	const text = "abcdefghijklmnopqrstuvwxyz" // 26 runes
	// The window of 10 starts a third (3) before the match.
	if got := snippetOf(text, []string{"MNO"}, 10); got != "…jklmnopqrs…" {
		t.Errorf("middle = %q", got)
	}
	// A match 3 runes in: the window starts at 0, so no leading mark.
	if got := snippetOf(text, []string{"def"}, 10); got != "abcdefghij…" {
		t.Errorf("start edge = %q", got)
	}
	// The window reaches the last rune exactly: no trailing mark, and it fills back.
	if got := snippetOf(text, []string{"xyz"}, 10); got != "…qrstuvwxyz" {
		t.Errorf("tail = %q", got)
	}
	if got := snippetOf(text, []string{"qrs"}, 10); got != "…nopqrstuvw…" {
		t.Errorf("one short of the tail = %q", got)
	}
}

func TestFirstMatchByteIsTheLowestOffsetOfAnyTerm(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name  string
		terms []string
		want  int
	}{
		{"at the very start", []string{"abc"}, 0},
		{"earlier term first", []string{"a", "c"}, 0},
		{"later term earlier in the text", []string{"c", "a"}, 0},
		{"order of equal offsets", []string{"ab", "a"}, 0},
		{"first term at the start keeps winning", []string{"a", "b", "c"}, 0},
		{"blank terms are skipped", []string{" ", "", "c"}, 2},
		{"upper-case term", []string{"C"}, 2},
		{"none", []string{"zzz"}, -1},
		{"no terms", nil, -1},
	}
	for _, tc := range cases {
		if got := firstMatchByte("abc", tc.terms); got != tc.want {
			t.Errorf("%s: %d, want %d", tc.name, got, tc.want)
		}
	}
	if got := firstMatchByte("xxabxxab", []string{"b", "ab"}); got != 2 {
		t.Errorf("got %d, want 2", got)
	}
}

func TestMergeRangesSortsThenJoinsOverlapsAndTouches(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		in   [][2]int
		want [][2]int
	}{
		{"unsorted", [][2]int{{5, 6}, {0, 2}, {8, 9}, {3, 4}}, [][2]int{{0, 2}, {3, 4}, {5, 6}, {8, 9}}},
		{"touching ranges join", [][2]int{{1, 2}, {0, 1}}, [][2]int{{0, 2}}},
		{"a gap of one keeps them apart", [][2]int{{0, 1}, {2, 3}}, [][2]int{{0, 1}, {2, 3}}},
		{"overlap joins to the larger end", [][2]int{{4, 6}, {0, 10}, {2, 3}}, [][2]int{{0, 10}}},
		{"same start, shorter first", [][2]int{{0, 5}, {0, 1}, {3, 4}}, [][2]int{{0, 5}}},
		{"descending", [][2]int{{9, 10}, {6, 7}, {3, 4}, {0, 1}}, [][2]int{{0, 1}, {3, 4}, {6, 7}, {9, 10}}},
		{"empty", nil, nil},
	}
	for _, tc := range cases {
		if got := mergeRanges(tc.in); !reflect.DeepEqual(got, tc.want) {
			t.Errorf("%s: %v, want %v", tc.name, got, tc.want)
		}
	}
}

func TestHighlightHTMLMergesAdjacentTermsAndEscapes(t *testing.T) {
	t.Parallel()
	if got := highlightHTML("xAbCx", []string{"a", "b", "c"}); got != "x<mark>AbC</mark>x" {
		t.Errorf("adjacent terms = %q", got)
	}
	if got := highlightHTML("a<b>&c", []string{"b"}); got != "a&lt;<mark>b</mark>&gt;&amp;c" {
		t.Errorf("escaping = %q", got)
	}
	if got := highlightHTML("aaaa", []string{"aa"}); got != "<mark>aaaa</mark>" {
		t.Errorf("repeats = %q", got)
	}
}
