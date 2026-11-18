package zxcvbn

import (
	"unicode"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

// Feedback is the advice Rust zxcvbn 3.1.1 attaches to a weak password:
// Warning explains what is wrong (empty when there is none) and Suggestions
// say how to improve it. The texts are the Rust crate's, because rdrs shows
// them to users verbatim.
type Feedback struct {
	Warning     string
	Suggestions []string
}

const (
	suggestAddWord       = "Add another word or two. Uncommon words are better."
	suggestUseFewWords   = "Use a few words, avoid common phrases."
	suggestNoNeedSymbols = "No need for symbols, digits, or uppercase letters."
)

// FeedbackFor mirrors Rust's get_feedback. The second result is false where
// Rust returns None (score 3 or better).
func FeedbackFor(result Result) (Feedback, bool) {
	if len(result.Sequence) == 0 {
		return Feedback{Suggestions: []string{suggestUseFewWords, suggestNoNeedSymbols}}, true
	}
	if result.Score >= minGoodScore {
		return Feedback{}, false
	}

	// Rust's max_by_key keeps the last of equally long tokens.
	longest := result.Sequence[0]
	for _, m := range result.Sequence[1:] {
		if utf8.RuneCountInString(m.Token) >= utf8.RuneCountInString(longest.Token) {
			longest = m
		}
	}
	feedback := matchFeedback(longest, len(result.Sequence) == 1)
	feedback.Suggestions = append([]string{suggestAddWord}, feedback.Suggestions...)
	return feedback, true
}

const minGoodScore = 3

func matchFeedback(m *match.Match, soleMatch bool) Feedback {
	switch m.Pattern {
	case "dictionary":
		return dictionaryFeedback(m, soleMatch)
	case "spatial":
		warning := "Short keyboard patterns are easy to guess."
		if m.Turns == 1 {
			warning = "Straight rows of keys are easy to guess."
		}
		return Feedback{Warning: warning, Suggestions: []string{"Use a longer keyboard pattern with more turns."}}
	case "repeat":
		warning := "Repeats like \"abcabcabc\" are only slightly harder to guess than \"abc\"."
		if utf8.RuneCountInString(m.BaseToken) == 1 {
			warning = "Repeats like \"aaa\" are easy to guess."
		}
		return Feedback{Warning: warning, Suggestions: []string{"Avoid repeated words and characters."}}
	case "sequence":
		return Feedback{
			Warning:     "Sequences like abc or 6543 are easy to guess.",
			Suggestions: []string{"Avoid sequences."},
		}
	case "regex":
		if m.RegexName != "recent_year" {
			return Feedback{}
		}
		return Feedback{
			Warning:     "Recent years are easy to guess.",
			Suggestions: []string{"Avoid recent years.", "Avoid years that are associated with you."},
		}
	case "date":
		return Feedback{
			Warning:     "Dates are often easy to guess.",
			Suggestions: []string{"Avoid dates and years that are associated with you."},
		}
	}
	return Feedback{}
}

func dictionaryFeedback(m *match.Match, soleMatch bool) Feedback {
	var warning string
	switch m.DictionaryName {
	case "passwords":
		warning = passwordWarning(m, soleMatch)
	case "english_wikipedia":
		if soleMatch {
			warning = "A word by itself is easy to guess."
		}
	case "surnames", "female_names", "male_names":
		warning = "Common names and surnames are easy to guess."
		if soleMatch {
			warning = "Names and surnames by themselves are easy to guess."
		}
	}

	if m.Token == "" {
		return Feedback{}
	}
	var suggestions []string
	first, _ := utf8.DecodeRuneInString(m.Token)
	switch {
	case unicode.IsUpper(first):
		suggestions = append(suggestions, "Capitalization doesn't help very much.")
	case allUpper(m.Token):
		suggestions = append(suggestions, "All-uppercase is almost as easy to guess as all-lowercase.")
	}
	if m.Reversed && utf8.RuneCountInString(m.Token) >= 4 {
		suggestions = append(suggestions, "Reversed words aren't much harder to guess.")
	}
	if m.L33t {
		suggestions = append(suggestions, "Predictable substitutions like '@' instead of 'a' don't help very much.")
	}
	return Feedback{Warning: warning, Suggestions: suggestions}
}

func passwordWarning(m *match.Match, soleMatch bool) string {
	switch {
	case !soleMatch || m.L33t || m.Reversed:
		return "This is similar to a commonly used password."
	case m.Rank <= 10:
		return "This is a top-10 common password."
	case m.Rank <= 100:
		return "This is a top-100 common password."
	}
	return "This is a very common password."
}

func allUpper(s string) bool {
	for _, r := range s {
		if !unicode.IsUpper(r) {
			return false
		}
	}
	return true
}
