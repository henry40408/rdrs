package matching

import (
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

type dictionaryMatch struct {
	rankedDictionaries map[string]rankedDictionnary
	// maxWordLen skips substrings no dictionary could contain.
	maxWordLen int
}

func (dm dictionaryMatch) Matches(password []rune) []*match.Match {
	var results []*match.Match
	// lower may be longer than password; slicing it at password's offsets
	// is what Rust zxcvbn does.
	lower := rustLowercase(password)
	for i := range password {
		last := min(len(password), i+dm.maxWordLen) - 1
		for j := i; j <= last; j++ {
			results = append(results, dm.lookup(password, string(lower[i:j+1]), i, j)...)
		}
	}
	match.Sort(results)
	return results
}

func (dm dictionaryMatch) lookup(password []rune, word string, i, j int) []*match.Match {
	var results []*match.Match
	for dictionaryName, rankedDict := range dm.rankedDictionaries {
		rank, ok := rankedDict[word]
		if !ok {
			continue
		}
		results = append(results, &match.Match{
			Pattern:        "dictionary",
			I:              i,
			J:              j,
			Token:          string(password[i : j+1]),
			MatchedWord:    word,
			Rank:           rank,
			DictionaryName: dictionaryName,
		})
	}
	return results
}

func (dm dictionaryMatch) withDict(name string, d rankedDictionnary) dictionaryMatch {
	rd2 := make(map[string]rankedDictionnary, len(dm.rankedDictionaries)+1)
	for k, v := range dm.rankedDictionaries {
		rd2[k] = v
	}
	rd2[name] = d
	return dictionaryMatch{rankedDictionaries: rd2, maxWordLen: max(dm.maxWordLen, d.maxWordLen())}
}

type rankedDictionnary map[string]int

func buildRankedDict(unrankedList []string) rankedDictionnary {
	result := make(rankedDictionnary)
	for i, v := range unrankedList {
		result[rustLowercaseString(v)] = i + 1
	}
	return result
}

func (d rankedDictionnary) maxWordLen() int {
	longest := 0
	for word := range d {
		longest = max(longest, utf8.RuneCountInString(word))
	}
	return longest
}
