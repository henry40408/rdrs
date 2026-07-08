//! Boolean query-language parser for the global `/search` page.
//!
//! Grammar (authoritative copy in the design spec):
//!   or   := and { "OR" and }
//!   and  := not { ["AND"] not }        // implicit AND between adjacent atoms
//!   not  := {"NOT"|"-"} atom
//!   atom := "(" or ")" | field ":" value | text
//!
//! Pure string → AST; no DB access. Dates are validated here via `chrono`.
//! `NOT` / `AND` / `OR` are case-insensitive keywords (quote to search them
//! literally). A `token:` is a filter only when `token` is a known field name.

use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryNode {
    And(Box<QueryNode>, Box<QueryNode>),
    Or(Box<QueryNode>, Box<QueryNode>),
    Not(Box<QueryNode>),
    Text(String),
    Field { field: TextField, value: String },
    Source { kind: SourceKind, value: String },
    Status(Status),
    Date { bound: DateBound, date: NaiveDate },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    Title,
    Author,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Feed,
    Category,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unread,
    Read,
    Starred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBound {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset into the input where the error was detected.
    pub position: usize,
    pub message: String,
}

/// Collect free-text and `title:` values for result highlighting. Negated
/// subtrees are skipped (we do not highlight terms the user excluded).
pub fn free_text_terms(node: &QueryNode) -> Vec<String> {
    let mut out = Vec::new();
    collect_terms(node, &mut out);
    out
}

fn collect_terms(node: &QueryNode, out: &mut Vec<String>) {
    match node {
        QueryNode::And(a, b) | QueryNode::Or(a, b) => {
            collect_terms(a, out);
            collect_terms(b, out);
        }
        QueryNode::Text(s) => out.push(s.clone()),
        QueryNode::Field {
            field: TextField::Title,
            value,
        } => out.push(value.clone()),
        // `Not(_)` (negated subtrees) and all other node kinds contribute no terms.
        _ => {}
    }
}

pub fn parse(input: &str) -> Result<QueryNode, ParseError> {
    let toks = lex(input)?;
    if toks.is_empty() {
        return Err(ParseError {
            position: 0,
            message: "Query is empty".into(),
        });
    }
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        end: input.len(),
    };
    let node = p.parse_or()?;
    if p.pos < toks.len() {
        return Err(ParseError {
            position: toks[p.pos].pos,
            message: "Unexpected extra input (maybe an extra ')')".into(),
        });
    }
    Ok(node)
}

// ---- Field kinds (lexer-internal) --------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Is,
    Feed,
    Category,
    Title,
    Author,
    Before,
    After,
}

impl Field {
    fn from_name(s: &str) -> Option<Field> {
        match s.to_ascii_lowercase().as_str() {
            "is" => Some(Field::Is),
            "feed" => Some(Field::Feed),
            "category" => Some(Field::Category),
            "title" => Some(Field::Title),
            "author" => Some(Field::Author),
            "before" => Some(Field::Before),
            "after" => Some(Field::After),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Field::Is => "'is:'",
            Field::Feed => "'feed:'",
            Field::Category => "'category:'",
            Field::Title => "'title:'",
            Field::Author => "'author:'",
            Field::Before => "'before:'",
            Field::After => "'after:'",
        }
    }
}

// ---- Tokenizer ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Minus,
    Filter(Field, String),
    Text(String),
}

struct Spanned {
    tok: Tok,
    pos: usize,
}

fn lex(input: &str) -> Result<Vec<Spanned>, ParseError> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut k = 0;
    while k < n {
        let (bpos, c) = chars[k];
        if c.is_whitespace() {
            k += 1;
            continue;
        }
        match c {
            '(' => {
                out.push(Spanned {
                    tok: Tok::LParen,
                    pos: bpos,
                });
                k += 1;
            }
            ')' => {
                out.push(Spanned {
                    tok: Tok::RParen,
                    pos: bpos,
                });
                k += 1;
            }
            '"' => {
                let (val, nk) = read_quoted(&chars, input, k)?;
                out.push(Spanned {
                    tok: Tok::Text(val),
                    pos: bpos,
                });
                k = nk;
            }
            '-' => {
                // Tight negation only when the next char exists and is neither
                // whitespace nor ')'. Otherwise a literal "-" text token.
                let neg = k + 1 < n && {
                    let d = chars[k + 1].1;
                    !d.is_whitespace() && d != ')'
                };
                out.push(Spanned {
                    tok: if neg {
                        Tok::Minus
                    } else {
                        Tok::Text("-".into())
                    },
                    pos: bpos,
                });
                k += 1;
            }
            _ => {
                // Word run until whitespace / '(' / ')' / '"'.
                let mut e = k;
                while e < n {
                    let d = chars[e].1;
                    if d.is_whitespace() || d == '(' || d == ')' || d == '"' {
                        break;
                    }
                    e += 1;
                }
                let start_byte = bpos;
                let end_byte = if e < n { chars[e].0 } else { input.len() };
                let head = &input[start_byte..end_byte];

                if let Some((name, rest)) = head.split_once(':')
                    && let Some(field) = Field::from_name(name)
                {
                    if !rest.is_empty() {
                        out.push(Spanned {
                            tok: Tok::Filter(field, rest.to_string()),
                            pos: bpos,
                        });
                        k = e;
                        continue;
                    } else if e < n && chars[e].1 == '"' {
                        // `field:"quoted value"` — value is the tight quote.
                        let (val, nk) = read_quoted(&chars, input, e)?;
                        out.push(Spanned {
                            tok: Tok::Filter(field, val),
                            pos: bpos,
                        });
                        k = nk;
                        continue;
                    } else {
                        // `field:` with no value — parser rejects.
                        out.push(Spanned {
                            tok: Tok::Filter(field, String::new()),
                            pos: bpos,
                        });
                        k = e;
                        continue;
                    }
                }

                let tok = match head.to_ascii_lowercase().as_str() {
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    _ => Tok::Text(head.to_string()),
                };
                out.push(Spanned { tok, pos: bpos });
                k = e;
            }
        }
    }
    Ok(out)
}

/// Read a `"..."` phrase; `chars[k]` must be the opening quote. Returns the
/// inner content and the char index just past the closing quote.
fn read_quoted(
    chars: &[(usize, char)],
    input: &str,
    k: usize,
) -> Result<(String, usize), ParseError> {
    let open_byte = chars[k].0;
    let content_start = chars.get(k + 1).map_or(input.len(), |x| x.0);
    let mut e = k + 1;
    while e < chars.len() {
        if chars[e].1 == '"' {
            let content_end = chars[e].0;
            return Ok((input[content_start..content_end].to_string(), e + 1));
        }
        e += 1;
    }
    Err(ParseError {
        position: open_byte,
        message: "Unclosed quote".into(),
    })
}

// ---- Parser ------------------------------------------------------------

struct Parser<'a> {
    toks: &'a [Spanned],
    pos: usize,
    end: usize,
}

fn starts_atom(t: &Tok) -> bool {
    matches!(
        t,
        Tok::LParen | Tok::Filter(_, _) | Tok::Text(_) | Tok::Not | Tok::Minus
    )
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }
    fn peek_pos(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |s| s.pos)
    }
    fn bump(&mut self) -> &Spanned {
        let s = &self.toks[self.pos];
        self.pos += 1;
        s
    }

    fn parse_or(&mut self) -> Result<QueryNode, ParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.bump();
            let right = self.parse_and()?;
            left = QueryNode::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<QueryNode, ParseError> {
        let mut left = self.parse_not()?;
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.bump();
                    let right = self.parse_not()?;
                    left = QueryNode::And(Box::new(left), Box::new(right));
                }
                Some(t) if starts_atom(t) => {
                    let right = self.parse_not()?;
                    left = QueryNode::And(Box::new(left), Box::new(right));
                }
                _ => break, // Or, RParen, or EOF
            }
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<QueryNode, ParseError> {
        if matches!(self.peek(), Some(Tok::Not | Tok::Minus)) {
            self.bump();
            let inner = self.parse_not()?;
            return Ok(QueryNode::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<QueryNode, ParseError> {
        match self.peek() {
            Some(Tok::LParen) => {
                let open_pos = self.peek_pos();
                self.bump();
                let inner = self.parse_or()?;
                if !matches!(self.peek(), Some(Tok::RParen)) {
                    return Err(ParseError {
                        position: open_pos,
                        message: "Unbalanced parentheses: '(' is not closed".into(),
                    });
                }
                self.bump();
                Ok(inner)
            }
            Some(Tok::Filter(_, _)) => {
                let pos = self.peek_pos();
                let (field, value) = match &self.bump().tok {
                    Tok::Filter(f, v) => (*f, v.clone()),
                    _ => unreachable!(),
                };
                node_from_filter(field, &value, pos)
            }
            Some(Tok::Text(_)) => {
                let s = match &self.bump().tok {
                    Tok::Text(s) => s.clone(),
                    _ => unreachable!(),
                };
                Ok(QueryNode::Text(s))
            }
            _ => Err(ParseError {
                position: self.peek_pos(),
                message: "Expected a search term here".into(),
            }),
        }
    }
}

fn node_from_filter(field: Field, value: &str, pos: usize) -> Result<QueryNode, ParseError> {
    if value.is_empty() {
        return Err(ParseError {
            position: pos,
            message: format!("{} needs a value", field.label()),
        });
    }
    match field {
        Field::Is => match value.to_ascii_lowercase().as_str() {
            "unread" => Ok(QueryNode::Status(Status::Unread)),
            "read" => Ok(QueryNode::Status(Status::Read)),
            "starred" => Ok(QueryNode::Status(Status::Starred)),
            other => Err(ParseError {
                position: pos,
                message: format!("Unknown is: value \"{other}\" (use unread / read / starred)"),
            }),
        },
        Field::Feed => Ok(QueryNode::Source {
            kind: SourceKind::Feed,
            value: value.to_string(),
        }),
        Field::Category => Ok(QueryNode::Source {
            kind: SourceKind::Category,
            value: value.to_string(),
        }),
        Field::Title => Ok(QueryNode::Field {
            field: TextField::Title,
            value: value.to_string(),
        }),
        Field::Author => Ok(QueryNode::Field {
            field: TextField::Author,
            value: value.to_string(),
        }),
        Field::Before | Field::After => {
            let date =
                NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_parse_err| ParseError {
                    position: pos,
                    message: format!("{} expects a date like YYYY-MM-DD", field.label()),
                })?;
            let bound = if field == Field::Before {
                DateBound::Before
            } else {
                DateBound::After
            };
            Ok(QueryNode::Date { bound, date })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn t(s: &str) -> QueryNode {
        s.to_string().pipe_text()
    }
    trait PipeText {
        fn pipe_text(self) -> QueryNode;
    }
    impl PipeText for String {
        fn pipe_text(self) -> QueryNode {
            QueryNode::Text(self)
        }
    }

    #[test]
    fn single_free_text() {
        assert_eq!(parse("rust").unwrap(), QueryNode::Text("rust".into()));
    }

    #[test]
    fn implicit_and_between_terms() {
        assert_eq!(
            parse("rust go").unwrap(),
            QueryNode::And(Box::new(t("rust")), Box::new(t("go")))
        );
    }

    #[test]
    fn or_lower_precedence_than_and() {
        // `a b OR c` => (a AND b) OR c
        let got = parse("a b OR c").unwrap();
        assert_eq!(
            got,
            QueryNode::Or(
                Box::new(QueryNode::And(Box::new(t("a")), Box::new(t("b")))),
                Box::new(t("c"))
            )
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        // `a AND (b OR c)`
        let got = parse("a AND (b OR c)").unwrap();
        assert_eq!(
            got,
            QueryNode::And(
                Box::new(t("a")),
                Box::new(QueryNode::Or(Box::new(t("b")), Box::new(t("c"))))
            )
        );
    }

    #[test]
    fn not_keyword_and_dash_negation() {
        assert_eq!(
            parse("NOT rust").unwrap(),
            QueryNode::Not(Box::new(t("rust")))
        );
        assert_eq!(parse("-rust").unwrap(), QueryNode::Not(Box::new(t("rust"))));
    }

    #[test]
    fn dash_inside_word_is_literal_text() {
        assert_eq!(
            parse("cross-platform").unwrap(),
            QueryNode::Text("cross-platform".into())
        );
    }

    #[test]
    fn dash_with_trailing_space_is_literal() {
        // `- rust` => "-" AND "rust"
        assert_eq!(
            parse("- rust").unwrap(),
            QueryNode::And(Box::new(t("-")), Box::new(t("rust")))
        );
    }

    #[test]
    fn status_filter() {
        assert_eq!(
            parse("is:unread").unwrap(),
            QueryNode::Status(Status::Unread)
        );
        assert_eq!(
            parse("IS:Starred").unwrap(),
            QueryNode::Status(Status::Starred)
        );
    }

    #[test]
    fn unknown_is_value_errors() {
        assert!(parse("is:archived").is_err());
    }

    #[test]
    fn source_and_field_filters() {
        assert_eq!(
            parse("feed:rust").unwrap(),
            QueryNode::Source {
                kind: SourceKind::Feed,
                value: "rust".into()
            }
        );
        assert_eq!(
            parse("author:jane").unwrap(),
            QueryNode::Field {
                field: TextField::Author,
                value: "jane".into()
            }
        );
    }

    #[test]
    fn quoted_value_with_spaces() {
        assert_eq!(
            parse("feed:\"Rust Blog\"").unwrap(),
            QueryNode::Source {
                kind: SourceKind::Feed,
                value: "Rust Blog".into()
            }
        );
    }

    #[test]
    fn quoted_keyword_is_literal_text() {
        assert_eq!(parse("\"and\"").unwrap(), QueryNode::Text("and".into()));
    }

    #[test]
    fn colon_in_non_field_is_literal_text() {
        assert_eq!(
            parse("http://example.com").unwrap(),
            QueryNode::Text("http://example.com".into())
        );
    }

    #[test]
    fn date_filters_parse() {
        assert_eq!(
            parse("after:2026-01-01").unwrap(),
            QueryNode::Date {
                bound: DateBound::After,
                date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
            }
        );
    }

    #[test]
    fn bad_date_errors() {
        let e = parse("before:yesterday").unwrap_err();
        assert!(e.message.contains("YYYY-MM-DD"));
    }

    #[test]
    fn unbalanced_paren_errors() {
        let e = parse("(rust OR go").unwrap_err();
        assert!(e.message.contains("'('"));
    }

    #[test]
    fn trailing_operator_errors() {
        assert!(parse("rust OR").is_err());
    }

    #[test]
    fn stray_close_paren_errors() {
        assert!(parse("rust)").is_err());
    }

    #[test]
    fn free_text_terms_collects_text_and_title_skips_negated() {
        let ast = parse("rust title:axum -go is:unread").unwrap();
        let mut terms = free_text_terms(&ast);
        terms.sort();
        assert_eq!(terms, vec!["axum".to_string(), "rust".to_string()]);
    }
}
