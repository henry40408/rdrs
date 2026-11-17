package testutil

import (
	"bufio"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// minimumMappedTests is the number of Rust tests ported so far; raise it with
// every ported file. The goal is the Rust count (about 615).
const minimumMappedTests = 31

// TestEveryMappedGoTestExists keeps testdata/test_map.txt honest: each Go test
// it names must be declared in its package, and the map may not shrink.
func TestEveryMappedGoTestExists(t *testing.T) {
	root := filepath.Join("..", "..")
	file, err := os.Open(filepath.Join(root, "testdata", "test_map.txt"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()

	mapped := 0
	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		fields := strings.Fields(line)
		pkg, name, ok := strings.Cut(strings.Join(fields[1:], ""), "::")
		if len(fields) != 2 || !ok {
			t.Errorf("malformed line %q", line)
			continue
		}
		mapped++
		if !declaresTest(t, filepath.Join(root, pkg), name) {
			t.Errorf("%s: %s is not declared in %s", fields[0], name, pkg)
		}
	}
	if mapped < minimumMappedTests {
		t.Errorf("only %d Rust tests are mapped, expected at least %d", mapped, minimumMappedTests)
	}
}

func declaresTest(t *testing.T, dir, name string) bool {
	t.Helper()
	declaration := regexp.MustCompile(`(?m)^func ` + regexp.QuoteMeta(name) + `\(`)
	files, err := filepath.Glob(filepath.Join(dir, "*_test.go"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		source, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if declaration.Match(source) {
			return true
		}
	}
	return false
}
