package matching

import (
	"slices"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

type reverseDictionnaryMatch struct {
	dm dictionaryMatch
}

func (rdm reverseDictionnaryMatch) Matches(password []rune) []*match.Match {
	matches := rdm.dm.Matches(reversed(password))
	last := len(password) - 1
	for _, m := range matches {
		m.Token = string(reversed([]rune(m.Token)))
		m.Reversed = true
		m.I, m.J = last-m.J, last-m.I
	}
	match.Sort(matches)
	return matches
}

func reversed(runes []rune) []rune {
	out := slices.Clone(runes)
	slices.Reverse(out)
	return out
}
