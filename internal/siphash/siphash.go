// Package siphash implements SipHash-1-3 with zero keys, the hash behind
// Rust's std DefaultHasher. Feed buckets are stored in the database, so the Go
// build must map every URL to the same bucket the Rust build chose.
package siphash

import (
	"encoding/binary"
	"math/bits"
)

type state struct{ v0, v1, v2, v3 uint64 }

func newState(k0, k1 uint64) state {
	return state{
		v0: k0 ^ 0x736f6d6570736575,
		v1: k1 ^ 0x646f72616e646f6d,
		v2: k0 ^ 0x6c7967656e657261,
		v3: k1 ^ 0x7465646279746573,
	}
}

func (s *state) round() {
	s.v0 += s.v1
	s.v1 = bits.RotateLeft64(s.v1, 13)
	s.v1 ^= s.v0
	s.v0 = bits.RotateLeft64(s.v0, 32)
	s.v2 += s.v3
	s.v3 = bits.RotateLeft64(s.v3, 16)
	s.v3 ^= s.v2
	s.v0 += s.v3
	s.v3 = bits.RotateLeft64(s.v3, 21)
	s.v3 ^= s.v0
	s.v2 += s.v1
	s.v1 = bits.RotateLeft64(s.v1, 17)
	s.v1 ^= s.v2
	s.v2 = bits.RotateLeft64(s.v2, 32)
}

func (s *state) absorb(m uint64) {
	s.v3 ^= m
	s.round() // c = 1
	s.v0 ^= m
}

// Sum13 returns SipHash-1-3 of data under the zero key.
func Sum13(data []byte) uint64 {
	s := newState(0, 0)
	length := len(data)
	for len(data) >= 8 {
		s.absorb(binary.LittleEndian.Uint64(data))
		data = data[8:]
	}
	last := uint64(length&0xff) << 56
	for i, b := range data {
		last |= uint64(b) << (8 * uint(i))
	}
	s.absorb(last)
	s.v2 ^= 0xff
	for range 3 { // d = 3
		s.round()
	}
	return s.v0 ^ s.v1 ^ s.v2 ^ s.v3
}

// RustStrHash returns what Rust's DefaultHasher yields for hashing a &str:
// the bytes, then a 0xff terminator.
func RustStrHash(text string) uint64 {
	data := make([]byte, 0, len(text)+1)
	data = append(data, text...)
	data = append(data, 0xff)
	return Sum13(data)
}
