package timezone

import (
	"os"
	"path/filepath"
	"testing"
)

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

func TestAHostWithoutLegacyAliasesDefersToTheEmbeddedDatabase(t *testing.T) {
	host := t.TempDir()
	if err := os.MkdirAll(filepath.Join(host, "Asia"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(host, "Asia", "Taipei"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	original := hostZoneinfoDirs
	hostZoneinfoDirs = []string{host}
	t.Cleanup(func() { hostZoneinfoDirs = original })

	if !hasExactCase("Asia/Calcutta") {
		t.Error("an alias the host lacks was refused")
	}
	if hasExactCase("Asia/taipei") {
		t.Error("a wrong-case spelling was accepted")
	}
}
