package matching

import (
	"unicode"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

type sequenceMatch struct{}

const maxDelta = 5

// Matches finds runs with a constant code point step, such as "abc", "9753"
// or "абв". It follows Rust zxcvbn 3.1.1, including its quirk that a leading
// zero step adopts the next one, so "aab" is a sequence.
func (sequenceMatch) Matches(password []rune) []*match.Match {
	matches := []*match.Match{}
	if len(password) <= 1 {
		return matches
	}
	i := 0
	lastDelta := 0
	for k := 1; k < len(password); k++ {
		delta := int(password[k]) - int(password[k-1])
		if lastDelta == 0 {
			lastDelta = delta
		}
		if delta == lastDelta {
			continue
		}
		j := k - 1
		matches = appendSequence(matches, password, sequenceSpan{i: i, j: j, delta: lastDelta})
		i = j
		lastDelta = delta
	}
	return appendSequence(matches, password, sequenceSpan{i: i, j: len(password) - 1, delta: lastDelta})
}

type sequenceSpan struct {
	i, j, delta int
}

func appendSequence(matches []*match.Match, password []rune, span sequenceSpan) []*match.Match {
	absDelta := max(span.delta, -span.delta)
	if span.j-span.i <= 1 && absDelta != 1 {
		return matches
	}
	if absDelta == 0 || absDelta > maxDelta {
		return matches
	}
	token := password[span.i : span.j+1]
	name, space := sequenceKind(token[0])
	return append(matches, &match.Match{
		Pattern:       "sequence",
		I:             span.i,
		J:             span.j,
		Token:         string(token),
		SequenceName:  name,
		SequenceSpace: space,
		Ascending:     span.delta > 0,
	})
}

// sequenceKind conservatively sizes every non-digit alphabet as the roman one.
func sequenceKind(first rune) (string, int) {
	switch {
	case unicode.IsLower(first):
		return "lower", 26
	case unicode.IsUpper(first):
		return "upper", 26
	case first >= '0' && first <= '9':
		return "digits", 10
	}
	return "unicode", 26
}
