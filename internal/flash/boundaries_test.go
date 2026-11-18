package flash

import "testing"

func TestEscapeValueBoundaries(t *testing.T) {
	tests := map[string]string{
		"a\x1fb": "a%1Fb", "a b": "a%20b", "a~b": "a~b", "a\x7fb": "a%7Fb", "aé": "a%C3%A9", "AZaz09": "AZaz09",
	}
	for in, want := range tests {
		if got := escapeValue(in); got != want {
			t.Errorf("escapeValue(%q) = %q, want %q", in, got, want)
		}
	}
}
