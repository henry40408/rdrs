package datetime

import "testing"

func TestSixDigitNumberIsNotADate(t *testing.T) {
	for _, s := range []string{"171113", "010203"} {
		if _, ok := TryParse(s); ok {
			t.Errorf("TryParse(%q) succeeded", s)
		}
	}
}
