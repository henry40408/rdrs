package matching

import (
	"strconv"
	"strings"
	"unicode"

	"github.com/henry40408/rdrs/internal/zxcvbn/internal/mathutils"
	"github.com/henry40408/rdrs/internal/zxcvbn/match"
	"github.com/henry40408/rdrs/internal/zxcvbn/scoring"
)

const dateMaxYear = 2050
const dateMinYear = 1000

var dateSplits = map[int][]struct{ k, l int }{
	4: { // for length-4 strings, eg 1191 or 9111, two ways to split:
		{1, 2}, // 1 1 91 (2nd split starts at index 1, 3rd at index 2)
		{2, 3}, // 91 1 1
	},
	5: {
		{1, 3}, // 1 11 91
		{2, 3}, // 11 1 91
	},
	6: {
		{1, 2}, // 1 1 1991
		{2, 4}, // 11 11 91
		{4, 5}, // 1991 1 1
	},
	7: {
		{1, 3}, // 1 11 1991
		{2, 3}, // 11 1 1991
		{4, 5}, // 1991 1 11
		{4, 6}, // 1991 11 1
	},
	8: {
		{2, 4}, // 11 11 1991
		{4, 6}, // 1991 11 11
	},
}

// a "date" is recognized as:
//   any 3-tuple that starts or ends with a 2- or 4-digit year,
//   with 2 or 0 separator chars (1.1.91 or 1191),
//   maybe zero-padded (01-01-91 vs 1-1-91),
//   a month between 1 and 12,
//   a day between 1 and 31.
//
// note: this isn't true date parsing in that "feb 31st" is allowed,
// this doesn't check for leap years, etc.
//
// every substring of the password is tried, which is slower than one regex
// over the whole password but finds every possible date.

type dateMatchCandidate struct {
	Day   int
	Month int
	Year  int
}

type dateMatch struct{}

func (dm dateMatch) Matches(password []rune) []*match.Match {
	matches := append(datesWithoutSeparator(password), datesWithSeparator(password)...)
	filtered := withoutSubmatches(matches)
	match.Sort(filtered)
	return filtered
}

// datesWithoutSeparator looks at every substring of 4 ('1191') to 8
// ('11111991') digits.
func datesWithoutSeparator(password []rune) []*match.Match {
	var matches []*match.Match
	for i := 0; i+4 <= len(password); i++ {
		for j := i + 3; j <= i+7 && j < len(password); j++ {
			token := password[i : j+1]
			if !allDigits(token) {
				continue
			}
			best, ok := bestDateSplit(string(token))
			if !ok {
				continue
			}
			matches = append(matches, &match.Match{
				Pattern: "date",
				Token:   string(token),
				I:       i,
				J:       j,
				Year:    best.Year,
				Month:   best.Month,
				Day:     best.Day,
			})
		}
	}
	return matches
}

// bestDateSplit prefers, among the ways to read token as a date, the year
// closest to the reference year: '111504' is 11-15-04, not 1-1-1504.
func bestDateSplit(token string) (*dateMatchCandidate, bool) {
	var best *dateMatchCandidate
	for _, s := range dateSplits[len(token)] {
		candidate := mapIntsToDMY(token[0:s.k], token[s.k:s.l], token[s.l:])
		if candidate == nil {
			continue
		}
		if best == nil || dateMatchMetric(candidate) < dateMatchMetric(best) {
			best = candidate
		}
	}
	return best, best != nil
}

// datesWithSeparator looks at every substring of 6 ('1/1/91') to 10
// ('11/11/1991') characters.
func datesWithSeparator(password []rune) []*match.Match {
	var matches []*match.Match
	for i := 0; i+6 <= len(password); i++ {
		for j := i + 5; j <= i+9 && j < len(password); j++ {
			token := password[i : j+1]
			parts, ok := splitSeparatedDate(token)
			if !ok {
				continue
			}
			dmy := mapIntsToDMY(parts.first, parts.second, parts.third)
			if dmy == nil {
				continue
			}
			matches = append(matches, &match.Match{
				Pattern:   "date",
				Token:     string(token),
				I:         i,
				J:         j,
				Separator: string(parts.separator),
				Year:      dmy.Year,
				Month:     dmy.Month,
				Day:       dmy.Day,
			})
		}
	}
	return matches
}

type separatedDate struct {
	first, second, third string
	separator            rune
}

// splitSeparatedDate stands in for the upstream regex
// ^(\d{1,4})([\s/\\_.-])(\d{1,2})\2(\d{1,4})$, taking \s as Unicode
// whitespace like Rust's regex crate does.
func splitSeparatedDate(token []rune) (separatedDate, bool) {
	first, rest, ok := leadingDigits(token, 4)
	if !ok || len(rest) == 0 || !isDateSeparator(rest[0]) {
		return separatedDate{}, false
	}
	separator := rest[0]
	second, rest, ok := leadingDigits(rest[1:], 2)
	if !ok || len(rest) == 0 || rest[0] != separator {
		return separatedDate{}, false
	}
	third, rest, ok := leadingDigits(rest[1:], 4)
	if !ok || len(rest) != 0 {
		return separatedDate{}, false
	}
	return separatedDate{first: first, second: second, third: third, separator: separator}, true
}

// leadingDigits splits off the leading ASCII digits, of which there must be
// 1 to limit.
func leadingDigits(runes []rune, limit int) (string, []rune, bool) {
	n := 0
	for n < len(runes) && isDigit(runes[n]) {
		n++
	}
	if n == 0 || n > limit {
		return "", nil, false
	}
	return string(runes[:n]), runes[n:], true
}

func isDateSeparator(r rune) bool {
	return unicode.IsSpace(r) || strings.ContainsRune(`/\_.-`, r)
}

func isDigit(r rune) bool {
	return r >= '0' && r <= '9'
}

func allDigits(runes []rune) bool {
	for _, r := range runes {
		if !isDigit(r) {
			return false
		}
	}
	return true
}

// withoutSubmatches drops dates inside other dates: '2015_06_04' also
// contains 15_06_04, 5_06_04 and even 2015 (read as 5/1/2020).
func withoutSubmatches(matches []*match.Match) []*match.Match {
	var filtered []*match.Match
	for _, m := range matches {
		if !isSubmatch(m, matches) {
			filtered = append(filtered, m)
		}
	}
	return filtered
}

func isSubmatch(m *match.Match, matches []*match.Match) bool {
	for _, o := range matches {
		if m != o && o.I <= m.I && o.J >= m.J {
			return true
		}
	}
	return false
}

func dateMatchMetric(c *dateMatchCandidate) int {
	return mathutils.Abs(c.Year - scoring.ReferenceYear)
}

func mapIntsToDMY(s1, s2, s3 string) *dateMatchCandidate {
	// given a 3-tuple, discard if:
	//   middle int is over 31 (for all dmy formats, years are never allowed in the middle)
	//   middle int is zero
	//   any int is over the max allowable year
	//   any int is over two digits but under the min allowable year
	//   2 ints are over 31, the max allowable day
	//   2 ints are zero
	//   all ints are over 12, the max allowable month
	i1, _ := strconv.Atoi(s1)
	i2, _ := strconv.Atoi(s2)
	i3, _ := strconv.Atoi(s3)
	if i2 > 31 || i2 <= 0 {
		return nil
	}
	over12 := 0
	over31 := 0
	under1 := 0
	for _, i := range [3]int{i1, i2, i3} {
		if (i > 99 && i < dateMinYear) || i > dateMaxYear {
			return nil
		}
		if i > 31 {
			over31++
		}
		if i > 12 {
			over12++
		}
		if i <= 0 {
			under1++
		}
	}
	if over31 >= 2 || over12 == 3 || under1 >= 2 {
		return nil
	}

	// first look for a four digit year: yyyy + daymonth or daymonth + yyyy
	possibleYearSplits := [][3]int{
		{i3, i1, i2}, // year last
		{i1, i2, i3}, // year first
	}
	for _, split := range possibleYearSplits {
		y := split[0]
		if dateMinYear <= y && y <= dateMaxYear {
			// for a candidate that includes a four-digit year,
			// when the remaining ints don't match to a day and month,
			// it is not a date.
			return mapIntsToDM(split[1], split[2], y)
		}
	}

	// given no four-digit year, two digit years are the most flexible int to match, so
	// try to parse a day-month out of ints[0..1] or ints[1..0]
	for _, split := range possibleYearSplits {
		y := split[0]
		dm := mapIntsToDM(split[1], split[2], y)
		if dm != nil {
			dm.Year = twoToFourDigitYear(dm.Year)
			return dm
		}
	}
	return nil
}

// mapIntsToDM requires a day of 1-31 and a month of 1-12; upstream accepted
// zero for either, Rust zxcvbn 3.1.1 does not.
func mapIntsToDM(i1, i2 int, year int) *dateMatchCandidate {
	if validDay(i1) && validMonth(i2) {
		return &dateMatchCandidate{Day: i1, Month: i2, Year: year}
	}
	if validDay(i2) && validMonth(i1) {
		return &dateMatchCandidate{Day: i2, Month: i1, Year: year}
	}
	return nil
}

func validDay(d int) bool {
	return d >= 1 && d <= 31
}

func validMonth(m int) bool {
	return m >= 1 && m <= 12
}

func twoToFourDigitYear(year int) int {
	if year > 99 {
		return year
	} else if year > 50 {
		// 87 -> 1987
		return year + 1900
	} else {
		// 15 -> 2015
		return year + 2000
	}
}
