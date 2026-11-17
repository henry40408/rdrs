package server

import (
	"slices"
	"strings"

	"github.com/henry40408/rdrs/internal/textutil"
)

// asciiLower lowers only A-Z, like Rust's to_ascii_lowercase and SQLite's LIKE;
// it keeps byte offsets intact, which strings.ToLower does not.
func asciiLower(s string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'A' && r <= 'Z' {
			return r + 'a' - 'A'
		}
		return r
	}, s)
}

// buildSnippet is a max-rune window of the content's plain text, centred near
// the first match of any term and elided with "…"; the leading window when
// nothing matches.
func buildSnippet(html *string, terms []string, maxRunes int) string {
	if html == nil || *html == "" {
		return ""
	}
	plain := textutil.StripToPlainText(*html)
	runes := []rune(plain)
	if len(runes) <= maxRunes {
		return plain
	}
	matchAt := firstMatchByte(asciiLower(plain), terms)
	if matchAt < 0 {
		return strings.TrimRight(string(runes[:maxRunes]), " \t\n\r\f\v") + "…"
	}
	matchRune := len([]rune(plain[:matchAt]))
	start := max(matchRune-maxRunes/3, 0)
	end := min(start+maxRunes, len(runes))
	// Fill the window when it hits the tail.
	start = max(end-maxRunes, 0)
	prefix, suffix := "", ""
	if start > 0 {
		prefix = "…"
	}
	if end < len(runes) {
		suffix = "…"
	}
	return prefix + strings.TrimSpace(string(runes[start:end])) + suffix
}

// firstMatchByte is the lowest byte offset of any non-blank term in lowered, or -1.
func firstMatchByte(lowered string, terms []string) int {
	first := -1
	for _, term := range terms {
		term = strings.TrimSpace(term)
		if term == "" {
			continue
		}
		at := strings.Index(lowered, asciiLower(term))
		if at >= 0 && (first < 0 || at < first) {
			first = at
		}
	}
	return first
}

var minimalEscape = strings.NewReplacer("<", "&lt;", ">", "&gt;", "&", "&amp;", `"`, "&quot;", "'", "&#39;")

// highlightHTML escapes text and wraps every ASCII-case-insensitive match of a
// term in <mark>, merging overlaps. The result is safe to emit raw.
func highlightHTML(text string, terms []string) string {
	ranges := matchRanges(asciiLower(text), terms)
	if len(ranges) == 0 {
		return minimalEscape.Replace(text)
	}
	var out strings.Builder
	last := 0
	for _, r := range ranges {
		out.WriteString(minimalEscape.Replace(text[last:r[0]]))
		out.WriteString("<mark>" + minimalEscape.Replace(text[r[0]:r[1]]) + "</mark>")
		last = r[1]
	}
	out.WriteString(minimalEscape.Replace(text[last:]))
	return out.String()
}

// matchRanges are the merged, sorted [start, end) byte ranges of every term.
func matchRanges(lowered string, terms []string) [][2]int {
	var found [][2]int
	for _, term := range terms {
		needle := asciiLower(term)
		if needle == "" {
			continue
		}
		for from := 0; ; {
			at := strings.Index(lowered[from:], needle)
			if at < 0 {
				break
			}
			start := from + at
			found = append(found, [2]int{start, start + len(needle)})
			from = start + len(needle)
		}
	}
	return mergeRanges(found)
}

// mergeRanges joins ranges that overlap or touch.
func mergeRanges(found [][2]int) [][2]int {
	slices.SortFunc(found, func(x, y [2]int) int {
		if x[0] != y[0] {
			return x[0] - y[0]
		}
		return x[1] - y[1]
	})
	var merged [][2]int
	for _, r := range found {
		last := len(merged) - 1
		if last >= 0 && r[0] <= merged[last][1] {
			merged[last][1] = max(merged[last][1], r[1])
			continue
		}
		merged = append(merged, r)
	}
	return merged
}
