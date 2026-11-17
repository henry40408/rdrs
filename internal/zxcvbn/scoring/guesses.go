package scoring

import (
	"math"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/zxcvbn/adjacency"
	"github.com/henry40408/rdrs/internal/zxcvbn/internal/mathutils"
	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

const (
	BruteforceCardinality           = 10
	MinGuessesBeforeGrowingSequence = 10000
	MinSubmatchGuessesSingleChar    = 10
	MinSubmatchGuessesMultiChar     = 50
)

// EstimateGuesses is the guess count for m within a password of passwordLen
// runes.
func EstimateGuesses(m *match.Match, passwordLen int) float64 {
	if m.Guesses > 0 {
		// a match's guess estimate doesn't change. cache it.
		return m.Guesses
	}
	m.Guesses = max(patternGuesses(m), minGuesses(utf8.RuneCountInString(m.Token), passwordLen))
	return m.Guesses
}

func minGuesses(tokenLen, passwordLen int) float64 {
	switch {
	case tokenLen >= passwordLen:
		return 1
	case tokenLen == 1:
		return MinSubmatchGuessesSingleChar
	}
	return MinSubmatchGuessesMultiChar
}

func patternGuesses(m *match.Match) float64 {
	switch m.Pattern {
	case "bruteforce":
		return BruteforceGuesses(m)
	case "dictionary":
		return DictionaryGuesses(m)
	case "spatial":
		return SpatialGuesses(m)
	case "repeat":
		return RepeatGuesses(m)
	case "sequence":
		return SequenceGuesses(m)
	case "regex":
		return RegexGuesses(m)
	case "date":
		return DateGuesses(m)
	}
	return 0
}

func BruteforceGuesses(m *match.Match) float64 {
	runeCount := utf8.RuneCountInString(m.Token)
	guesses := math.Pow(BruteforceCardinality, float64(runeCount))
	// small detail: make bruteforce matches at minimum one guess bigger than smallest allowed
	// submatch guesses, such that non-bruteforce submatches over the same [i..j] take precedence.
	if runeCount == 1 {
		return max(guesses, MinSubmatchGuessesSingleChar+1)
	}
	return max(guesses, MinSubmatchGuessesMultiChar+1)
}

func DictionaryGuesses(m *match.Match) float64 {
	m.BaseGuesses = float64(m.Rank)
	m.UppercaseVariations = UppercaseVariations(m.Token)
	m.L33tVariations = L33tVariations(m)
	reversedVariations := 1.0
	if m.Reversed {
		reversedVariations = 2
	}
	return m.BaseGuesses * m.UppercaseVariations * m.L33tVariations * reversedVariations
}

// UppercaseVariations follows Rust zxcvbn 3.1.1, which judges case with
// Unicode properties rather than upstream's ASCII-only regexes.
func UppercaseVariations(w string) float64 {
	if strings.ToLower(w) == w || countRunes(w, isLowercase) == utf8.RuneCountInString(w) {
		return 1
	}
	// a capitalized word is the most common capitalization scheme,
	// so it only doubles the search space (uncapitalized + capitalized).
	// allcaps and end-capitalized are common enough too, underestimate as 2x factor to be safe.
	upper := countRunes(w, isUppercase)
	if upper == utf8.RuneCountInString(w) || upper == 1 && hasCapitalAtAnEnd(w) {
		return 2
	}
	// otherwise calculate the number of ways to capitalize U+L uppercase+lowercase letters
	// with U uppercase letters or less. or, if there's more uppercase than lower (for eg. PASSwORD),
	// the number of ways to lowercase U+L letters with L lowercase letters or less.
	lower := countRunes(w, isLowercase)
	variations := float64(0)
	for i := 1; i <= min(upper, lower); i++ {
		variations += mathutils.NCk(upper+lower, i)
	}
	return variations
}

func hasCapitalAtAnEnd(w string) bool {
	first, _ := utf8.DecodeRuneInString(w)
	last, _ := utf8.DecodeLastRuneInString(w)
	return isUppercase(first) || isUppercase(last)
}

// isLowercase and isUppercase are Rust's char::is_lowercase and
// char::is_uppercase, which include the Other_ properties.
func isLowercase(r rune) bool {
	return unicode.In(r, unicode.Ll, unicode.Other_Lowercase)
}

func isUppercase(r rune) bool {
	return unicode.In(r, unicode.Lu, unicode.Other_Uppercase)
}

func countRunes(s string, keep func(rune) bool) int {
	n := 0
	for _, r := range s {
		if keep(r) {
			n++
		}
	}
	return n
}

func L33tVariations(m *match.Match) float64 {
	if !m.L33t {
		return 1
	}
	// lower-case match.token before calculating: capitalization shouldn't affect l33t calc.
	chrs := strings.ToLower(m.Token)
	variations := float64(1)
	for subbed, unsubbed := range m.Sub {
		s := strings.Count(chrs, string(subbed))
		u := strings.Count(chrs, string(unsubbed))
		if s == 0 || u == 0 {
			// for this sub, password is either fully subbed (444) or fully unsubbed (aaa)
			// treat that as doubling the space (attacker needs to try fully subbed chars in addition to
			// unsubbed.)
			variations *= 2
			continue
		}
		// this case is similar to capitalization:
		// with aa44a, U = 3, S = 2, attacker needs to try unsubbed + one sub + two subs
		possibilities := float64(0)
		for i := 1; i <= min(u, s); i++ {
			possibilities += mathutils.NCk(u+s, i)
		}
		variations *= possibilities
	}
	return variations
}

func SpatialGuesses(m *match.Match) float64 {
	graph := adjacency.Graphs["keypad"]
	if m.Graph == "qwerty" || m.Graph == "dvorak" {
		graph = adjacency.Graphs["qwerty"]
	}
	s := float64(len(graph.Graph))
	d := float64(graph.AverageDegree)
	guesses := float64(0)
	l := utf8.RuneCountInString(m.Token)
	t := m.Turns
	// estimate the number of possible patterns w/ length L or less with t turns or less.
	for i := 2; i <= l; i++ {
		possibleTurns := min(t, i-1)
		for j := 1; j <= possibleTurns; j++ {
			guesses += mathutils.NCk(i-1, j-1) * s * math.Pow(d, float64(j))
		}
	}
	return guesses * shiftedVariations(m.ShiftedCount, l-m.ShiftedCount)
}

// shiftedVariations is the extra factor for shifted keys (% instead of 5, A
// instead of a), computed like l33t substitutions.
func shiftedVariations(shifted, unshifted int) float64 {
	if shifted == 0 {
		return 1
	}
	if unshifted == 0 {
		return 2
	}
	variations := float64(0)
	for i := 1; i <= min(shifted, unshifted); i++ {
		variations += mathutils.NCk(shifted+unshifted, i)
	}
	return variations
}

func RepeatGuesses(m *match.Match) float64 {
	return m.BaseGuesses * float64(m.RepeatCount)
}

func SequenceGuesses(m *match.Match) float64 {
	firstChr, _ := utf8.DecodeRuneInString(m.Token)
	// lower guesses for obvious starting points
	var baseGuesses float64
	switch {
	case strings.ContainsRune("aAzZ019", firstChr):
		baseGuesses = 4
	case firstChr >= '0' && firstChr <= '9':
		baseGuesses = 10
	default:
		// could give a higher base for uppercase,
		// assigning 26 to both upper and lower sequences is more conservative.
		baseGuesses = 26
	}
	if !m.Ascending {
		// need to try a descending sequence in addition to every ascending sequence ->
		// 2x guesses
		baseGuesses *= 2
	}
	return baseGuesses * float64(utf8.RuneCountInString(m.Token))
}

func RegexGuesses(m *match.Match) float64 {
	if m.RegexName != "recent_year" {
		return 0
	}
	// conservative estimate of year space: num years from REFERENCE_YEAR.
	// if year is close to REFERENCE_YEAR, estimate a year space of MIN_YEAR_SPACE.
	year, _ := strconv.Atoi(m.Token)
	return float64(max(mathutils.Abs(year-ReferenceYear), MinYearSpace))
}

const MinYearSpace = 20

var ReferenceYear = time.Now().Year()

func DateGuesses(m *match.Match) float64 {
	// base guesses: (year distance from ReferenceYear) * num_days * num_years
	yearSpace := max(mathutils.Abs(m.Year-ReferenceYear), MinYearSpace)
	guesses := yearSpace * 365
	// add factor of 4 for separator selection (one of ~4 choices)
	if m.Separator != "" {
		guesses *= 4
	}
	return float64(guesses)
}
