package sanitize

import (
	"strings"
	"testing"

	"golang.org/x/net/html"
)

func TestResolveLinkBoundaries(t *testing.T) {
	const base = "https://b.example/d/p"
	tests := []struct {
		name, raw, base, want string
	}{
		{"same scheme without a host is relative", "https:?x", base, "https://b.example/d/p?x"},
		{"other scheme without a host fails", "http:?x", base, ""},
		{"bare double slash has no host", "//", base, ""},
		{"empty credentials", "http://@h.example/", "", "http://h.example/"},
		{"port 65535 is valid", "http://h.example:65535/", "", "http://h.example:65535/"},
		{"port 65536 is invalid", "http://h.example:65536/", "", ""},
		{"lone continuation byte host is rejected by idna", "http://%80/", "", ""},
		{"underscore host skips idna", "http://a_b.example/", "", "http://a_b.example/"},
		{"scheme of z only", "z:foo", base, ""},
		{"scheme of upper Z only", "Z:foo", base, ""},
		{"scheme of a only", "a:foo", base, ""},
		{"delete is encoded in path", "http://h.example/a\x7fb", "", "http://h.example/a%7Fb"},
		{"first path set byte is encoded", "http://h.example/a`b", "", "http://h.example/a%60b"},
		{"first query set byte is encoded", "http://h.example/?a b", "", "http://h.example/?a%20b"},
		{"first fragment set byte is encoded", "http://h.example/#a b", "", "http://h.example/#a%20b"},
		{"first userinfo set byte is encoded", "http://a`b@h.example/", "", "http://a%60b@h.example/"},
		{"first special query byte is encoded", "http://h.example/?a'b", "", "http://h.example/?a%27b"},
	}
	for _, tc := range tests {
		got, ok := ResolveLink(tc.raw, tc.base)
		if tc.want == "" {
			if ok {
				t.Errorf("%s: ResolveLink(%q) = %q, want failure", tc.name, tc.raw, got)
			}
			continue
		}
		if !ok || got != tc.want {
			t.Errorf("%s: ResolveLink(%q) = %q, %v, want %q", tc.name, tc.raw, got, ok, tc.want)
		}
	}
}

func TestStripTrackingParamsDecodesEdgeEscapes(t *testing.T) {
	tests := []struct{ in, want string }{
		{"https://e.com/a?utm_x=1&q=%4", "https://e.com/a?q=%254"},
		{"https://e.com/a?utm_x=1&q=%", "https://e.com/a?q=%25"},
		{"https://e.com/a?utm_x=1&q=%41", "https://e.com/a?q=A"},
		{"https://e.com/a?utm_x=1&q=%zz", "https://e.com/a?q=%25zz"},
		{"https://e.com/a?utm_x=1&q=%30%39", "https://e.com/a?q=09"},
		{"https://e.com/a?utm_x=1&q=%2f%2F", "https://e.com/a?q=%2F%2F"},
		{"https://e.com/a?utm_x=1&q=%61%41%66%46", "https://e.com/a?q=aAfF"},
		{"https://e.com/a?utm_x=1&q=%67", "https://e.com/a?q=g"},
		{"https://e.com/a?utm_x=1&q=%3g", "https://e.com/a?q=%253g"},
		{"https://e.com/a?utm_x=1&q=%g3", "https://e.com/a?q=%25g3"},
		{"https://e.com/a?utm_x=1&q=%2g", "https://e.com/a?q=%252g"},
		{"https://e.com/a?utm_x=1&q=%:0", "https://e.com/a?q=%25%3A0"},
		{"https://e.com/a?utm_x=1&q=%/0", "https://e.com/a?q=%25%2F0"},
		{"https://e.com/a?utm_x=1&q=%@0", "https://e.com/a?q=%25%400"},
		{"https://e.com/a?utm_x=1&q=%`0", "https://e.com/a?q=%25%600"},
	}
	for _, tc := range tests {
		if got := StripTrackingParams(tc.in); got != tc.want {
			t.Errorf("StripTrackingParams(%q) = %q, want %q", tc.in, got, tc.want)
		}
	}
}

func TestHarvestDimensionsBoundaries(t *testing.T) {
	tests := []struct{ name, hints, want string }{
		{"digit nine in data attribute", `data-original-width="9" data-original-height="9"`, `width="9" height="9"`},
		{"non numeric data falls back to style", `data-original-width="12px" data-original-height="3" style="width:10px"`, `width="10" height="3"`},
		{"digit nine in style", `style="width:9px;height:9px"`, `width="9" height="9"`},
		{"declaration without digits is skipped", `style="width:px;width:5px;height:6px"`, `width="5" height="6"`},
	}
	for _, tc := range tests {
		out := sanitize(`<img src="https://e.com/a.jpg" ` + tc.hints + `>`)
		if !strings.Contains(out, tc.want) {
			t.Errorf("%s: %s", tc.name, out)
		}
	}
}

func TestAriaHiddenKeepsTextBesideHiddenElement(t *testing.T) {
	mustNotContain(t, sanitize(`<p aria-hidden="true">hide</p>keep`), "hide")
	mustContain(t, sanitize("<p aria-hidden=\"true\">hide</p>\n"), "hide")
}

func TestAnnotationXMLAcceptsXHTMLEncoding(t *testing.T) {
	parent := &html.Node{Type: html.ElementNode, Data: "annotation-xml", Namespace: "math",
		Attr: []html.Attribute{{Key: "encoding", Val: "application/xhtml+xml"}}}
	if !expectedNamespace(parent, &html.Node{Type: html.ElementNode, Data: "p"}) {
		t.Error("html inside annotation-xml declared application/xhtml+xml was rejected")
	}
	parent.Attr[0].Val = "application/x"
	if expectedNamespace(parent, &html.Node{Type: html.ElementNode, Data: "p"}) {
		t.Error("html inside annotation-xml declared application/x was accepted")
	}
}
