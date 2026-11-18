package secret

import "testing"

func TestVerifyTruncatedLeftLengthLimits(t *testing.T) {
	tag := Tag(testSecret, DomainSession, []byte("token"))
	tests := []struct {
		length int
		want   bool
	}{{3, false}, {4, true}, {31, true}, {32, true}, {33, false}}
	for _, tc := range tests {
		candidate := append([]byte(nil), tag[:min(tc.length, len(tag))]...)
		for len(candidate) < tc.length {
			candidate = append(candidate, 0)
		}
		if got := verifyTruncatedLeft(testSecret, DomainSession, candidate, []byte("token")); got != tc.want {
			t.Errorf("length %d: got %v, want %v", tc.length, got, tc.want)
		}
	}
}
