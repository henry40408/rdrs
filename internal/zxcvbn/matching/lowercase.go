package matching

import (
	"strings"
	"unicode"
)

const (
	capitalDottedI = 'İ'
	capitalSigma   = 'Σ'
	finalSigma     = 'ς'
)

// rustLowercase is Rust's str::to_lowercase, which Rust zxcvbn applies to the
// whole password before slicing it at the original password's character
// offsets. Two of its rules differ from Go's per-rune unicode.ToLower: 'İ'
// becomes "i̇", two runes, which shifts every later offset; and 'Σ' at the
// end of a word becomes 'ς'.
func rustLowercase(runes []rune) []rune {
	lower := make([]rune, 0, len(runes))
	for i, r := range runes {
		switch {
		case r == capitalDottedI:
			lower = append(lower, 'i', '\u0307')
		case r == capitalSigma && isWordFinal(runes, i):
			lower = append(lower, finalSigma)
		default:
			lower = append(lower, unicode.ToLower(r))
		}
	}
	return lower
}

func rustLowercaseString(s string) string {
	return string(rustLowercase([]rune(s)))
}

// isWordFinal is Unicode's Final_Sigma condition: a cased letter comes
// before position i and none comes after it, skipping case-ignorable runes.
func isWordFinal(runes []rune, i int) bool {
	return casedBefore(runes, i) && !casedAfter(runes, i)
}

func casedBefore(runes []rune, i int) bool {
	for k := i - 1; k >= 0; k-- {
		if !isCaseIgnorable(runes[k]) {
			return isCased(runes[k])
		}
	}
	return false
}

func casedAfter(runes []rune, i int) bool {
	for k := i + 1; k < len(runes); k++ {
		if !isCaseIgnorable(runes[k]) {
			return isCased(runes[k])
		}
	}
	return false
}

func isCased(r rune) bool {
	return unicode.In(r, unicode.Lu, unicode.Ll, unicode.Lt, unicode.Other_Lowercase, unicode.Other_Uppercase)
}

// isCaseIgnorable approximates the Case_Ignorable property: Go's unicode
// package has the categories but not the Word_Break punctuation list, of
// which the common members are spelled out.
func isCaseIgnorable(r rune) bool {
	return unicode.In(r, unicode.Mn, unicode.Me, unicode.Cf, unicode.Lm, unicode.Sk) ||
		strings.ContainsRune("'.:\u00b7\u0387\u05f4\u2018\u2019\u2024\u2027\ufe13\ufe52\ufe55\uff07\uff0e\uff1a\u00ad", r)
}
