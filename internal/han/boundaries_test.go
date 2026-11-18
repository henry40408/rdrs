package han

import "testing"

func TestBitSetCoversTheWholeRange(t *testing.T) {
	var bits bitSet
	for _, cp := range []rune{first, last} {
		bits.set(cp, true)
		if !bits.has(cp) {
			t.Errorf("U+%04X was not stored", cp)
		}
		bits.set(cp, false)
		if bits.has(cp) {
			t.Errorf("U+%04X was not cleared", cp)
		}
	}
	bits.set(first-1, true)
	bits.set(last+1, true)
	if bits.has(first-1) || bits.has(last+1) {
		t.Error("a code point outside the range was stored")
	}
}
