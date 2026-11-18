// Package zxcvbn estimates password strength.
//
// It is a fork of github.com/trustelem/zxcvbn v1.0.1 (MIT, see LICENSE.txt).
// Upstream mixes byte and rune offsets, so it misjudges non-ASCII passwords
// in both directions. This fork indexes runes throughout and follows Rust
// zxcvbn 3.1.1 where the two differ, because rdrs' password policy was
// first written against the Rust crate and must keep its verdicts.
package zxcvbn

import (
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
	"github.com/henry40408/rdrs/internal/zxcvbn/matching"
	"github.com/henry40408/rdrs/internal/zxcvbn/scoring"
)

// maxEvaluatedRunes bounds the matching work, which grows with the square of
// the length. Rust zxcvbn 3.1.1 evaluates the same prefix.
const maxEvaluatedRunes = 100

type Result struct {
	Guesses  float64
	Sequence []*match.Match
	Score    int
}

// PasswordStrength scores password from 0 (too guessable) to 4. userInputs,
// such as the username, count as known to the attacker.
func PasswordStrength(password string, userInputs []string) Result {
	// Invalid UTF-8 has no meaningful runes; report it as weakest.
	if password == "" || !utf8.ValidString(password) {
		return Result{}
	}
	runes := []rune(password)
	if len(runes) > maxEvaluatedRunes {
		runes = runes[:maxEvaluatedRunes]
	}
	matches := matching.Omnimatch(runes, userInputs)
	seq := scoring.MostGuessableMatchSequence(runes, matches, false)
	return Result{
		Guesses:  seq.Guesses,
		Sequence: seq.Sequence,
		Score:    guessesToScore(seq.Guesses),
	}
}

func guessesToScore(guesses float64) int {
	const delta = 5
	switch {
	case guesses < 1e3+delta:
		return 0
	case guesses < 1e6+delta:
		return 1
	case guesses < 1e8+delta:
		return 2
	case guesses < 1e10+delta:
		return 3
	}
	return 4
}
