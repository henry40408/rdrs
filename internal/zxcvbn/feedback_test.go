package zxcvbn

import (
	"slices"
	"testing"
)

func TestFeedbackWarnings(t *testing.T) {
	tests := []struct {
		password    string
		warning     string
		suggestions []string
	}{
		{"password", "This is a top-10 common password.", []string{suggestAddWord}},
		{"test", "This is a top-100 common password.", []string{suggestAddWord}},
		{"p4ssw0rd", "This is similar to a commonly used password.",
			[]string{suggestAddWord, "Predictable substitutions like '@' instead of 'a' don't help very much."}},
		{"aaaaaaaa", "Repeats like \"aaa\" are easy to guess.", []string{suggestAddWord, "Avoid repeated words and characters."}},
		{"abcdefgh", "Sequences like abc or 6543 are easy to guess.", []string{suggestAddWord, "Avoid sequences."}},
		{"1998", "Recent years are easy to guess.",
			[]string{suggestAddWord, "Avoid recent years.", "Avoid years that are associated with you."}},
		{"PASSWORD", "This is a top-10 common password.",
			[]string{suggestAddWord, "Capitalization doesn't help very much."}},
		{"Password", "This is a top-10 common password.", []string{suggestAddWord, "Capitalization doesn't help very much."}},
		{"drowssap", "This is similar to a commonly used password.",
			[]string{suggestAddWord, "Reversed words aren't much harder to guess."}},
		{"wikipedia", "A word by itself is easy to guess.", []string{suggestAddWord}},
		{"smith", "Names and surnames by themselves are easy to guess.", []string{suggestAddWord}},
	}
	for _, tc := range tests {
		feedback, ok := FeedbackFor(PasswordStrength(tc.password, nil))
		if !ok {
			t.Errorf("%q: no feedback", tc.password)
			continue
		}
		if feedback.Warning != tc.warning || !slices.Equal(feedback.Suggestions, tc.suggestions) {
			t.Errorf("%q: got %+v, want %q %q", tc.password, feedback, tc.warning, tc.suggestions)
		}
	}
}

func TestFeedbackIsAbsentForGoodScores(t *testing.T) {
	if _, ok := FeedbackFor(PasswordStrength("Rr7!kQ2#vLp9@xZw4$", nil)); ok {
		t.Fatal("a strong password must not carry feedback")
	}
}

func TestFeedbackForEmptySequenceIsTheDefault(t *testing.T) {
	feedback, ok := FeedbackFor(Result{})
	if !ok || feedback.Warning != "" || len(feedback.Suggestions) != 2 {
		t.Fatalf("got %+v %v", feedback, ok)
	}
}
