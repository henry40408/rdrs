package textutil

import (
	"strings"
	"testing"
)

var rustStripVectors = []struct{ input, plain, search string }{
	{"plain", "plain", "plain"},
	{"<p>Hello <b>world</b></p>", "Hello world", "Hello world"},
	{"a &amp; b &lt;c&gt; &#39;q&#39; &#x27;h&#x27; &nbsp; &bogus; &amp;#x27;", "a &amp; b &lt;c&gt; &#39;q&#39; &#x27;h&#x27; &nbsp; &bogus; &amp;#x27;", "a &amp; b &lt;c&gt; &#39;q&#39; &#x27;h&#x27; &nbsp; &bogus; &amp;#x27;"},
	{"<script>x()</script><style>p{}</style>text", "text", "text"},
	{"line1<br>line2<p>para</p>", "line1 line2 para", "line1line2para"},
	{"  spaced \n\n out\t text  ", "spaced out text", "spaced out text"},
	{"日本語<em>テスト</em>", "日本語 テスト", "日本語テスト"},
}

func TestStripVectors(t *testing.T) {
	for _, row := range rustStripVectors {
		if got := StripToPlainText(row.input); got != row.plain {
			t.Errorf("plain(%q) = %q, want %q", row.input, got, row.plain)
		}
		if got := StripToSearchText(row.input); got != row.search {
			t.Errorf("search(%q) = %q, want %q", row.input, got, row.search)
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
