// Package htmldepth works around x/net/html refusing documents with more than
// 511 open elements, a limit html5ever (the Rust parser) does not have.
package htmldepth

import (
	"strings"

	"golang.org/x/net/html"
)

// MaxDepth is how deep Flatten lets elements nest. It stays under the parser
// limit with room for the implied html, head and body elements.
const MaxDepth = 400

// IsTooDeep reports whether err is the parser's nesting-limit error.
func IsTooDeep(err error) bool {
	return err != nil && strings.Contains(err.Error(), "open stack of elements exceeds")
}

// Flatten removes the start and end tags of elements nested deeper than
// MaxDepth, keeping their content. Call it only after a parse failed: it
// counts open elements without the parser's implied end tags, so on a
// document that parses fine it could drop tags the parser would have closed.
func Flatten(page string) string {
	type open struct {
		name    string
		dropped bool
	}
	var stack []open
	var out strings.Builder
	tokenizer := html.NewTokenizer(strings.NewReader(page))
	for {
		kind := tokenizer.Next()
		if kind == html.ErrorToken {
			// The only error a string reader gives is io.EOF.
			return out.String()
		}
		raw := tokenizer.Raw()
		switch kind {
		case html.StartTagToken:
			name, _ := tokenizer.TagName()
			if voidElements[string(name)] {
				out.Write(raw)
				continue
			}
			dropped := len(stack) >= MaxDepth
			stack = append(stack, open{name: string(name), dropped: dropped})
			if !dropped {
				out.Write(raw)
			}
		case html.EndTagToken:
			name, _ := tokenizer.TagName()
			index := lastOpen(len(stack), func(i int) bool { return stack[i].name == string(name) })
			if index < 0 {
				out.Write(raw)
				continue
			}
			dropped := stack[index].dropped
			stack = stack[:index]
			if !dropped {
				out.Write(raw)
			}
		default:
			out.Write(raw)
		}
	}
}

func lastOpen(length int, matches func(int) bool) int {
	for i := length - 1; i >= 0; i-- {
		if matches(i) {
			return i
		}
	}
	return -1
}

var voidElements = map[string]bool{
	"area": true, "base": true, "br": true, "col": true, "embed": true, "hr": true,
	"img": true, "input": true, "link": true, "meta": true, "param": true,
	"source": true, "track": true, "wbr": true,
}
