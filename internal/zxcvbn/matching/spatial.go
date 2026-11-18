package matching

import (
	"strings"

	"github.com/henry40408/rdrs/internal/zxcvbn/adjacency"
	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

type spatialMatch struct {
	graphs []*adjacency.Graph
}

func (s spatialMatch) Matches(password []rune) (matches []*match.Match) {
	for _, graph := range s.graphs {
		if graph.Graph != nil {
			matches = append(matches, spatialMatchHelper(password, graph)...)
		}
	}
	match.Sort(matches)
	return matches
}

// shiftedChars is the Rust zxcvbn 3.1.1 set, which also counts '[' and ']'.
// Only the keyboard graphs have shifted keys.
var shiftedChars = map[string]string{
	"qwerty": `[~!@#$%^&*()_+QWERTYUIOP{}|ASDFGHJKL:"ZXCVBNM<>?]`,
	"dvorak": `[~!@#$%^&*()_+QWERTYUIOP{}|ASDFGHJKL:"ZXCVBNM<>?]`,
}

func spatialMatchHelper(password []rune, graph *adjacency.Graph) (matches []*match.Match) {
	shifted := shiftedChars[graph.Name]
	i := 0
	for i < len(password)-1 {
		j := i + 1
		lastDirection := -1
		turns := 0
		shiftedCount := 0
		if strings.ContainsRune(shifted, password[i]) {
			shiftedCount = 1
		}
		for j < len(password) {
			direction, isShifted, found := adjacentDirection(graph, password[j-1], password[j])
			if !found {
				break
			}
			if isShifted {
				shiftedCount++
			}
			// Every pattern starts with a turn, hence lastDirection starts
			// at an impossible -1.
			if direction != lastDirection {
				turns++
				lastDirection = direction
			}
			j++
		}
		// Chains of one or two keys are not patterns.
		if j-i > 2 {
			matches = append(matches, &match.Match{
				Pattern:      "spatial",
				I:            i,
				J:            j - 1,
				Token:        string(password[i:j]),
				Graph:        graph.Name,
				Turns:        turns,
				ShiftedCount: shiftedCount,
			})
		}
		i = j
	}
	return matches
}

// adjacentDirection finds cur among prev's neighbours. Position 1 in a
// neighbour entry is the shifted key: 'q' is next to "2@", and '@' is shifted.
func adjacentDirection(graph *adjacency.Graph, prev, cur rune) (direction int, shifted, found bool) {
	for direction, adj := range graph.Graph[string(prev)] {
		if idx := strings.IndexRune(adj, cur); idx != -1 {
			return direction, idx == 1, true
		}
	}
	return 0, false, false
}
