package matching

import (
	"github.com/henry40408/rdrs/internal/zxcvbn/adjacency"
	"github.com/henry40408/rdrs/internal/zxcvbn/frequency"
	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

// Omnimatch returns every pattern match in password, sorted by position.
func Omnimatch(password []rune, userInputs []string) []*match.Match {
	return newOmnimatcher(userInputs).matches(password)
}

type omnimatcher struct {
	matchers []match.Matcher
}

func newOmnimatcher(userInputs []string) *omnimatcher {
	dictMatcher := defaultRankedDictionnaries.withDict("user_inputs", buildRankedDict(userInputs))
	o := &omnimatcher{}
	o.matchers = []match.Matcher{
		dictMatcher,
		reverseDictionnaryMatch{dm: dictMatcher},
		l33tMatch{dm: dictMatcher, table: l33tTable},
		spatialMatch{graphs: defaultGraphs},
		// The repeated block is scored with the same dictionaries, user
		// inputs included, as Rust zxcvbn does.
		repeatMatch{omni: o},
		sequenceMatch{},
		regexpMatch{},
		dateMatch{},
	}
	return o
}

func (o *omnimatcher) matches(password []rune) []*match.Match {
	var matches []*match.Match
	for _, m := range o.matchers {
		matches = append(matches, m.Matches(password)...)
	}
	match.Sort(matches)
	return matches
}

var (
	defaultRankedDictionnaries = loadDefaultDictionnaries()
	defaultGraphs              = loadDefaultAdjacencyGraphs()
	l33tTable                  = map[rune][]rune{
		'a': {'4', '@'},
		'b': {'8'},
		'c': {'(', '{', '[', '<'},
		'e': {'3'},
		'g': {'6', '9'},
		'i': {'1', '!', '|'},
		'l': {'1', '|', '7'},
		'o': {'0'},
		's': {'$', '5'},
		't': {'+', '7'},
		'x': {'%'},
		'z': {'2'},
	}
)

func loadDefaultDictionnaries() dictionaryMatch {
	dm := dictionaryMatch{rankedDictionaries: make(map[string]rankedDictionnary)}
	for name, list := range frequency.FrequencyLists {
		dm = dm.withDict(name, buildRankedDict(list))
	}
	return dm
}

func loadDefaultAdjacencyGraphs() []*adjacency.Graph {
	return []*adjacency.Graph{
		adjacency.Graphs["qwerty"],
		adjacency.Graphs["dvorak"],
		adjacency.Graphs["keypad"],
		adjacency.Graphs["mac_keypad"],
	}
}
