package zxcvbn

import (
	"slices"
	"testing"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

func TestScoreOfThreeNeedsNoFeedback(t *testing.T) {
	result := Result{Score: minGoodScore, Sequence: []*match.Match{{Pattern: "sequence", Token: "abc"}}}
	if _, ok := FeedbackFor(result); ok {
		t.Error("a good enough score still produced feedback")
	}
	result.Score = minGoodScore - 1
	if _, ok := FeedbackFor(result); !ok {
		t.Error("a weak score produced no feedback")
	}
}

func TestReversedWordSuggestionNeedsFourCharacters(t *testing.T) {
	const reversed = "Reversed words aren't much harder to guess."
	tests := map[string]bool{"drow": true, "dro": false, "drowss": true}
	for token, want := range tests {
		m := &match.Match{Pattern: "dictionary", DictionaryName: "english_wikipedia", Token: token, Reversed: true}
		got := slices.Contains(matchFeedback(m, false).Suggestions, reversed)
		if got != want {
			t.Errorf("token %q: suggestion present = %v, want %v", token, got, want)
		}
	}
}
