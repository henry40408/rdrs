package sanitize

import "golang.org/x/net/html"

// expectedNamespace ports ammonia's check_expected_namespace: an element that
// switches between HTML, SVG and MathML anywhere but a legitimate boundary
// is removed together with its content. parent is the nearest ancestor that
// survives (unknown elements are unwrapped, so they are skipped), which is
// why the children of an unwrapped <math> are dropped.
//
// x/net/html marks namespaces as "" (HTML), "svg" and "math".
func expectedNamespace(parent, child *html.Node) bool {
	pns, cns := parent.Namespace, child.Namespace
	switch {
	case pns == "" && cns == "svg":
		return child.Data == "svg"
	case pns == "" && cns == "math":
		return child.Data == "math"
	case pns == "math" && cns != "math":
		return mathChildAllowed(parent, child)
	case pns == "svg" && cns != "svg":
		return parent.Data == "foreignObject" && (cns != "" || isHTMLTag(child.Data))
	case cns == "svg":
		return svgTagNames[child.Data]
	case cns == "math":
		return mathmlTagNames[child.Data]
	case cns == "":
		return isHTMLTag(child.Data)
	}
	return pns == cns
}

// mathChildAllowed covers leaving MathML, which only text integration points
// and annotation-xml permit.
func mathChildAllowed(parent, child *html.Node) bool {
	if parent.Data != "annotation-xml" {
		switch parent.Data {
		case "mi", "mo", "mn", "ms", "mtext":
			return child.Namespace != "" || isHTMLTag(child.Data)
		}
		return false
	}
	encodings := attrValues(parent, "encoding")
	if child.Namespace == "" && allHTMLEncodings(encodings) {
		return isHTMLTag(child.Data) && len(encodings) == 1
	}
	return child.Namespace == "svg" && child.Data == "svg"
}

func attrValues(n *html.Node, key string) []string {
	var values []string
	for _, a := range n.Attr {
		if a.Key == key {
			values = append(values, a.Val)
		}
	}
	return values
}

func allHTMLEncodings(values []string) bool {
	for _, v := range values {
		if v != "text/html" && v != "application/xhtml+xml" {
			return false
		}
	}
	return true
}

// isHTMLTag is false for names SVG or MathML own, with a few that HTML
// shares.
func isHTMLTag(name string) bool {
	switch name {
	case "title", "style", "font", "a", "script", "span":
		return true
	}
	return !svgTagNames[name] && !mathmlTagNames[name]
}
