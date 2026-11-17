package cache

import (
	"os"
	"testing"
)

// unset removes a variable for the test and restores it afterwards.
func unset(t *testing.T, name string) {
	t.Helper()
	old, had := os.LookupEnv(name)
	if err := os.Unsetenv(name); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if had {
			_ = os.Setenv(name, old)
		}
	})
}
