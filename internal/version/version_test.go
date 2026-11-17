package version

import "testing"

func TestInjectedVersionWins(t *testing.T) {
	t.Cleanup(func() { version = "" })
	version = "v1.2.3"
	if got := Version(); got != "v1.2.3" {
		t.Fatalf("got %q", got)
	}
}

func TestDevIsTreatedAsUnset(t *testing.T) {
	t.Cleanup(func() { version = "" })
	version = "dev"
	if got := Version(); got == "dev" || got == "" {
		t.Fatalf("got %q", got)
	}
}

// A test binary has no module version; neither "" nor "(devel)" may leak out.
func TestEmptyModuleVersionFallsBackToUnknown(t *testing.T) {
	t.Cleanup(func() { version = "" })
	version = ""
	if got := Version(); got != "unknown" {
		t.Fatalf("got %q, want unknown", got)
	}
}

func TestIsDirty(t *testing.T) {
	t.Cleanup(func() { version = "" })
	version = "v1.2.3-dirty"
	if !IsDirty() {
		t.Fatal("want dirty")
	}
	version = "v1.2.3"
	if IsDirty() {
		t.Fatal("want clean")
	}
}
