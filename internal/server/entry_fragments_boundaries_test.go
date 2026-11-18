package server

import "testing"

func TestIsScopedEntriesPath(t *testing.T) {
	t.Parallel()
	cases := map[string]bool{
		"/feeds/1/entries":          true,
		"/feeds/0/entries":          true,
		"/feeds/9/entries":          true,
		"/feeds/1234567890/entries": true,
		"/feeds//entries":           false,
		"/feeds/a/entries":          false,
		"/feeds/12a/entries":        false,
		"/feeds/a12/entries":        false,
		"/feeds/-1/entries":         false,
		"/feeds/1.5/entries":        false,
		"/feeds/:/entries":          false, // the byte after '9'
		"/feeds//{}/entries":        false,
		"/feeds/1":                  false,
		"/feeds/1/entries/":         false,
		"/feeds/1/entries/2":        false,
		"/categories/1/entries":     false,
		"feeds/1/entries":           false,
		"":                          false,
	}
	for path, want := range cases {
		if got := isScopedEntriesPath(path, "feeds"); got != want {
			t.Errorf("%q = %v, want %v", path, got, want)
		}
	}
	// '/' is the byte before '0'.
	if isScopedEntriesPath("/feeds/1/2/entries", "feeds") {
		t.Error("a nested id was accepted")
	}
	if !isScopedEntriesPath("/categories/7/entries", "categories") {
		t.Error("the kind is not honoured")
	}
}
