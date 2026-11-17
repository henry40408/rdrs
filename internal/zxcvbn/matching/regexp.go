package matching

import (
	"regexp"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

// recentYear is the Rust zxcvbn 3.1.1 pattern; upstream stopped at 2019.
var recentYear = regexp.MustCompile(`19[0-9]{2}|20[0-9]{2}`)

type regexpMatch struct{}

func (regexpMatch) Matches(password []rune) []*match.Match {
	var matches []*match.Match
	text := string(password)
	for _, span := range recentYear.FindAllStringIndex(text, -1) {
		i := utf8.RuneCountInString(text[:span[0]])
		matches = append(matches, &match.Match{
			Pattern:   "regex",
			Token:     text[span[0]:span[1]],
			I:         i,
			J:         i + utf8.RuneCountInString(text[span[0]:span[1]]) - 1,
			RegexName: "recent_year",
		})
	}
	return matches
}
