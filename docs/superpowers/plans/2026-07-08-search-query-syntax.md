# Search Query Syntax Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a hand-written boolean query-language parser to the global `/search` page so users can combine field filters (`is:`, `feed:`, `category:`, `title:`, `author:`, `before:`, `after:`), booleans (`AND`/`OR`/`NOT`), grouping, quoting, and `-` negation in one search box.

**Architecture:** A new pure module `src/models/entry/query.rs` (tokenizer → recursive-descent parser → `QueryNode` AST, dates validated with `chrono`). `EntryFilter` gains a `query: Option<QueryNode>` field. `filters.rs` gains `render_query()` that recursively turns the AST into a parenthesized `WHERE` fragment through the existing `Dialect` seam, `AND`-combined into the query by `apply_filter_conditions`. The `/search` handler parses `q`, either sets `EntryFilter.query` or renders an inline error; `search.html` shows the error banner plus a `<details>` syntax-help panel. Backend matching stays `LIKE`/`ILIKE` — no FTS.

**Tech Stack:** Rust, Axum, Askama, sqlx (dual SQLite/Postgres), `chrono` (already a dependency), nextest.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-08-search-query-syntax-design.md` (authoritative for grammar/semantics).
- No new third-party dependencies. `chrono` is already present; use it for dates.
- Tests run with `cargo nextest run` (never `cargo test`). Use `RDRS_FAST_HASH=1` for auth-heavy suites (not needed here but harmless).
- `cargo fmt` before every commit; `cargo clippy --all-targets -- -D warnings` must pass (warnings fail CI).
- All commits GPG-signed (default; do not pass `--no-gpg-sign`). Stage files explicitly by name — never `git add -A`/`git add .`.
- Work stays on branch `feat/search-query-syntax`.
- Scope: global `/search` only. Do NOT touch scoped/category/feed search, the `?` keyboard-help overlay, or add any DB index (analysis in spec §7 confirms existing indexes cover every index-able predicate).
- `/search` is not among the four README screenshots, so no screenshot regeneration is required.

---

### Task 1: Query AST, tokenizer, and recursive-descent parser

**Files:**
- Create: `src/models/entry/query.rs`
- Modify: `src/models/entry/mod.rs:9` (add `pub mod query;`)

**Interfaces:**
- Produces (public API other tasks consume):
  - `pub enum QueryNode { And(Box<QueryNode>, Box<QueryNode>), Or(Box<QueryNode>, Box<QueryNode>), Not(Box<QueryNode>), Text(String), Field { field: TextField, value: String }, Source { kind: SourceKind, value: String }, Status(Status), Date { bound: DateBound, date: chrono::NaiveDate } }` — derives `Debug, Clone, PartialEq, Eq`.
  - `pub enum TextField { Title, Author }`, `pub enum SourceKind { Feed, Category }`, `pub enum Status { Unread, Read, Starred }`, `pub enum DateBound { Before, After }` — each `Debug, Clone, Copy, PartialEq, Eq`.
  - `pub struct ParseError { pub position: usize, pub message: String }` — `Debug, Clone, PartialEq, Eq`. `position` is a **byte** offset into the input.
  - `pub fn parse(input: &str) -> Result<QueryNode, ParseError>`
  - `pub fn free_text_terms(node: &QueryNode) -> Vec<String>` — free-text + `title:` values (skips negated subtrees), for result highlighting.

- [ ] **Step 1: Write the failing tests**

Add to the bottom of the new file `src/models/entry/query.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn t(s: &str) -> QueryNode { s.to_string().pipe_text() }
    trait PipeText { fn pipe_text(self) -> QueryNode; }
    impl PipeText for String { fn pipe_text(self) -> QueryNode { QueryNode::Text(self) } }

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
        assert_eq!(parse("NOT rust").unwrap(), QueryNode::Not(Box::new(t("rust"))));
        assert_eq!(parse("-rust").unwrap(), QueryNode::Not(Box::new(t("rust"))));
    }

    #[test]
    fn dash_inside_word_is_literal_text() {
        assert_eq!(parse("cross-platform").unwrap(), QueryNode::Text("cross-platform".into()));
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
        assert_eq!(parse("is:unread").unwrap(), QueryNode::Status(Status::Unread));
        assert_eq!(parse("IS:Starred").unwrap(), QueryNode::Status(Status::Starred));
    }

    #[test]
    fn unknown_is_value_errors() {
        assert!(parse("is:archived").is_err());
    }

    #[test]
    fn source_and_field_filters() {
        assert_eq!(
            parse("feed:rust").unwrap(),
            QueryNode::Source { kind: SourceKind::Feed, value: "rust".into() }
        );
        assert_eq!(
            parse("author:jane").unwrap(),
            QueryNode::Field { field: TextField::Author, value: "jane".into() }
        );
    }

    #[test]
    fn quoted_value_with_spaces() {
        assert_eq!(
            parse("feed:\"Rust Blog\"").unwrap(),
            QueryNode::Source { kind: SourceKind::Feed, value: "Rust Blog".into() }
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
            QueryNode::Date { bound: DateBound::After, date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() }
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p rdrs query::tests 2>&1 | head -40` (adjust `-p` to the crate name if different; the module path is `models::entry::query::tests`).
Expected: FAIL to compile — `parse`, `QueryNode`, etc. not defined.

- [ ] **Step 3: Implement the module**

Write the full implementation at the top of `src/models/entry/query.rs` (above the `#[cfg(test)] mod tests`):

```rust
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
pub enum TextField { Title, Author }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind { Feed, Category }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status { Unread, Read, Starred }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBound { Before, After }

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
        QueryNode::Not(_) => {}
        QueryNode::Text(s) => out.push(s.clone()),
        QueryNode::Field { field: TextField::Title, value } => out.push(value.clone()),
        _ => {}
    }
}

pub fn parse(input: &str) -> Result<QueryNode, ParseError> {
    let toks = lex(input)?;
    if toks.is_empty() {
        return Err(ParseError { position: 0, message: "查詢是空的".into() });
    }
    let mut p = Parser { toks: &toks, pos: 0, end: input.len() };
    let node = p.parse_or()?;
    if p.pos < toks.len() {
        return Err(ParseError {
            position: toks[p.pos].pos,
            message: "多餘的字元（可能是多出的 ')'）".into(),
        });
    }
    Ok(node)
}

// ---- Field kinds (lexer-internal) --------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field { Is, Feed, Category, Title, Author, Before, After }

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
            '(' => { out.push(Spanned { tok: Tok::LParen, pos: bpos }); k += 1; }
            ')' => { out.push(Spanned { tok: Tok::RParen, pos: bpos }); k += 1; }
            '"' => {
                let (val, nk) = read_quoted(&chars, input, k)?;
                out.push(Spanned { tok: Tok::Text(val), pos: bpos });
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
                    tok: if neg { Tok::Minus } else { Tok::Text("-".into()) },
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

                if let Some((name, rest)) = head.split_once(':') {
                    if let Some(field) = Field::from_name(name) {
                        if !rest.is_empty() {
                            out.push(Spanned { tok: Tok::Filter(field, rest.to_string()), pos: bpos });
                            k = e;
                            continue;
                        } else if e < n && chars[e].1 == '"' {
                            // `field:"quoted value"` — value is the tight quote.
                            let (val, nk) = read_quoted(&chars, input, e)?;
                            out.push(Spanned { tok: Tok::Filter(field, val), pos: bpos });
                            k = nk;
                            continue;
                        } else {
                            // `field:` with no value — parser rejects.
                            out.push(Spanned { tok: Tok::Filter(field, String::new()), pos: bpos });
                            k = e;
                            continue;
                        }
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
fn read_quoted(chars: &[(usize, char)], input: &str, k: usize) -> Result<(String, usize), ParseError> {
    let open_byte = chars[k].0;
    let content_start = chars.get(k + 1).map(|x| x.0).unwrap_or_else(|| input.len());
    let mut e = k + 1;
    while e < chars.len() {
        if chars[e].1 == '"' {
            let content_end = chars[e].0;
            return Ok((input[content_start..content_end].to_string(), e + 1));
        }
        e += 1;
    }
    Err(ParseError { position: open_byte, message: "引號未關閉".into() })
}

// ---- Parser ------------------------------------------------------------

struct Parser<'a> {
    toks: &'a [Spanned],
    pos: usize,
    end: usize,
}

fn starts_atom(t: &Tok) -> bool {
    matches!(t, Tok::LParen | Tok::Filter(_, _) | Tok::Text(_) | Tok::Not | Tok::Minus)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|s| &s.tok)
    }
    fn peek_pos(&self) -> usize {
        self.toks.get(self.pos).map(|s| s.pos).unwrap_or(self.end)
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
        if matches!(self.peek(), Some(Tok::Not) | Some(Tok::Minus)) {
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
                    return Err(ParseError { position: open_pos, message: "括號不對稱：'(' 未關閉".into() });
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
            _ => Err(ParseError { position: self.peek_pos(), message: "此處需要一個搜尋條件".into() }),
        }
    }
}

fn node_from_filter(field: Field, value: &str, pos: usize) -> Result<QueryNode, ParseError> {
    if value.is_empty() {
        return Err(ParseError { position: pos, message: format!("{} 後面需要一個值", field.label()) });
    }
    match field {
        Field::Is => match value.to_ascii_lowercase().as_str() {
            "unread" => Ok(QueryNode::Status(Status::Unread)),
            "read" => Ok(QueryNode::Status(Status::Read)),
            "starred" => Ok(QueryNode::Status(Status::Starred)),
            other => Err(ParseError {
                position: pos,
                message: format!("未知的 is: 值「{other}」（可用 unread / read / starred）"),
            }),
        },
        Field::Feed => Ok(QueryNode::Source { kind: SourceKind::Feed, value: value.to_string() }),
        Field::Category => Ok(QueryNode::Source { kind: SourceKind::Category, value: value.to_string() }),
        Field::Title => Ok(QueryNode::Field { field: TextField::Title, value: value.to_string() }),
        Field::Author => Ok(QueryNode::Field { field: TextField::Author, value: value.to_string() }),
        Field::Before | Field::After => {
            let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ParseError {
                position: pos,
                message: format!("{} 需要 YYYY-MM-DD 格式的日期", field.label()),
            })?;
            let bound = if field == Field::Before { DateBound::Before } else { DateBound::After };
            Ok(QueryNode::Date { bound, date })
        }
    }
}
```

Also register the module — edit `src/models/entry/mod.rs:9`, changing `mod filters;` block to add above/below it:

```rust
pub mod query;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run models::entry::query`
Expected: PASS (all Task 1 tests green).

Then: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

Note: the test helper uses a tiny `PipeText` trait to keep assertions short; if clippy objects to it, inline `QueryNode::Text("...".into())` instead.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/query.rs src/models/entry/mod.rs
git commit -m "feat(search): add boolean query-language parser (AST + lexer)"
```

---

### Task 2: `EntryFilter.query` + AST→SQL rendering (`filters.rs`)

**Files:**
- Modify: `src/models/entry/mod.rs:59-75` (add `query` field to `EntryFilter`)
- Modify: `src/models/entry/filters.rs` (add `render_query`, `like_contains`, `Dialect::ci_like_esc`; call render in `apply_filter_conditions`; extend `is_no_entry_side_predicate`)

**Interfaces:**
- Consumes from Task 1: `QueryNode`, `TextField`, `SourceKind`, `Status`, `DateBound` from `super::query`.
- Produces: `EntryFilter.query: Option<QueryNode>` (set by Task 4); `render_query` invoked internally by `apply_filter_conditions`, so all `list_by_user`/`count_by_user` callers gain query support with `query: None` a no-op.

- [ ] **Step 1: Write the failing SQL-shape tests**

Add these tests inside the existing `#[cfg(test)] mod tests` in `src/models/entry/filters.rs` (alongside the dialect tests):

```rust
use super::super::query::{parse as parse_query};

fn render(qs: &str, dialect: Dialect) -> (String, Vec<Bind>) {
    let ast = parse_query(qs).expect("valid query");
    let mut binds = Vec::new();
    let frag = render_query(&ast, &mut binds, dialect);
    (frag, binds)
}

#[test]
fn render_status_unread_sqlite() {
    let (frag, binds) = render("is:unread", Dialect::Sqlite);
    assert_eq!(frag, "e.read_at IS NULL");
    assert!(binds.is_empty());
}

#[test]
fn render_free_text_matches_title_and_content_with_escape() {
    let (frag, binds) = render("rust", Dialect::Sqlite);
    assert_eq!(
        frag,
        "(e.title LIKE $1 ESCAPE '\\' OR e.content_text LIKE $1 ESCAPE '\\')"
    );
    assert!(matches!(&binds[0], Bind::Text(s) if s == "%rust%"));
}

#[test]
fn render_free_text_pg_uses_ilike() {
    let (frag, _) = render("rust", Dialect::Postgres);
    assert_eq!(
        frag,
        "(e.title ILIKE $1 ESCAPE '\\' OR e.content_text ILIKE $1 ESCAPE '\\')"
    );
}

#[test]
fn render_like_wildcards_are_escaped() {
    let (_, binds) = render("50%", Dialect::Sqlite);
    assert!(matches!(&binds[0], Bind::Text(s) if s == "%50\\%%"));
}

#[test]
fn render_feed_and_author_columns() {
    let (feed, _) = render("feed:news", Dialect::Sqlite);
    assert_eq!(feed, "f.title LIKE $1 ESCAPE '\\'");
    let (author, _) = render("author:jane", Dialect::Sqlite);
    assert_eq!(author, "e.author LIKE $1 ESCAPE '\\'");
}

#[test]
fn render_boolean_nesting_and_bind_numbering() {
    // `(rust OR go) AND is:unread` — two text binds, status has none.
    let (frag, binds) = render("(rust OR go) AND is:unread", Dialect::Sqlite);
    assert_eq!(
        frag,
        "(((e.title LIKE $1 ESCAPE '\\' OR e.content_text LIKE $1 ESCAPE '\\') \
OR (e.title LIKE $2 ESCAPE '\\' OR e.content_text LIKE $2 ESCAPE '\\')) AND e.read_at IS NULL)"
    );
    assert_eq!(binds.len(), 2);
}

#[test]
fn render_not_wraps() {
    let (frag, _) = render("-is:read", Dialect::Sqlite);
    assert_eq!(frag, "(NOT e.read_at IS NOT NULL)");
}

#[test]
fn render_after_date_sqlite_epoch_and_int_bind() {
    let (frag, binds) = render("after:2026-01-01", Dialect::Sqlite);
    assert_eq!(
        frag,
        "CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) >= $1"
    );
    // 2026-01-01T00:00:00Z
    assert!(matches!(binds[0], Bind::Int(1_767_225_600)));
}

#[test]
fn render_before_uses_lt_and_pg_epoch() {
    let (frag, _) = render("before:2026-01-01", Dialect::Postgres);
    assert_eq!(
        frag,
        "EXTRACT(EPOCH FROM COALESCE(e.published_at, e.created_at))::bigint < $1"
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run models::entry::filters 2>&1 | head -30`
Expected: FAIL to compile — `render_query`, `ci_like_esc` not defined.

- [ ] **Step 3: Implement the rendering**

3a. Add the `query` field to `EntryFilter` in `src/models/entry/mod.rs` (inside the struct at lines 59-75; `#[serde(skip)]` because it is set programmatically, never deserialized from a form):

```rust
    /// Parsed boolean query AST for the global `/search` page. Set by the
    /// search handler from the `?q=` string; `None` on every other list path
    /// (a no-op). Rendered to SQL by `filters::render_query`.
    #[serde(skip)]
    pub query: Option<query::QueryNode>,
```

3b. In `src/models/entry/filters.rs`, extend the imports at the top (the `use super::{...}` line) to add the query types:

```rust
use super::query::{DateBound, QueryNode, SourceKind, Status, TextField};
```

3c. Add the `ci_like_esc` method to `impl Dialect` (next to `ci_like`):

```rust
    /// Case-insensitive `LIKE` with an explicit backslash `ESCAPE` clause, so
    /// user `%` / `_` / `\` (escaped by `like_contains`) match literally.
    /// SQLite's `LIKE` is already ASCII-case-insensitive, so no `COLLATE`
    /// suffix is needed; PostgreSQL uses `ILIKE`.
    fn ci_like_esc(self, column: &str, placeholder: usize) -> String {
        match self {
            Dialect::Sqlite => format!("{column} LIKE ${placeholder} ESCAPE '\\'"),
            Dialect::Postgres => format!("{column} ILIKE ${placeholder} ESCAPE '\\'"),
        }
    }
```

3d. Add the renderer and the escape helper (e.g. just below `apply_filter_conditions`):

```rust
/// Escape a user value for a `LIKE '%...%'` contains-match: backslash-escape
/// the LIKE metacharacters `\`, `%`, `_` (paired with an `ESCAPE '\'` clause)
/// and wrap in `%...%`.
fn like_contains(value: &str) -> String {
    let mut esc = String::with_capacity(value.len() + 2);
    esc.push('%');
    for c in value.chars() {
        if matches!(c, '\\' | '%' | '_') {
            esc.push('\\');
        }
        esc.push(c);
    }
    esc.push('%');
    esc
}

/// Recursively render a parsed query AST into a parenthesized WHERE fragment,
/// pushing one `Bind` per leaf that needs a value. Dispatches dialect-specific
/// SQL through `Dialect`.
pub(super) fn render_query(node: &QueryNode, binds: &mut Vec<Bind>, dialect: Dialect) -> String {
    match node {
        QueryNode::And(a, b) => format!(
            "({} AND {})",
            render_query(a, binds, dialect),
            render_query(b, binds, dialect)
        ),
        QueryNode::Or(a, b) => format!(
            "({} OR {})",
            render_query(a, binds, dialect),
            render_query(b, binds, dialect)
        ),
        QueryNode::Not(a) => format!("(NOT {})", render_query(a, binds, dialect)),
        QueryNode::Text(t) => {
            let idx = binds.len() + 1;
            let frag = format!(
                "({} OR {})",
                dialect.ci_like_esc("e.title", idx),
                dialect.ci_like_esc("e.content_text", idx)
            );
            binds.push(Bind::Text(like_contains(t)));
            frag
        }
        QueryNode::Field { field, value } => {
            let col = match field {
                TextField::Title => "e.title",
                TextField::Author => "e.author",
            };
            let idx = binds.len() + 1;
            let frag = dialect.ci_like_esc(col, idx);
            binds.push(Bind::Text(like_contains(value)));
            frag
        }
        QueryNode::Source { kind, value } => {
            let col = match kind {
                SourceKind::Feed => "f.title",
                SourceKind::Category => "c.name",
            };
            let idx = binds.len() + 1;
            let frag = dialect.ci_like_esc(col, idx);
            binds.push(Bind::Text(like_contains(value)));
            frag
        }
        QueryNode::Status(s) => match s {
            Status::Unread => "e.read_at IS NULL".to_string(),
            Status::Read => "e.read_at IS NOT NULL".to_string(),
            Status::Starred => "e.starred_at IS NOT NULL".to_string(),
        },
        QueryNode::Date { bound, date } => {
            let epoch = dialect.epoch("COALESCE(e.published_at, e.created_at)");
            let secs = date
                .and_hms_opt(0, 0, 0)
                .expect("valid midnight")
                .and_utc()
                .timestamp();
            let idx = binds.len() + 1;
            let cmp = match bound {
                DateBound::After => ">=",
                DateBound::Before => "<",
            };
            let frag = format!("{epoch} {cmp} ${idx}");
            binds.push(Bind::Int(secs));
            frag
        }
    }
}
```

3e. Wire it into `apply_filter_conditions` — add at the END of that function's body (after the `has_summary` block, before the closing brace at line ~231):

```rust
    if let Some(ref q) = filter.query {
        conditions.push(render_query(q, binds, dialect));
    }
```

3f. Extend `is_no_entry_side_predicate` (line 118) to include the new field, so a query-only search doesn't get forced onto `idx_entry_sort_ts`:

```rust
        && filter.has_summary.is_none()
        && filter.query.is_none()
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo nextest run models::entry::filters`
Expected: PASS (new render tests + existing dialect tests all green).

Then: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/filters.rs src/models/entry/mod.rs
git commit -m "feat(search): render query AST to SQL via the Dialect seam"
```

---

### Task 3: End-to-end integration tests against real SQLite

**Files:**
- Modify: `src/models/entry/mod.rs` (extend the existing `#[cfg(test)] mod tests` search section, ~line 2017)

**Interfaces:**
- Consumes: `query::parse`, `EntryFilter { query, .. }`, `list_by_user` — all from earlier tasks.
- Produces: confidence that parsed queries return the correct rows on a live DB.

- [ ] **Step 1: Write the failing integration tests**

Add to the search test module in `src/models/entry/mod.rs`. Reuse whatever fixture/setup helper the neighboring search tests use (e.g. `test_search_entries_by_title` at line 2017 shows the pattern: create a pool, a user/category/feed, upsert entries, then call `list_by_user`). Mirror that helper here — do not invent a new one. Example shape (adapt names to the existing helpers):

```rust
#[tokio::test]
async fn query_is_unread_returns_only_unread() {
    let ctx = setup_search_fixture().await; // existing helper used by sibling tests
    // ctx seeds: entry "alpha" unread, entry "beta" read (read_at set).
    let filter = EntryFilter {
        query: Some(query::parse("is:unread").unwrap()),
        ..Default::default()
    };
    let rows = list_by_user(&ctx.db, ctx.user_id, &filter, EntrySortOrder::PublishedAt, 50, 0)
        .await
        .unwrap();
    let titles: Vec<_> = rows.iter().filter_map(|r| r.entry.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "alpha"));
    assert!(!titles.iter().any(|t| t == "beta"));
}

#[tokio::test]
async fn query_boolean_and_negation() {
    let ctx = setup_search_fixture().await;
    // "rust" unread, "rust weekly" read, "go" unread.
    let filter = EntryFilter {
        query: Some(query::parse("rust -is:read").unwrap()),
        ..Default::default()
    };
    let rows = list_by_user(&ctx.db, ctx.user_id, &filter, EntrySortOrder::PublishedAt, 50, 0)
        .await
        .unwrap();
    let titles: Vec<_> = rows.iter().filter_map(|r| r.entry.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "rust"));
    assert!(!titles.iter().any(|t| t == "rust weekly")); // read → excluded
    assert!(!titles.iter().any(|t| t == "go"));          // no "rust" → excluded
}

#[tokio::test]
async fn query_feed_name_fuzzy_match() {
    let ctx = setup_search_fixture().await; // feed titled "Rust Blog"
    let filter = EntryFilter {
        query: Some(query::parse("feed:rust").unwrap()),
        ..Default::default()
    };
    let rows = list_by_user(&ctx.db, ctx.user_id, &filter, EntrySortOrder::PublishedAt, 50, 0)
        .await
        .unwrap();
    assert!(!rows.is_empty());
}

#[tokio::test]
async fn query_after_date_filters() {
    let ctx = setup_search_fixture().await; // "old" published 2025, "new" published 2026
    let filter = EntryFilter {
        query: Some(query::parse("after:2026-01-01").unwrap()),
        ..Default::default()
    };
    let rows = list_by_user(&ctx.db, ctx.user_id, &filter, EntrySortOrder::PublishedAt, 50, 0)
        .await
        .unwrap();
    let titles: Vec<_> = rows.iter().filter_map(|r| r.entry.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "new"));
    assert!(!titles.iter().any(|t| t == "old"));
}

#[tokio::test]
async fn query_escapes_literal_percent() {
    let ctx = setup_search_fixture().await; // entry titled "50% off", entry "50 dollars"
    let filter = EntryFilter {
        query: Some(query::parse("\"50%\"").unwrap()),
        ..Default::default()
    };
    let rows = list_by_user(&ctx.db, ctx.user_id, &filter, EntrySortOrder::PublishedAt, 50, 0)
        .await
        .unwrap();
    let titles: Vec<_> = rows.iter().filter_map(|r| r.entry.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "50% off"));
    assert!(!titles.iter().any(|t| t == "50 dollars")); // literal %, not wildcard
}
```

Note: the exact fixture helper name (`setup_search_fixture`) and its returned struct (`ctx.db`, `ctx.user_id`) MUST be replaced with whatever the sibling search tests already use. Read `test_search_entries_by_title` (mod.rs:2017) and `test_search_combined_with_filters` (mod.rs:2153) first and copy their setup verbatim; extend the seed data with the extra entries/feeds those new assertions need.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo nextest run models::entry::tests::query_ 2>&1 | head -40`
Expected: FAIL — either compile (helper name) until you align with the existing fixture, then assertion failures if seed data is missing. Fix the fixture wiring until they compile and fail only where the feature is exercised.

- [ ] **Step 3: Make them pass**

No new production code should be needed (Tasks 1-2 implement the behavior). If a test fails, debug via `superpowers:systematic-debugging` — likely a seed-data or fixture-alignment issue, not a logic gap. Adjust seed data / assertions.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo nextest run models::entry::tests`
Expected: PASS (new query integration tests + all existing search tests).

Then: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

- [ ] **Step 5: Commit**

```bash
git add src/models/entry/mod.rs
git commit -m "test(search): integration coverage for parsed query filters (SQLite)"
```

---

### Task 4: `/search` handler wiring, error banner, and syntax-help panel

**Files:**
- Modify: `src/handlers/pages/mod.rs:1735-1808` (`search_page`) and `:2791` (`SearchTemplate` — add `error` field)
- Modify: `templates/search.html` (error banner + `<details>` help panel; branch on `error`)
- Modify: `static/css/app.css` (append `.search-syntax-help` / `.search-error` styles)

**Interfaces:**
- Consumes: `entry::query::{parse, free_text_terms}`, `EntryFilter { query, .. }`.
- Produces: `/search` renders results for a valid query, or an inline error (no results) for an invalid one; a collapsed help panel is always present.

- [ ] **Step 1: Write the failing handler test**

Add a handler-level test. If `src/handlers/pages/mod.rs` (or a sibling `tests` module) already has SSR handler tests that build an app/router and hit routes, mirror that. Otherwise add a focused test that calls `entry::query::parse` through the same code path by extracting the parse+error formatting into a small helper and testing it. Preferred (integration) form, adapted to the repo's existing handler-test harness:

```rust
#[tokio::test]
async fn search_invalid_query_shows_error_no_results() {
    let app = test_app().await; // existing harness used by other handler tests
    let resp = app.get("/search?q=%28rust%20OR").await; // "(rust OR" url-encoded
    let body = resp.text().await;
    assert!(body.contains("搜尋語法錯誤"));
    assert!(!body.contains("data-testid=\"search-results\""));
}

#[tokio::test]
async fn search_valid_query_renders_results_area() {
    let app = test_app_with_seed().await; // seeded with a matching entry
    let resp = app.get("/search?q=is%3Aunread").await;
    let body = resp.text().await;
    assert!(!body.contains("搜尋語法錯誤"));
}
```

If no such handler harness exists, instead unit-test a new pure helper `fn parse_or_error(q: &str) -> Result<entry::query::QueryNode, String>` (returning the formatted `搜尋語法錯誤（第 N 字）：…` string) and call it from the handler. Add the test next to the helper.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run search_ 2>&1 | head -30`
Expected: FAIL (route/handler not yet parsing; `error` field missing).

- [ ] **Step 3: Implement the handler + template + CSS**

3a. Add the `error` field to `SearchTemplate` (`src/handlers/pages/mod.rs:2791`):

```rust
pub struct SearchTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub q: String,
    pub error: Option<String>,
    pub results: Vec<SearchResultView>,
}
```

3b. Rewrite the body of `search_page` (`mod.rs:1740-1808`) — replace the `results` computation and the returned template:

```rust
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let q = query.q.unwrap_or_default().trim().to_string();
    let user_id = auth_user.user.id;

    let mut error: Option<String> = None;
    let results = if q.is_empty() {
        Vec::new()
    } else {
        match entry::query::parse(&q) {
            Err(e) => {
                // Byte offset → 1-based character position for the message.
                let char_pos = q.get(..e.position).map(|p| p.chars().count()).unwrap_or(0) + 1;
                error = Some(format!("搜尋語法錯誤（第 {char_pos} 字）：{}", e.message));
                Vec::new()
            }
            Ok(ast) => {
                let terms = entry::query::free_text_terms(&ast);
                let needle = terms.first().cloned().unwrap_or_default();
                let filter = entry::EntryFilter {
                    query: Some(ast),
                    ..Default::default()
                };
                const LIMIT: i64 = 50;
                let rows = entry::list_by_user(
                    &state.db,
                    user_id,
                    &filter,
                    entry::EntrySortOrder::PublishedAt,
                    LIMIT,
                    0,
                )
                .await
                .unwrap_or_default();
                rows.into_iter()
                    .map(|e| {
                        let title = e
                            .entry
                            .title
                            .clone()
                            .unwrap_or_else(|| "(no title)".to_string());
                        let snippet = build_snippet(
                            e.entry.content.as_deref().or(e.entry.summary.as_deref()),
                            &needle,
                            200,
                        );
                        let (published_relative, published_at_iso) =
                            format_relative_time(e.entry.published_at);
                        SearchResultView {
                            entry_id: e.entry.id,
                            title_html: highlight_html(&title, &needle),
                            feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                            published_relative,
                            published_at_iso,
                            snippet_html: highlight_html(&snippet, &needle),
                        }
                    })
                    .collect()
            }
        }
    };

    (
        flash,
        SearchTemplate {
            title: "Search",
            git_version: crate::GIT_VERSION,
            layout,
            q,
            error,
            results,
        },
    )
```

(Note: `highlight_html`/`build_snippet` already no-op on an empty `needle` — verified in `search_text.rs` — so a pure structured query like `is:unread` renders escaped titles and leading snippets.)

3c. Update `templates/search.html` — insert the syntax-help `<details>` right after the `</form>`/`<script>` block (after line 31), and branch on `error` for the results region. Replace the `{% if q.is_empty() %} … {% endif %}` block (lines 33-62) with:

```html
                <details class="search-syntax-help">
                    <summary>Search syntax</summary>
                    <div class="search-syntax-help-body">
                        <ul>
                            <li><code>is:unread</code> / <code>is:read</code> / <code>is:starred</code> — by status</li>
                            <li><code>feed:name</code> / <code>category:name</code> — by source (fuzzy, case-insensitive)</li>
                            <li><code>title:word</code> / <code>author:name</code> — by field</li>
                            <li><code>before:2026-01-01</code> / <code>after:2026-01-01</code> — by date (UTC)</li>
                            <li><code>AND</code> <code>OR</code> <code>NOT</code> and <code>(&nbsp;)</code> — combine; adjacent words imply AND</li>
                            <li><code>-term</code> — exclude; <code>"exact phrase"</code> — quote (also for values with spaces, e.g. <code>feed:"Rust Blog"</code>)</li>
                        </ul>
                    </div>
                </details>

                {% if let Some(err) = error %}
                    <div class="search-error" role="alert" data-testid="search-error">{{ err }}</div>
                {% else if q.is_empty() %}
                    <div class="empty-state">
                        <h2 class="empty-state-title">Search your library</h2>
                        <p class="empty-state-text">Type a keyword and press <kbd class="empty-state-kbd">Enter</kbd> to find entries by title or content.</p>
                    </div>
                {% else if results.is_empty() %}
                    <div class="empty-state" data-testid="search-empty">
                        <h2 class="empty-state-title">No matches</h2>
                        <p class="empty-state-text">Nothing matched &ldquo;{{ q }}&rdquo;. Try another keyword or check the spelling.</p>
                    </div>
                {% else %}
                    <ul class="search-results" data-testid="search-results">
                        {% for r in results %}
                            <li class="search-result">
                                <a href="/entries/{{ r.entry_id }}" class="search-result-title">{{ r.title_html|safe }}</a>
                                <div class="search-result-meta">
                                    <span class="muted">{{ r.feed_title }}</span>
                                    {% if !r.published_relative.is_empty() %}
                                        <span class="muted">&middot;
                                            {% if !r.published_at_iso.is_empty() %}<time datetime="{{ r.published_at_iso }}">{{ r.published_relative }}</time>{% else %}{{ r.published_relative }}{% endif %}
                                        </span>
                                    {% endif %}
                                </div>
                                {% if !r.snippet_html.is_empty() %}
                                    <p class="search-result-snippet">{{ r.snippet_html|safe }}</p>
                                {% endif %}
                            </li>
                        {% endfor %}
                    </ul>
                {% endif %}
```

3d. Append to `static/css/app.css` (reuse existing tokens; the `<details>`/`summary` base styling at `app.css:2005-2008` already applies):

```css
.search-syntax-help {
    margin: var(--space-sm) 0 var(--space-md);
    font-size: var(--font-sm);
}
.search-syntax-help-body ul {
    margin: var(--space-xs) 0 0;
    padding-left: var(--space-lg);
    color: var(--color-text-muted);
}
.search-syntax-help-body li {
    margin: var(--space-2xs) 0;
}
.search-syntax-help-body code {
    font-family: var(--font-mono, monospace);
}
.search-error {
    margin: var(--space-md) 0;
    padding: var(--space-sm) var(--space-md);
    border-radius: var(--radius-md);
    background: var(--color-danger-bg, #fde8e8);
    color: var(--color-danger-text, #9b1c1c);
}
```

Note: verify the exact token names against the top of `app.css` (`--space-*`, `--font-*`, `--radius-*`, `--color-danger-*`). If a token doesn't exist, substitute the nearest existing one or a literal — check what `.feed-http-hint` and existing alert/flash styles use and match them.

- [ ] **Step 4: Rebuild and run tests**

Because CSS/templates are embedded via `include_str!`, rebuild first:

Run: `cargo build && cargo nextest run search_`
Expected: PASS.

Then: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

Manual smoke (optional, per `/run` skill): start the app, visit `/search?q=(rust OR` → error banner; `/search?q=is:unread rust` → results; expand the "Search syntax" panel.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/pages/mod.rs templates/search.html static/css/app.css
git commit -m "feat(search): parse query on /search with error banner and syntax help"
```

---

### Task 5: Postgres parity tests + documentation

**Files:**
- Modify: `tests/postgres_test.rs` (add 1-2 query cases; env-gated, runs on the CI Postgres lane)
- Modify: `ARCHITECTURE.md` (update the search description)

**Interfaces:**
- Consumes: everything above; validates the PG `ILIKE` + epoch path against a live server.

- [ ] **Step 1: Write the failing Postgres tests**

Mirror the existing env-gated pattern in `tests/postgres_test.rs` (it already validates PG list paths). Add:

```rust
#[tokio::test]
async fn pg_query_is_unread_and_free_text() {
    let Some(db) = pg_db_from_env().await else { return }; // existing skip-if-no-DSN guard
    // seed a user/category/feed + one unread entry titled "rust news"
    // ... (reuse the file's existing seed helpers) ...
    let filter = rdrs::models::entry::EntryFilter {
        query: Some(rdrs::models::entry::query::parse("is:unread rust").unwrap()),
        ..Default::default()
    };
    let rows = rdrs::models::entry::list_by_user(
        &db, user_id, &filter, rdrs::models::entry::EntrySortOrder::PublishedAt, 50, 0,
    ).await.unwrap();
    assert!(rows.iter().any(|r| r.entry.title.as_deref() == Some("rust news")));
}

#[tokio::test]
async fn pg_query_after_date() {
    let Some(db) = pg_db_from_env().await else { return };
    // seed "old" (2025) + "new" (2026)
    let filter = rdrs::models::entry::EntryFilter {
        query: Some(rdrs::models::entry::query::parse("after:2026-01-01").unwrap()),
        ..Default::default()
    };
    let rows = rdrs::models::entry::list_by_user(
        &db, user_id, &filter, rdrs::models::entry::EntrySortOrder::PublishedAt, 50, 0,
    ).await.unwrap();
    assert!(rows.iter().any(|r| r.entry.title.as_deref() == Some("new")));
    assert!(!rows.iter().any(|r| r.entry.title.as_deref() == Some("old")));
}
```

Replace `pg_db_from_env` / seed helpers / the crate name (`rdrs`) with whatever the existing file uses — read the top of `tests/postgres_test.rs` first and copy its conventions verbatim (including how it exposes `query`/`EntryFilter`; you may need `pub mod query;` to already be `pub`, which Task 1 ensures).

- [ ] **Step 2: Run to verify (skips without a DSN)**

Run: `cargo nextest run --test postgres_test 2>&1 | tail -20`
Expected: PASS-or-skip locally (no DSN → the guard returns early). On the CI Postgres lane they execute for real.

If you have a local Postgres, set the env DSN the file expects (grep the file for the var name) and run again to see them actually exercise the PG path.

- [ ] **Step 3: Update the docs**

Edit `ARCHITECTURE.md` — find the search/entry-filter description and update it from "single case-insensitive `LIKE '%q%'` over title + content" to note that the **global `/search`** page now accepts a boolean query language (`is:`/`feed:`/`category:`/`title:`/`author:`/`before:`/`after:`, `AND`/`OR`/`NOT`, grouping, quoting, `-` negation) parsed by `models/entry/query.rs` into a `QueryNode` AST and rendered to SQL by `filters::render_query`; backend matching remains `LIKE`/`ILIKE` (no FTS); scoped views still use the plain-substring `search` field. Keep it to 2-4 sentences consistent with the file's style.

- [ ] **Step 4: Verify build + full suite**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo nextest run 2>&1 | tail -20`
Expected: fmt clean, no clippy warnings, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/postgres_test.rs ARCHITECTURE.md
git commit -m "test(search): Postgres parity for query filters; docs update"
```

---

## Self-Review

**Spec coverage:**
- §2 grammar/semantics → Task 1 (lexer + parser + all semantic rules; tests cover precedence, implicit AND, `-` tightness, `field:` disambiguation, quoting, dates, errors). ✅
- §3 AST + `EntryFilter.query` + `parse()` → Task 1 (AST, `parse`, `free_text_terms`) + Task 2 (field). ✅
- §4 AST→SQL via Dialect, LIKE escaping, joins, AND-combine → Task 2. ✅
- §5 error handling (GET, no FlashRedirect, error banner, no results) → Task 4. ✅
- §6 `<details>` syntax-help panel → Task 4. ✅
- §7 indexing (no new index; verify PG parity) → no index tasks (by design); PG parity verified in Task 5. ✅
- §8 test plan (parser unit, SQL-shape both dialects, SQLite integration, PG env-gated, handler) → Tasks 1-5. ✅
- §9 docs (ARCHITECTURE.md) → Task 5. ✅

**Placeholder scan:** Integration/handler/PG tests intentionally defer to the repo's existing fixture helpers (named explicitly with instructions to copy sibling patterns verbatim) rather than inventing parallel harnesses — this is a real instruction, not a TBD. All production code is spelled out in full.

**Type consistency:** `QueryNode` variant names, `TextField`/`SourceKind`/`Status`/`DateBound`, `parse`, `free_text_terms`, `render_query(node, binds, dialect)`, `ci_like_esc`, `like_contains`, and `EntryFilter.query` are used identically across Tasks 1-5. `render_query` signature matches its call site in `apply_filter_conditions`. Bind numbering uses `binds.len() + 1` consistently with the existing builders.
