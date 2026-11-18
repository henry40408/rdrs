package sanitize

import (
	"testing"

	"golang.org/x/net/html"
)

func element(namespace, name string, attrs ...html.Attribute) *html.Node {
	return &html.Node{Type: html.ElementNode, Namespace: namespace, Data: name, Attr: attrs}
}

func TestExpectedNamespaceAllowsOnlyLegitimateBoundaries(t *testing.T) {
	htmlEncoding := html.Attribute{Key: "encoding", Val: "text/html"}
	cases := []struct {
		name          string
		parent, child *html.Node
		want          bool
	}{
		{"svg root inside html", element("", "div"), element("svg", "svg"), true},
		{"svg shape directly inside html", element("", "div"), element("svg", "circle"), false},
		{"math root inside html", element("", "div"), element("math", "math"), true},
		{"math token directly inside html", element("", "div"), element("math", "mi"), false},
		{"html inside a math text integration point", element("math", "mi"), element("", "b"), true},
		{"foreign element inside a math text integration point", element("math", "ms"), element("svg", "svg"), true},
		{"html inside other math elements", element("math", "mrow"), element("", "b"), false},
		{"html inside annotation-xml declared text/html", element("math", "annotation-xml", htmlEncoding), element("", "p"), true},
		{"annotation-xml with conflicting encodings", element("math", "annotation-xml", htmlEncoding, html.Attribute{Key: "encoding", Val: "x"}), element("", "p"), false},
		{"annotation-xml without an encoding", element("math", "annotation-xml"), element("", "p"), false},
		{"svg inside annotation-xml declared otherwise", element("math", "annotation-xml", html.Attribute{Key: "encoding", Val: "image/svg+xml"}), element("svg", "svg"), true},
		{"html inside foreignObject", element("svg", "foreignObject"), element("", "p"), true},
		{"math inside foreignObject", element("svg", "foreignObject"), element("math", "math"), true},
		{"html inside other svg elements", element("svg", "g"), element("", "p"), false},
		{"known svg element inside svg", element("svg", "g"), element("svg", "circle"), true},
		{"unknown svg element inside svg", element("svg", "g"), element("svg", "bogus"), false},
		{"known math element inside math", element("math", "mrow"), element("math", "mi"), true},
		{"unknown math element inside math", element("math", "mrow"), element("math", "bogus"), false},
		{"html element inside html", element("", "div"), element("", "p"), true},
		{"svg-owned name parsed as html", element("", "div"), element("", "circle"), false},
		{"html name shared with svg", element("", "div"), element("", "span"), true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := expectedNamespace(c.parent, c.child); got != c.want {
				t.Errorf("expectedNamespace = %v, want %v", got, c.want)
			}
		})
	}
}
