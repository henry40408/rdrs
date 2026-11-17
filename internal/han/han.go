// Package han detects Simplified Chinese text, used only to choose a lang
// attribute. Simplified-only means GBK encodes the character and Big5 does not.
package han

import (
	"sync"
	"unicode/utf8"

	"golang.org/x/text/encoding"
	"golang.org/x/text/encoding/simplifiedchinese"
	"golang.org/x/text/encoding/traditionalchinese"
)

// first and last bound the codepoints covered (CJK Ext A through the main
// block); neither GBK nor Big5 maps anything outside it.
const (
	first = 0x3400
	last  = 0x9FFF
	words = (last - first + 1 + 63) / 64
)

// threshold is the Simplified-only characters needed before tagging. One
// misfires on Traditional variants and on Japanese names.
const threshold = 2

// Lead byte each encoding's two-byte sequences start at. Big5 starts at 0xA1
// because "Big5 has it" means the encoder's repertoire, which excludes HKSCS.
const (
	gbkFirstLead  = 0x81
	big5FirstLead = 0xA1
)

// LangSimplified is the lang attribute value for Simplified Chinese text.
const LangSimplified = "zh-Hans"

type bitSet [words]uint64

func (b *bitSet) set(cp rune, present bool) {
	if cp < first || cp > last {
		return
	}
	i := int(cp - first)
	if present {
		b[i/64] |= 1 << (i % 64)
		return
	}
	b[i/64] &^= 1 << (i % 64)
}

func (b *bitSet) has(cp rune) bool {
	if cp < first || cp > last {
		return false
	}
	i := int(cp - first)
	return b[i/64]&(1<<(i%64)) != 0
}

// simplifiedOnly is built by sweeping the decoders: GBK sets, then Big5
// clears what both share, leaving the GBK-only characters.
var simplifiedOnly = sync.OnceValue(func() *bitSet {
	var bits bitSet
	sweep(simplifiedchinese.GBK, gbkFirstLead, func(cp rune) { bits.set(cp, true) })
	sweep(traditionalchinese.Big5, big5FirstLead, func(cp rune) { bits.set(cp, false) })
	return &bits
})

// sweep calls visit for every two-byte sequence that decodes to one character.
func sweep(enc encoding.Encoding, firstLead byte, visit func(rune)) {
	decoder := enc.NewDecoder()
	for lead := int(firstLead); lead <= 0xFE; lead++ {
		for trail := 0x40; trail <= 0xFE; trail++ {
			text, err := decoder.Bytes([]byte{byte(lead), byte(trail)})
			if err != nil {
				continue
			}
			ch, size := utf8.DecodeRune(text)
			// U+FFFD is the decoder's marker for a malformed sequence; some
			// Big5 sequences decode to two characters, which are skipped.
			if ch == utf8.RuneError || size != len(text) {
				continue
			}
			visit(ch)
		}
	}
}

// LangAttr returns "zh-Hans" when text reads as Simplified Chinese, else "".
// It is safe on raw HTML (ASCII cannot match) and stops at the threshold.
func LangAttr(text string) string {
	bits := simplifiedOnly()
	seen := 0
	for _, ch := range text {
		if !bits.has(ch) {
			continue
		}
		seen++
		if seen >= threshold {
			return LangSimplified
		}
	}
	return ""
}
