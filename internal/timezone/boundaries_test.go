package timezone

import "testing"

func TestHasLowercaseSegmentBoundaries(t *testing.T) {
	tests := map[string]bool{
		"asia/Taipei": true, "Asia/taipei": true, "a": true, "z": true, "Zulu": false, "Asia/Taipei": false,
		"`x": false, "{x": false, "A/b": true, "": false, "9/X": false,
	}
	for name, want := range tests {
		if got := hasLowercaseSegment(name); got != want {
			t.Errorf("hasLowercaseSegment(%q) = %v, want %v", name, got, want)
		}
	}
}
