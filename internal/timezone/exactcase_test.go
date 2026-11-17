package timezone

import "testing"

func TestWrongCaseIsRefused(t *testing.T) {
	for _, name := range []string{"ASIA/TAIPEI", "Asia/TaiPei", "asia/taipei", "utc"} {
		if _, ok := Parse(name); ok {
			t.Errorf("Parse(%q) succeeded", name)
		}
	}
	for _, name := range []string{"Asia/Taipei", "America/Port-au-Prince", "America/Argentina/ComodRivadavia", "UTC"} {
		if _, ok := Parse(name); !ok {
			t.Errorf("Parse(%q) failed", name)
		}
	}
}
