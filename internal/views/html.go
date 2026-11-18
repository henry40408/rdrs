// Package views renders the pages. The markup is built with small helpers
// that mirror maud, byte for byte: no whitespace between elements, void
// elements without a closing slash, and the four characters maud escapes.
package views

import (
	"strconv"
	"strings"
)

// node is markup that is already safe to concatenate: every piece of text and
// every attribute value in it went through escape.
type node string

// attr is one attribute. A flag renders bare (`defer`), like maud does; a
// skipped attribute renders nothing, like maud's `name=[None]`.
type attr struct {
	name, value string
	flag        bool
	skip        bool
}

func a(name, value string) attr { return attr{name: name, value: value} }

func flag(name string) attr { return attr{name: name, flag: true} }

// class is the class attribute from the non-empty names.
func class(names ...string) attr {
	var kept []string
	for _, name := range names {
		if name != "" {
			kept = append(kept, name)
		}
	}
	return attr{name: "class", value: strings.Join(kept, " ")}
}

// when keeps at (any attribute) only if ok holds, like maud's `attr[cond]`.
func when(ok bool, at attr) attr {
	at.skip = !ok
	return at
}

// onlyIf yields name when ok holds, else "": a conditional class name.
func onlyIf(ok bool, name string) string {
	if ok {
		return name
	}
	return ""
}

// escape covers both contexts the views use, element text and double-quoted
// attribute values. maud escapes exactly these four characters in both, and
// html/template would write `&#34;` instead of `&quot;`.
var escape = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;").Replace

// text escapes s for use as element content.
func text(s string) node { return node(escape(s)) }

// num renders an integer as text.
func num(n int64) node { return node(strconv.FormatInt(n, 10)) }

// raw is maud's PreEscaped: s is trusted markup.
func raw(s string) node { return node(s) }

// group concatenates nodes without any whitespace between them.
func group(children ...node) node {
	var b strings.Builder
	for _, child := range children {
		b.WriteString(string(child))
	}
	return node(b.String())
}

// show is maud's `@if` without an else: the node when ok, nothing otherwise.
func show(ok bool, n node) node {
	if ok {
		return n
	}
	return ""
}

// each is maud's `@for`.
func each[T any](items []T, render func(T) node) node {
	var b strings.Builder
	for _, item := range items {
		b.WriteString(string(render(item)))
	}
	return node(b.String())
}

func openTag(tag string, attrs []attr) string {
	var b strings.Builder
	b.WriteString("<" + tag)
	for _, at := range attrs {
		if at.skip {
			continue
		}
		b.WriteString(" " + at.name)
		if !at.flag {
			b.WriteString(`="` + escape(at.value) + `"`)
		}
	}
	b.WriteString(">")
	return b.String()
}

// el renders an element with children and a closing tag.
func el(tag string, attrs []attr, children ...node) node {
	return node(openTag(tag, attrs) + string(group(children...)) + "</" + tag + ">")
}

// void renders an element that has no content and no closing tag.
func void(tag string, attrs ...attr) node {
	return node(openTag(tag, attrs))
}

// attrs is shorthand for a literal attribute list.
func attrs(list ...attr) []attr { return list }

const doctype node = "<!DOCTYPE html>"
