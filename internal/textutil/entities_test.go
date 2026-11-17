package textutil

import (
	"strings"
	"testing"
)

func TestDecodeHTMLEntities(t *testing.T) {
	cases := []struct{ in, want string }{
		{"Collabora&#x27;s CODE", "Collabora's CODE"},
		{"a&#X27;b", "a'b"},
		{"it&#39;s", "it's"},
		{"Tom &amp; Jerry", "Tom & Jerry"},
		{"&quot;hi&quot; &apos;yo&apos;", `"hi" 'yo'`},
		{"&lt;tag&gt;", "<tag>"},
		{"A &amp; B &#x27;C&#x27; &lt;d&gt; &#39;e&#39;", "A & B 'C' <d> 'e'"},
		{"plain text, no entities", "plain text, no entities"},
		{"日本語 &amp; 中文", "日本語 & 中文"},
		{"ends with &", "ends with &"},
		{"R&D", "R&D"},
		{"a&unknown;b", "a&unknown;b"},
		{"100&#nope;", "100&#nope;"},
		{"a & b", "a & b"},
		{"&#xFFFFFFFF;", "&#xFFFFFFFF;"},
		{"&#xD800;", "&#xD800;"},
		{"&#+65;", "A"},
		{"&#x+41;", "A"},
		{"&#++65;", "&#++65;"},
		{"&#;", "&#;"},
		{"&#-5;", "&#-5;"},
		{"&AMP;", "&AMP;"},
		{"&&amp;", "&&"},
		{"&é;", "&é;"},
	}
	for _, c := range cases {
		if got := DecodeHTMLEntities(c.in); got != c.want {
			t.Errorf("DecodeHTMLEntities(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestDecodeHTMLEntitiesWindow(t *testing.T) {
	// The ';' may be at most the 32nd character after '&' (offset 31).
	inside := "&#" + strings.Repeat("0", 28) + "65;"
	if got := DecodeHTMLEntities(inside); got != "A" {
		t.Errorf("semicolon at offset 31: got %q", got)
	}
	outside := "&#" + strings.Repeat("0", 29) + "65;"
	if got := DecodeHTMLEntities(outside); got != outside {
		t.Errorf("semicolon at offset 32: got %q", got)
	}
}
