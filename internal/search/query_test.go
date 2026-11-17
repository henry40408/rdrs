package search

import (
	"errors"
	"reflect"
	"slices"
	"testing"
	"time"
)

func text(s string) Node { return Text{Value: s} }

func TestParseTrees(t *testing.T) {
	cases := []struct {
		input string
		want  Node
	}{
		{"rust", text("rust")},
		{"rust go", And{text("rust"), text("go")}},
		{"a b OR c", Or{And{text("a"), text("b")}, text("c")}},
		{"a AND (b OR c)", And{text("a"), Or{text("b"), text("c")}}},
		{"a and b", And{text("a"), text("b")}},
		{"NOT rust", Not{text("rust")}},
		{"-rust", Not{text("rust")}},
		{"--rust", Not{Not{text("rust")}}},
		{"cross-platform", text("cross-platform")},
		{"- rust", And{text("-"), text("rust")}},
		{"rust -", And{text("rust"), text("-")}},
		{"(a -)", And{text("a"), text("-")}},
		{"is:unread", StatusFilter{StatusUnread}},
		{"IS:Starred", StatusFilter{StatusStarred}},
		{"is:read", StatusFilter{StatusRead}},
		{"feed:rust", Source{SourceFeed, "rust"}},
		{"Category:news", Source{SourceCategory, "news"}},
		{"author:jane", Field{FieldAuthor, "jane"}},
		{"title:axum", Field{FieldTitle, "axum"}},
		{`feed:"Rust Blog"`, Source{SourceFeed, "Rust Blog"}},
		{`"and"`, text("and")},
		{`"a b"`, text("a b")},
		{"http://example.com", text("http://example.com")},
		{"nofield:value", text("nofield:value")},
		{
			"after:2026-01-01",
			Date{After, time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)},
		},
		{
			"before:2026-1-5",
			Date{Before, time.Date(2026, 1, 5, 0, 0, 0, 0, time.UTC)},
		},
		{"a(b)", And{text("a"), text("b")}},
		{`a"b"`, And{text("a"), text("b")}},
	}
	for _, tc := range cases {
		t.Run(tc.input, func(t *testing.T) {
			got, err := Parse(tc.input)
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(got, tc.want) {
				t.Errorf("got %#v, want %#v", got, tc.want)
			}
		})
	}
}

// The messages and byte offsets are copied from src/models/entry/query.rs.
func TestParseErrors(t *testing.T) {
	cases := []struct {
		input    string
		position int
		message  string
	}{
		{"", 0, "Query is empty"},
		{"   ", 0, "Query is empty"},
		{`""`, 0, "Empty quoted phrase"},
		{`rust ""`, 5, "Empty quoted phrase"},
		{`a "abc`, 2, "Unclosed quote"},
		{`feed:"abc`, 5, "Unclosed quote"},
		{"(rust OR go", 0, "Unbalanced parentheses: '(' is not closed"},
		{"a (b (c)", 2, "Unbalanced parentheses: '(' is not closed"},
		{"rust)", 4, "Unexpected extra input (maybe an extra ')')"},
		{"rust OR", 7, "Expected a search term here"},
		{"OR rust", 0, "Expected a search term here"},
		{"rust AND", 8, "Expected a search term here"},
		{"()", 1, "Expected a search term here"},
		{"x feed:", 2, "'feed:' needs a value"},
		{"é title:", 3, "'title:' needs a value"},
		{"is:", 0, "'is:' needs a value"},
		{"is:archived", 0, `Unknown is: value "archived" (use unread / read / starred)`},
		{"IS:ARCHIVED", 0, `Unknown is: value "archived" (use unread / read / starred)`},
		{"before:yesterday", 0, "'before:' expects a date like YYYY-MM-DD"},
		{"x after:2026-02-30", 2, "'after:' expects a date like YYYY-MM-DD"},
		{"after:2026-13-01", 0, "'after:' expects a date like YYYY-MM-DD"},
		{"after:2026-01-01x", 0, "'after:' expects a date like YYYY-MM-DD"},
		{"after:12345-01-01", 0, "'after:' expects a date like YYYY-MM-DD"},
	}
	for _, tc := range cases {
		t.Run(tc.input, func(t *testing.T) {
			_, err := Parse(tc.input)
			var parseErr *ParseError
			if !errors.As(err, &parseErr) {
				t.Fatalf("got %v, want a ParseError", err)
			}
			if parseErr.Position != tc.position || parseErr.Message != tc.message {
				t.Errorf("got (%d, %q), want (%d, %q)",
					parseErr.Position, parseErr.Message, tc.position, tc.message)
			}
		})
	}
}

func TestParseDateEdges(t *testing.T) {
	for _, value := range []string{"+12345-01-01", "0001-01-01", "2024-02-29", " 2026-01-01", "+262142-01-01", "-262143-01-01"} {
		if _, ok := parseChronoDate(value); !ok {
			t.Errorf("%q should parse", value)
		}
	}
	for _, value := range []string{"2023-02-29", "2026-001-01", "2026/01/01", "", "+ 2026-01-01", "- 2026-01-01", "+262143-01-01", "-262144-01-01"} {
		if _, ok := parseChronoDate(value); ok {
			t.Errorf("%q should not parse", value)
		}
	}
}

func TestFreeTextTermsSkipsNegatedAndNonText(t *testing.T) {
	ast, err := Parse("rust title:axum -go is:unread author:jane")
	if err != nil {
		t.Fatal(err)
	}
	terms := FreeTextTerms(ast)
	slices.Sort(terms)
	if !slices.Equal(terms, []string{"axum", "rust"}) {
		t.Errorf("terms = %v", terms)
	}
}
