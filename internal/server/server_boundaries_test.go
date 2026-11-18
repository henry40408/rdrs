package server

import "testing"

func TestPatternConvertsACatchAllAtTheStart(t *testing.T) {
	t.Parallel()
	route := Route{Method: "GET", Path: "{*rest}"}
	if got := route.pattern(); got != "GET {rest...}" {
		t.Errorf("pattern = %q", got)
	}
}

func TestPatternLeavesAPathWithoutACatchAllAlone(t *testing.T) {
	t.Parallel()
	for path, want := range map[string]string{
		"/feeds/{id}": "POST /feeds/{id}",
		"/":           "POST /{$}",
	} {
		if got := (Route{Method: "POST", Path: path}).pattern(); got != want {
			t.Errorf("%q = %q, want %q", path, got, want)
		}
	}
}
