package datetime

import "testing"

func TestOffsetBoundaries(t *testing.T) {
	if got := NormalizeTimezoneFormat("+08:00"); got != "+0800" {
		t.Errorf("a bare offset: got %q", got)
	}
	if got := offsetSeconds("+", "01", "30"); got != 5400 {
		t.Errorf("offsetSeconds = %d", got)
	}
	if got := offsetSeconds("-", "00", "01"); got != -60 {
		t.Errorf("offsetSeconds = %d", got)
	}
	for _, offset := range []string{"+2400", "-2400"} {
		if _, ok := parseChinese("週二, 6 一月 2026 14:28:00 " + offset); ok {
			t.Errorf("a whole-day offset %s was accepted", offset)
		}
	}
	for _, text := range []string{"2024-03-06 12:00:00 +2300", "2024-03-06 12:00:00 +0059"} {
		if _, ok := ParseTimestamp(text); !ok {
			t.Errorf("%q was rejected", text)
		}
	}
}
