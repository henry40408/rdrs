package siphash

import "encoding/binary"

// Sum24x128 returns the 128-bit SipHash-2-4 of data under the given key, as
// the two halves the Rust siphasher crate calls h1 and h2. Generated entry ids
// are stored in the database, so they must match the Rust build.
func Sum24x128(k0, k1 uint64, data []byte) (h1, h2 uint64) {
	s := newState(k0, k1)
	s.v1 ^= 0xee
	length := len(data)
	for len(data) >= 8 {
		s.absorb2(binary.LittleEndian.Uint64(data))
		data = data[8:]
	}
	last := uint64(length&0xff) << 56
	for i, b := range data {
		last |= uint64(b) << (8 * uint(i))
	}
	s.absorb2(last)
	s.v2 ^= 0xee
	for range 4 {
		s.round()
	}
	h1 = s.v0 ^ s.v1 ^ s.v2 ^ s.v3
	s.v1 ^= 0xdd
	for range 4 {
		s.round()
	}
	h2 = s.v0 ^ s.v1 ^ s.v2 ^ s.v3
	return h1, h2
}

func (s *state) absorb2(m uint64) {
	s.v3 ^= m
	s.round()
	s.round() // c = 2
	s.v0 ^= m
}
