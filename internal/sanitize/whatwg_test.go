package sanitize

import (
	"strings"
	"testing"
)

// The expectations follow the URL standard as implemented by url 2.5.8: its
// percent-encode sets, default port removal and empty fragment handling.
func TestResolveLinkMatchesTheURLCrate(t *testing.T) {
	const base = "https://b.example/dir/page?x=1#top"
	tests := []struct {
		name, raw, base, want string
	}{
		{"pipe and caret stay in path", "http://h.example/a|b^c", "", "http://h.example/a|b^c"},
		{"path encodes braces and space", "http://h.example/a b{c}", "", "http://h.example/a%20b%7Bc%7D"},
		{"query encodes space and quotes", `http://h.example/?q=a b"c'd`, "", "http://h.example/?q=a%20b%22c%27d"},
		{"query encodes non-ascii", "http://h.example/?q=中", "", "http://h.example/?q=%E4%B8%AD"},
		{"path encodes non-ascii", "http://h.example/中", "", "http://h.example/%E4%B8%AD"},
		{"fragment keeps hash", "http://h.example/a#b#c", "", "http://h.example/a#b#c"},
		{"empty fragment is kept", "http://h.example/a#", "", "http://h.example/a#"},
		{"empty query is kept", "http://h.example/a?", "", "http://h.example/a?"},
		{"fragment encodes space", "http://h.example/a#b c", "", "http://h.example/a#b%20c"},
		{"default port dropped", "http://h.example:080/", "", "http://h.example/"},
		{"other port kept", "https://h.example:8443", "", "https://h.example:8443/"},
		{"host lowercased", "HTTP://H.Example/A", "", "http://h.example/A"},
		{"dot segments", "http://h.example/a/./b/../c", "", "http://h.example/a/c"},
		{"percent encoded dots", "http://h.example/a/%2e%2E/c", "", "http://h.example/c"},
		{"trailing dot segment", "http://h.example/a/b/..", "", "http://h.example/a/"},
		{"backslash is slash", `http://h.example\a\b`, "", "http://h.example/a/b"},
		{"credentials", "http://us er:p@ss@h.example/", "", "http://us%20er:p%40ss@h.example/"},
		{"ipv4 decimal", "http://3232235777/", "", "http://192.168.1.1/"},
		{"ipv4 hex and octal", "http://0xC0.0250.1.1/", "", "http://192.168.1.1/"},
		{"ipv6 compressed", "http://[2001:0db8:0:0:0:0:0:1]/", "", "http://[2001:db8::1]/"},
		{"idna host", "http://Bücher.example/", "", "http://xn--bcher-kva.example/"},
		{"scheme without slashes", "http:evil.example/a.png", "", "http://evil.example/a.png"},
		{"scheme without slashes keeps base scheme", "https:evil.example/a.png", base, "https://evil.example/a.png"},
		{"protocol relative takes the base scheme", "//o.example/x", base, "https://o.example/x"},
		{"backslash protocol relative", `\\o.example\p`, base, "https://o.example/p"},
		{"absolute path", "/abs?q=a b", base, "https://b.example/abs?q=a%20b"},
		{"relative path", "a|b", base, "https://b.example/dir/a|b"},
		{"parent path", "../up", base, "https://b.example/up"},
		{"query only", "?y=2", base, "https://b.example/dir/page?y=2"},
		{"fragment only", "#f", base, "https://b.example/dir/page?x=1#f"},
		{"empty reference drops the fragment", "", base, "https://b.example/dir/page?x=1"},
	}
	for _, tc := range tests {
		got, ok := ResolveLink(tc.raw, tc.base)
		if !ok || got != tc.want {
			t.Errorf("%s: ResolveLink(%q, %q) = %q, %v; want %q", tc.name, tc.raw, tc.base, got, ok, tc.want)
		}
	}
}

func TestResolveLinkRefusals(t *testing.T) {
	tests := []struct{ raw, base string }{
		{"mailto:a@b.example", ""},
		{"/relative", ""},
		{"http://", ""},
		{"http://h.example:99999/", ""},
		{"http://1.2.3.256/", ""},
		{"http://1.2.3.4.5/", ""},
		{"http://xn--a/", ""},
		{"http://[::1/", ""},
		{"http://a b/", ""},
		{"x", "mailto:a@b.example"},
	}
	for _, tc := range tests {
		if got, ok := ResolveLink(tc.raw, tc.base); ok {
			t.Errorf("ResolveLink(%q, %q) = %q, want a refusal", tc.raw, tc.base, got)
		}
	}
}

func TestDeeplyNestedMarkupIsKeptAsText(t *testing.T) {
	for _, depth := range []int{511, 512, 600} {
		out := sanitize(strings.Repeat("<div>", depth) + "kept")
		if !strings.Contains(out, "kept") {
			t.Errorf("depth %d lost the content: %q", depth, out)
		}
	}
	mustNotContain(t, Summary(strings.Repeat("<b>", 600)+"x<script>"), "<script>")
}
