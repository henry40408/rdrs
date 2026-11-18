package search

import "testing"

func TestScanDigitsLimitsAndRange(t *testing.T) {
	value, rest, ok := scanDigits("12345", 2)
	if !ok || value != 12 || rest != "345" {
		t.Errorf("limited: %d %q %v", value, rest, ok)
	}
	value, rest, ok = scanDigits("09x", -1)
	if !ok || value != 9 || rest != "x" {
		t.Errorf("full range: %d %q %v", value, rest, ok)
	}
	if _, _, ok = scanDigits("/:", -1); ok {
		t.Error("characters next to the digits were read as digits")
	}
}

func TestAsciiLowerCoversTheFullUppercaseRange(t *testing.T) {
	if got := asciiLower("AZaz@[É"); got != "azaz@[É" {
		t.Errorf("asciiLower = %q", got)
	}
}
