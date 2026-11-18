package matching

import (
	"slices"
	"sort"
	"strings"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

type l33tMatch struct {
	dm    dictionaryMatch
	table map[rune][]rune
}

func (lm l33tMatch) Matches(password []rune) []*match.Match {
	matches := []*match.Match{}
	for _, sub := range enumerateLeetSubs(relevantSubtable(password, lm.table)) {
		if len(sub) == 0 {
			break
		}
		matches = append(matches, lm.subbedMatches(password, sub)...)
	}
	match.Sort(matches)
	return matches
}

// subbedMatches are the dictionary matches of password with sub applied that
// contain at least one substitution. Unlike upstream, single-character
// matches are kept, as Rust zxcvbn 3.1.1 keeps them.
func (lm l33tMatch) subbedMatches(password []rune, sub map[rune]rune) []*match.Match {
	var matches []*match.Match
	for _, m := range lm.dm.Matches(translate(password, sub)) {
		token := password[m.I : m.J+1]
		if rustLowercaseString(string(token)) == m.MatchedWord {
			continue
		}
		m.Sub = make(map[rune]rune)
		for subbed, chr := range sub {
			if slices.Contains(token, subbed) {
				m.Sub[subbed] = chr
			}
		}
		m.L33t = true
		m.Token = string(token)
		matches = append(matches, m)
	}
	return matches
}

func translate(password []rune, sub map[rune]rune) []rune {
	out := make([]rune, len(password))
	for i, r := range password {
		if v, ok := sub[r]; ok {
			r = v
		}
		out[i] = r
	}
	return out
}

type kv struct {
	k rune
	v rune
}

func dedup(subs [][]kv) [][]kv {
	var res [][]kv
	var b strings.Builder
	members := make(map[string]bool)
	for _, sub := range subs {
		sort.SliceStable(sub, func(i, j int) bool {
			return sub[i].k < sub[j].k
		})
		b.Reset()
		for _, x := range sub {
			b.WriteRune(x.k)
			b.WriteRune(',')
			b.WriteRune(x.v)
		}
		key := b.String()
		if !members[key] {
			res = append(res, sub)
			members[key] = true
		}
	}
	return res
}

// enumerateLeetSubs returns the list of possible 1337 replacement dictionaries for a given password
func enumerateLeetSubs(table map[rune][]rune) []map[rune]rune {
	var keys []rune
	for k := range table {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	subs := [][]kv{{}}
	for _, key := range keys {
		subs = dedup(extendLeetSubs(subs, key, table[key]))
	}
	var subDicts []map[rune]rune
	for _, sub := range subs {
		subDict := make(map[rune]rune)
		for _, x := range sub {
			subDict[x.k] = x.v
		}
		subDicts = append(subDicts, subDict)
	}
	return subDicts
}

// extendLeetSubs adds each substitution of key to every sub. A l33t character
// already used for another letter forks the sub into both choices.
func extendLeetSubs(subs [][]kv, key rune, l33tChrs []rune) [][]kv {
	var nextSubs [][]kv
	for _, l33tChr := range l33tChrs {
		for _, sub := range subs {
			dup := slices.IndexFunc(sub, func(x kv) bool { return x.k == l33tChr })
			if dup == -1 {
				nextSubs = append(nextSubs, append(slices.Clone(sub), kv{k: l33tChr, v: key}))
				continue
			}
			alternative := slices.Delete(slices.Clone(sub), dup, dup+1)
			alternative = append(alternative, kv{k: l33tChr, v: key})
			nextSubs = append(nextSubs, sub, alternative)
		}
	}
	return nextSubs
}

func relevantSubtable(password []rune, table map[rune][]rune) map[rune][]rune {
	relevantSubs := make(map[rune][]rune)
	for key, values := range table {
		for _, value := range values {
			if slices.Contains(password, value) {
				relevantSubs[key] = append(relevantSubs[key], value)
			}
		}
	}
	return relevantSubs
}
