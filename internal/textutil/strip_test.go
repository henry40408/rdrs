package textutil

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
)

func TestStripVectors(t *testing.T) {
	data, err := os.ReadFile("../../testdata/vectors/text.json")
	if err != nil {
		t.Fatal(err)
	}
	var vectors struct {
		Rows []struct {
			Input  string `json:"input"`
			Plain  string `json:"plain"`
			Search string `json:"search"`
		} `json:"rows"`
	}
	if err := json.Unmarshal(data, &vectors); err != nil {
		t.Fatal(err)
	}
	if len(vectors.Rows) == 0 {
		t.Fatal("no vectors")
	}
	for _, row := range vectors.Rows {
		if got := StripToPlainText(row.Input); got != row.Plain {
			t.Errorf("plain(%q) = %q, want %q", row.Input, got, row.Plain)
		}
		if got := StripToSearchText(row.Input); got != row.Search {
			t.Errorf("search(%q) = %q, want %q", row.Input, got, row.Search)
		}
	}
}

// The cases below are the unit tests of src/utils/text.rs.
func TestStripRustCases(t *testing.T) {
	html := "a<SCRIPT type=x>var a='</b>';</ScRiPt>b<Style>p{}</STYLE>c<!-- <b>x</b> -->d<B>e</B>f"
	cases := []struct{ input, plain, search string }{
		{"超<b>少女</b>與機器人", "超 少女 與機器人", "超少女與機器人"},
		{`<a href="https://x/超少女">hi</a><script>var superheroine=1</script>`, "hi", "hi"},
		{html, "a b c d e f", "abcdef"},
		{"a<script>b<b>c", "a", "a"},
		{"a<style>b", "a", "a"},
		{"a<!-- b", "a", "a"},
		{"a<scrip>b</scrip>", "a b", "ab"},
		{"a b", "a b", "a b"},
	}
	for _, tc := range cases {
		if got := StripToPlainText(tc.input); got != tc.plain {
			t.Errorf("plain(%q) = %q, want %q", tc.input, got, tc.plain)
		}
		if got := StripToSearchText(tc.input); got != tc.search {
			t.Errorf("search(%q) = %q, want %q", tc.input, got, tc.search)
		}
	}
}

func TestStripIsLinear(t *testing.T) {
	// A quadratic scanner would not finish this input in reasonable time.
	input := strings.Repeat("<script>x</script>y", 200_000)
	if got := StripToSearchText(input); len(got) != 200_000 {
		t.Errorf("len = %d", len(got))
	}
}
