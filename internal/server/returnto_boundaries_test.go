package server

import "testing"

func TestEncodeURLPartEncodesEveryByteFromTheFirstNonASCII(t *testing.T) {
	t.Parallel()
	never := func(byte) bool { return false }
	cases := map[string]string{
		"\x7f": "\x7f", // the last ASCII byte is copied when the set leaves it
		"\x80": "%80",  // the first non-ASCII byte is always encoded
		"\xff": "%FF",
		"é":    "%C3%A9",
	}
	for part, want := range cases {
		if got := encodeURLPart(part, never); got != want {
			t.Errorf("encodeURLPart(%q) = %q, want %q", part, got, want)
		}
	}
	always := func(byte) bool { return true }
	if got := encodeURLPart("a", always); got != "%61" {
		t.Errorf("a byte in the set = %q, want %%61", got)
	}
}
