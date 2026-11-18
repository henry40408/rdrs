// Package search parses the boolean query language of the global /search page.
//
// Grammar:
//
//	or   := and { "OR" and }
//	and  := not { ["AND"] not }        // implicit AND between adjacent atoms
//	not  := {"NOT"|"-"} atom
//	atom := "(" or ")" | field ":" value | text
//
// NOT, AND and OR are case-insensitive keywords (quote them to search for the
// words). A "token:" is a filter only when token is a known field name.
// Error messages and byte offsets match the Rust build exactly.
package search

import (
	"strings"
	"time"
	"unicode"
	"unicode/utf8"
)

// Node is one node of the parsed query tree.
type Node interface{ isNode() }

// And matches when both sides match.
type And struct{ Left, Right Node }

// Or matches when either side matches.
type Or struct{ Left, Right Node }

// Not negates its operand.
type Not struct{ Inner Node }

// Text is a free-text term matched against title and content.
type Text struct{ Value string }

// Field is a "title:" or "author:" term.
type Field struct {
	Field TextField
	Value string
}

// Source is a "feed:" or "category:" term.
type Source struct {
	Kind  SourceKind
	Value string
}

// StatusFilter is an "is:" term.
type StatusFilter struct{ Status Status }

// Date is a "before:" or "after:" term; Date is midnight UTC.
type Date struct {
	Bound DateBound
	Date  time.Time
}

func (And) isNode()          {}
func (Or) isNode()           {}
func (Not) isNode()          {}
func (Text) isNode()         {}
func (Field) isNode()        {}
func (Source) isNode()       {}
func (StatusFilter) isNode() {}
func (Date) isNode()         {}

// TextField is a column a Field term searches.
type TextField int

// The searchable entry columns.
const (
	FieldTitle TextField = iota
	FieldAuthor
)

// SourceKind says whether a Source term names a feed or a category.
type SourceKind int

// The source kinds.
const (
	SourceFeed SourceKind = iota
	SourceCategory
)

// Status is the value of an "is:" term.
type Status int

// The statuses.
const (
	StatusUnread Status = iota
	StatusRead
	StatusStarred
)

// DateBound says which side of the date a Date term keeps.
type DateBound int

// The date bounds.
const (
	Before DateBound = iota
	After
)

// ParseError is a query the user must fix.
type ParseError struct {
	// Position is the byte offset into the input where the error was detected.
	Position int
	Message  string
}

func (e *ParseError) Error() string { return e.Message }

// FreeTextTerms collects free-text and "title:" values for highlighting,
// skipping negated subtrees.
func FreeTextTerms(node Node) []string {
	switch n := node.(type) {
	case And:
		return append(FreeTextTerms(n.Left), FreeTextTerms(n.Right)...)
	case Or:
		return append(FreeTextTerms(n.Left), FreeTextTerms(n.Right)...)
	case Text:
		return []string{n.Value}
	case Field:
		if n.Field == FieldTitle {
			return []string{n.Value}
		}
	}
	return nil
}

// Parse parses a search query.
func Parse(input string) (Node, error) {
	toks, err := lex(input)
	if err != nil {
		return nil, err
	}
	if len(toks) == 0 {
		return nil, &ParseError{Position: 0, Message: "Query is empty"}
	}
	p := &parser{toks: toks, end: len(input)}
	node, err := p.parseOr()
	if err != nil {
		return nil, err
	}
	if p.pos < len(toks) {
		return nil, &ParseError{
			Position: toks[p.pos].pos,
			Message:  "Unexpected extra input (maybe an extra ')')",
		}
	}
	return node, nil
}

// asciiLower lowercases ASCII letters only, like Rust's to_ascii_lowercase.
func asciiLower(s string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'A' && r <= 'Z' {
			return r + 'a' - 'A'
		}
		return r
	}, s)
}

// fieldKind is a known filter name.
type fieldKind int

const (
	kindIs fieldKind = iota
	kindFeed
	kindCategory
	kindTitle
	kindAuthor
	kindBefore
	kindAfter
)

var fieldKinds = map[string]fieldKind{
	"is":       kindIs,
	"feed":     kindFeed,
	"category": kindCategory,
	"title":    kindTitle,
	"author":   kindAuthor,
	"before":   kindBefore,
	"after":    kindAfter,
}

var fieldLabels = map[fieldKind]string{
	kindIs:       "'is:'",
	kindFeed:     "'feed:'",
	kindCategory: "'category:'",
	kindTitle:    "'title:'",
	kindAuthor:   "'author:'",
	kindBefore:   "'before:'",
	kindAfter:    "'after:'",
}

type tokenKind int

const (
	tokLParen tokenKind = iota
	tokRParen
	tokAnd
	tokOr
	tokNot
	tokMinus
	tokFilter
	tokText
)

type token struct {
	kind  tokenKind
	field fieldKind // tokFilter only
	value string    // tokFilter and tokText
	pos   int
}

func isWordBreak(r rune) bool {
	return unicode.IsSpace(r) || r == '(' || r == ')' || r == '"'
}

func lex(input string) ([]token, error) {
	var out []token
	for k := 0; k < len(input); {
		c, size := utf8.DecodeRuneInString(input[k:])
		switch {
		case unicode.IsSpace(c):
			k += size
		case c == '(':
			out = append(out, token{kind: tokLParen, pos: k})
			k += size
		case c == ')':
			out = append(out, token{kind: tokRParen, pos: k})
			k += size
		case c == '"':
			value, next, err := readQuoted(input, k)
			if err != nil {
				return nil, err
			}
			out = append(out, token{kind: tokText, value: value, pos: k})
			k = next
		case c == '-':
			out = append(out, lexDash(input, k))
			k += size
		default:
			tok, next, err := lexWord(input, k)
			if err != nil {
				return nil, err
			}
			// A word always spans at least one rune. Without this check a lexer
			// that stopped advancing would append tokens until memory ran out,
			// which killed the CI runner while mutation testing flipped the
			// word-break test.
			if next <= k {
				return nil, &ParseError{Position: k, Message: "Unexpected input"}
			}
			out = append(out, tok)
			k = next
		}
	}
	return out, nil
}

// lexDash is a negation only when tight against a following atom; otherwise
// it is the literal "-".
func lexDash(input string, k int) token {
	next, _ := utf8.DecodeRuneInString(input[k+1:])
	tight := k+1 < len(input) && !unicode.IsSpace(next) && next != ')'
	if tight {
		return token{kind: tokMinus, pos: k}
	}
	return token{kind: tokText, value: "-", pos: k}
}

// lexWord reads a run up to whitespace, a parenthesis or a quote.
func lexWord(input string, k int) (token, int, error) {
	end := k
	for end < len(input) {
		r, size := utf8.DecodeRuneInString(input[end:])
		if isWordBreak(r) {
			break
		}
		end += size
	}
	head := input[k:end]

	if name, rest, found := strings.Cut(head, ":"); found {
		if field, known := fieldKinds[asciiLower(name)]; known {
			return lexFilter(input, filterStart{field: field, rest: rest, pos: k, end: end})
		}
	}

	tok := token{kind: tokText, value: head, pos: k}
	switch asciiLower(head) {
	case "and":
		tok = token{kind: tokAnd, pos: k}
	case "or":
		tok = token{kind: tokOr, pos: k}
	case "not":
		tok = token{kind: tokNot, pos: k}
	}
	return tok, end, nil
}

type filterStart struct {
	field fieldKind
	rest  string
	pos   int
	end   int
}

func lexFilter(input string, f filterStart) (token, int, error) {
	if f.rest != "" {
		return token{kind: tokFilter, field: f.field, value: f.rest, pos: f.pos}, f.end, nil
	}
	// `field:"quoted value"`: the value is the tight quote.
	if f.end < len(input) && input[f.end] == '"' {
		value, next, err := readQuoted(input, f.end)
		if err != nil {
			return token{}, 0, err
		}
		return token{kind: tokFilter, field: f.field, value: value, pos: f.pos}, next, nil
	}
	// `field:` with no value; the parser rejects it.
	return token{kind: tokFilter, field: f.field, pos: f.pos}, f.end, nil
}

// readQuoted reads a "..." phrase whose opening quote is input[k]. It returns
// the content and the offset past the closing quote.
func readQuoted(input string, k int) (string, int, error) {
	closing := strings.IndexByte(input[k+1:], '"')
	if closing < 0 {
		return "", 0, &ParseError{Position: k, Message: "Unclosed quote"}
	}
	contentEnd := k + 1 + closing
	return input[k+1 : contentEnd], contentEnd + 1, nil
}

type parser struct {
	toks []token
	pos  int
	end  int
}

func (p *parser) peek() (token, bool) {
	if p.pos >= len(p.toks) {
		return token{}, false
	}
	return p.toks[p.pos], true
}

func (p *parser) peekKind(kind tokenKind) bool {
	tok, ok := p.peek()
	return ok && tok.kind == kind
}

func (p *parser) peekPos() int {
	if tok, ok := p.peek(); ok {
		return tok.pos
	}
	return p.end
}

func startsAtom(kind tokenKind) bool {
	switch kind {
	case tokLParen, tokFilter, tokText, tokNot, tokMinus:
		return true
	}
	return false
}

func (p *parser) parseOr() (Node, error) {
	left, err := p.parseAnd()
	if err != nil {
		return nil, err
	}
	for p.peekKind(tokOr) {
		p.pos++
		right, err := p.parseAnd()
		if err != nil {
			return nil, err
		}
		left = Or{Left: left, Right: right}
	}
	return left, nil
}

func (p *parser) parseAnd() (Node, error) {
	left, err := p.parseNot()
	if err != nil {
		return nil, err
	}
	for {
		tok, ok := p.peek()
		if !ok {
			return left, nil
		}
		switch {
		case tok.kind == tokAnd:
			p.pos++
		case startsAtom(tok.kind):
		default:
			return left, nil // Or, RParen
		}
		right, err := p.parseNot()
		if err != nil {
			return nil, err
		}
		left = And{Left: left, Right: right}
	}
}

func (p *parser) parseNot() (Node, error) {
	if p.peekKind(tokNot) || p.peekKind(tokMinus) {
		p.pos++
		inner, err := p.parseNot()
		if err != nil {
			return nil, err
		}
		return Not{Inner: inner}, nil
	}
	return p.parseAtom()
}

func (p *parser) parseAtom() (Node, error) {
	tok, ok := p.peek()
	if !ok {
		return nil, &ParseError{Position: p.peekPos(), Message: "Expected a search term here"}
	}
	switch tok.kind {
	case tokLParen:
		return p.parseGroup()
	case tokFilter:
		p.pos++
		return nodeFromFilter(tok)
	case tokText:
		p.pos++
		// `""` would be LIKE '%%', matching (and "mark matching as read"
		// marking) everything.
		if tok.value == "" {
			return nil, &ParseError{Position: tok.pos, Message: "Empty quoted phrase"}
		}
		return Text{Value: tok.value}, nil
	}
	return nil, &ParseError{Position: tok.pos, Message: "Expected a search term here"}
}

func (p *parser) parseGroup() (Node, error) {
	openPos := p.peekPos()
	p.pos++
	inner, err := p.parseOr()
	if err != nil {
		return nil, err
	}
	if !p.peekKind(tokRParen) {
		return nil, &ParseError{Position: openPos, Message: "Unbalanced parentheses: '(' is not closed"}
	}
	p.pos++
	return inner, nil
}

func nodeFromFilter(tok token) (Node, error) {
	if tok.value == "" {
		return nil, &ParseError{Position: tok.pos, Message: fieldLabels[tok.field] + " needs a value"}
	}
	switch tok.field {
	case kindIs:
		return statusNode(tok)
	case kindFeed:
		return Source{Kind: SourceFeed, Value: tok.value}, nil
	case kindCategory:
		return Source{Kind: SourceCategory, Value: tok.value}, nil
	case kindTitle:
		return Field{Field: FieldTitle, Value: tok.value}, nil
	case kindAuthor:
		return Field{Field: FieldAuthor, Value: tok.value}, nil
	}
	return dateNode(tok)
}

func statusNode(tok token) (Node, error) {
	value := asciiLower(tok.value)
	switch value {
	case "unread":
		return StatusFilter{Status: StatusUnread}, nil
	case "read":
		return StatusFilter{Status: StatusRead}, nil
	case "starred":
		return StatusFilter{Status: StatusStarred}, nil
	}
	return nil, &ParseError{
		Position: tok.pos,
		Message:  `Unknown is: value "` + value + `" (use unread / read / starred)`,
	}
}

func dateNode(tok token) (Node, error) {
	date, ok := parseChronoDate(tok.value)
	if !ok {
		return nil, &ParseError{
			Position: tok.pos,
			Message:  fieldLabels[tok.field] + " expects a date like YYYY-MM-DD",
		}
	}
	bound := After
	if tok.field == kindBefore {
		bound = Before
	}
	return Date{Bound: bound, Date: date}, nil
}
