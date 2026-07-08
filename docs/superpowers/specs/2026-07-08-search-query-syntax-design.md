# Search Query Syntax — Design Spec

- **Date:** 2026-07-08
- **Status:** Approved (pending implementation)
- **Scope:** Introduce a boolean query-language parser for the global `/search`
  page. Backend matching stays `LIKE`/`ILIKE` (no full-text search this round).

## 1. Motivation & Current State

Today "search" is a single free-text string matched as a case-insensitive
substring (`LIKE '%q%'` on SQLite, `ILIKE` on Postgres) against `entry.title`
and `entry.content_text` only (`src/models/entry/filters.rs:207-217`). There is
**no query grammar** — no field operators, booleans, quoting, or negation.
Structured filters (`unread_only`, `starred_only`, `feed_id`, `category_id`, …)
are populated from separate query params / routes, never parsed from the search
text.

This spec adds a real query language so users can express, in one search box:
field filters, boolean logic, grouping, negation, and quoted phrases — parsed
into the existing structured filtering layer.

**Non-goals (explicitly out of scope):**
- Full-text search (FTS5 / `tsvector`). Free-text remains a `%v%` scan.
- Applying the syntax to the scoped per-view search (`_entries_layout.html`)
  or category/feed pages. Global `/search` only.
- A third-party parser dependency (`winnow`/`chumsky`). Hand-written, zero deps.

## 2. Grammar (EBNF) & Semantics

Operator precedence, high → low: `NOT` > `AND` (implicit or explicit) > `OR`.
Parentheses override. Adjacent terms with no operator are an implicit `AND`.

```ebnf
query    = or_expr ;
or_expr  = and_expr { "OR" and_expr } ;
and_expr = not_expr { [ "AND" ] not_expr } ;   (* omitted operator = implicit AND *)
not_expr = { "NOT" | "-" } atom ;              (* NOT keyword or tight "-" negates *)
atom     = "(" or_expr ")" | filter | text ;
filter   = field ":" value ;
field    = "is" | "feed" | "category" | "title" | "author" | "before" | "after" ;
value    = quoted | bare ;
text     = quoted | bare ;
quoted   = '"' { char - '"' } '"' ;
bare     = { char - ( space | '(' | ')' | '"' ) } ;
```

### Semantic rules

| Rule | Behavior |
|---|---|
| Keyword case | `AND`/`OR`/`NOT` are case-insensitive operators. To search them literally, quote: `"and"`. |
| Field-name case | Field names (`is`, `feed`, …) are case-insensitive. |
| `field:` trigger | A `token:` is a filter **only** when `token` is a known field name; otherwise the whole token (colon included) is free text. `http://example.com` → `http` is unknown → literal text. |
| Free text | Matches `title OR content_text` (unchanged from today). |
| `title:` / `author:` | Single-column case-insensitive substring. `author` is **newly** searchable. |
| `is:` | `unread` → `read_at IS NULL`; `read` → `read_at IS NOT NULL`; `starred` → `starred_at IS NOT NULL`. Unknown value → `ParseError`. |
| `feed:` / `category:` | Case-insensitive substring on `feed.title` / `category.name`. One term may match multiple sources (implicit OR over the matched set). |
| `before:` / `after:` | Value is `YYYY-MM-DD`, interpreted in **UTC** (the app pins `TimeZone=UTC`). `after:D` → `COALESCE(published_at, created_at) >= D 00:00`; `before:D` → `< D 00:00`. |
| `-` negation | `-` negates only when **tight** against an atom (no whitespace) at an atom position: `-is:read`, `-rust`, `-(a OR b)`. `cross-platform` is one text token (`-` mid-token). `- rust` (space) → `-` is a literal text token. Literal `-rust` → quote it: `"-rust"`. |
| Value quoting | Values with spaces need quotes: `feed:"Rust Blog"`, `title:"exact words"`. |

### Examples

- `is:unread rust` → unread AND (title/content contains rust)
- `is:starred (rust OR go) -is:read` → starred AND (rust OR go) AND NOT read
- `feed:"Hacker News" after:2026-01-01 title:rust`

## 3. AST & Parser (`src/models/entry/query.rs`, new module)

Hand-written tokenizer + recursive-descent parser + AST, ~250–350 lines,
zero new dependencies (`chrono` — already a dependency — validates dates).

```rust
pub enum QueryNode {
    And(Box<QueryNode>, Box<QueryNode>),
    Or(Box<QueryNode>, Box<QueryNode>),
    Not(Box<QueryNode>),
    Text(String),                               // free text → title OR content_text
    Field { field: TextField, value: String },  // TextField::{Title, Author}
    Source { kind: SourceKind, value: String },  // SourceKind::{Feed, Category}
    Status(Status),                             // Status::{Unread, Read, Starred}
    Date { bound: DateBound, date: NaiveDate },  // DateBound::{Before, After}
}

pub struct ParseError { pub position: usize, pub message: String }

pub fn parse(input: &str) -> Result<QueryNode, ParseError>;
```

- **Tokenizer** emits `Word / Quoted / LParen / RParen / And / Or / Not / Minus /
  FieldColon(field)`, each carrying its byte offset (for error positioning).
- **Parser** is four mutually-recursive functions mirroring `or_expr /
  and_expr / not_expr / atom`.
- **Date validation** happens at parse time via
  `NaiveDate::parse_from_str(v, "%Y-%m-%d")`; failure → `ParseError`.

`EntryFilter` (`src/models/entry/mod.rs:59`) gains one field:

```rust
pub query: Option<QueryNode>,   // global /search only; existing `search: Option<String>` unchanged for scoped views
```

## 4. AST → SQL (`src/models/entry/filters.rs`)

New recursive function alongside `apply_filter_conditions`, dispatching through
the existing `Dialect` seam:

```rust
fn render_query(node: &QueryNode, dialect: Dialect, binds: &mut Vec<Bind>) -> String
```

Produces a parenthesized WHERE fragment; each leaf pushes its binds. SQLite
shown (Postgres uses `ILIKE` and its own epoch form):

| Node | SQL fragment |
|---|---|
| `And(a,b)` | `(<a> AND <b>)` |
| `Or(a,b)` | `(<a> OR <b>)` |
| `Not(a)` | `(NOT <a>)` |
| `Text(t)` | `COALESCE((e.title LIKE $n ESCAPE '\' OR e.content_text LIKE $n ESCAPE '\'), <false>)` |
| `Field{Title,v}` | `COALESCE(e.title LIKE $n ESCAPE '\', <false>)` |
| `Field{Author,v}` | `COALESCE(e.author LIKE $n ESCAPE '\', <false>)` (author newly searched) |
| `Source{Feed,v}` | `COALESCE(f.title LIKE $n ESCAPE '\', <false>)` |
| `Source{Category,v}` | `COALESCE(c.name LIKE $n ESCAPE '\', <false>)` |
| `Status(Unread)` | `e.read_at IS NULL` (read → `IS NOT NULL`; starred → `e.starred_at IS NOT NULL`) |
| `Date{After,d}` | `COALESCE(e.published_at, e.created_at) >= <epoch(d)>` (before → `<`) |

Postgres uses `ILIKE`; SQLite's `LIKE` is already ASCII-case-insensitive, so no
`COLLATE NOCASE` suffix is emitted.

- **Null-safe leaves:** each `LIKE` leaf is wrapped in `COALESCE(<expr>, <false>)`
  (`<false>` = `0` on SQLite, `FALSE` on Postgres) so a `NULL` column reads as
  `FALSE`, not `NULL`. This makes the predicate two-valued: positive matches are
  unaffected, and a negated filter such as `-author:jane` correctly *includes*
  entries whose `author` is `NULL` instead of silently dropping them.
- **LIKE wildcard escaping:** user-supplied `%` / `_` / `\` are escaped and an
  explicit `ESCAPE '\'` clause is emitted, so a search for a literal `%` is a
  literal. (The current single-string path does *not* escape — a pre-existing
  minor bug we do not carry into the new path.)
- **Joins:** `feed`/`category` predicates rely on the `f`/`c` aliases already
  joined by `list_by_user`. If a backend's query lacks the join, add it.
- When the global `/search` handler sets `filter.query`, `render_query`'s output
  is `AND`-combined into the WHERE clause. The legacy `filter.search` path is not
  used on the global page; scoped views keep using it unchanged.

## 5. Error Handling & UI Wiring

`/search` is a `GET` page (not a POST → no `FlashRedirect`). In `search_page`
(`src/handlers/pages/mod.rs:1735`):

```
q empty            → current behavior (no search)
parse(q) == Ok(a)  → EntryFilter.query = Some(a) → normal listing
parse(q) == Err(e) → do NOT run the query; render the page with
                     error = Some("Search syntax error (near character {char_pos}): {e.message}")
```

- `search.html` gains an `error: Option<String>` field; when `Some`, an error
  banner renders below the input and the results area stays empty.
- Messages are English, to match the English `/search` UI. Examples:
  `Unbalanced parentheses: '(' is not closed`, `'before:' expects a date like
  YYYY-MM-DD`, `Expected a search term here`. (`char_pos` is the 1-based
  character position derived from the parser's byte offset `e.position`.)
- **Screenshot impact:** the error banner only appears on invalid input; the
  four README screenshots (unread list + reading pane; keyboard-help overlay)
  do not include `/search`, so no regeneration is required. The search input
  `placeholder` is left unchanged this round.

## 6. Discoverability — Syntax Help Panel

Users must be able to discover the syntax without prior knowledge. Reuse the
existing native `<details>/<summary>` disclosure pattern
(`templates/feed_edit.html:36-38`, CSS `app.css:2010-2019`).

- Add a collapsed-by-default `<details class="search-syntax-help">` beneath the
  `/search` input in `templates/search.html`. `<summary>` reads e.g. "Search
  syntax". Expanded body lists each operator with a one-line example
  (`is:unread`, `feed:"…"`, `title:` / `author:`, `before:` / `after:`,
  `AND`/`OR`/`NOT`, `-`, quoting).
- Reuses existing disclosure CSS; a small amount of list styling may be added
  under a new `.search-syntax-help` selector in `app.css`.
- Not added to the `?` keyboard-help overlay (that overlay is keyboard
  shortcuts, and it *is* captured in screenshots — changing it would force a
  screenshot regen). This is listed as an optional follow-up.
- `/search` is not among the four README screenshots, so the panel does not
  require screenshot regeneration.

## 7. Indexing & Performance

**Conclusion: no new index is added, by design.** Verified against
`migrations/sqlite/0001_initial.sql:79-94`.

| New predicate | Index needed | Present today |
|---|---|---|
| `is:unread` / `is:read` (`read_at IS [NOT] NULL`) | `read_at` | ✅ `idx_entry_read_at` + partial `idx_entry_unread_sort` / `idx_entry_read_sort` |
| `is:starred` (`starred_at IS NOT NULL`) | `starred_at` | ✅ `idx_entry_starred_at` + `idx_entry_starred_sort` |
| `before:` / `after:` (`COALESCE(published_at, created_at)` compare) | expression index on that COALESCE | ✅ `idx_entry_sort_ts` on `COALESCE(published_at, created_at)` — same expression the `ORDER BY` already uses. Note: the emitted predicate wraps it in an epoch cast (`CAST(strftime('%s', COALESCE(...)) AS INTEGER)`), so the WHERE clause is not a verbatim index seek; this matches the pre-existing `apply_time_conditions` pattern, so there is no regression and no new index is warranted. |
| `feed:` / `category:` (`f.title` / `c.name LIKE '%v%'`) | none | feed/category are tiny per-user tables; scan is negligible |
| free text / `title:` / `author:` (`LIKE '%v%'`) | **none possible** | leading-wildcard LIKE cannot use a B-tree index — unchanged full scan from today; only FTS (out of scope) would help |

Rationale:
- Index-able predicates (status, date) hit existing indexes → no regression.
- Non-index-able predicates are leading-wildcard `LIKE` scans; adding indexes
  would **not** help and would only slow writes, so we deliberately add none.
  This is the same cost profile as today's search.
- **Verification item:** confirm `migrations/postgres/0001_initial.sql` mirrors
  these indexes (multi-db parity).

## 8. Testing Plan (TDD)

1. **Tokenizer / parser unit tests** (`query.rs` `#[cfg(test)]`):
   precedence (`NOT` > `AND` > `OR`), implicit AND, parentheses; `-` tightness
   (`-is:read` negates, `cross-platform` unaffected, `- rust` literal); `field:`
   disambiguation (`feed:rust` filter vs `http://x` text); quoted values and
   literal `"and"`; date parse (valid + `before:yesterday` error); error paths
   (unbalanced parens, `OR` missing operand, trailing operator).
2. **SQL-shape tests** (`filters.rs`, mirroring existing dialect tests
   `319-421`): assert the WHERE fragment for representative ASTs, **SQLite and
   Postgres each** (`COALESCE(… LIKE … ESCAPE '\', 0)` vs `COALESCE(… ILIKE …
   ESCAPE '\', FALSE)`; both epoch forms).
3. **Integration tests** (extend the search test module in `entry/mod.rs`,
   against real SQLite): seed feeds/categories/entries; verify result sets for
   each operator and boolean/negation combos; verify `%` literal escaping.
4. **Postgres path** (env-gated `tests/postgres_test.rs`): 1–2 query cases
   validating `ILIKE` + epoch.
5. **Handler test** (`/search`): invalid query → error banner, no results;
   valid query → results.

## 9. Documentation

Per repo convention (update existing, do not create new docs beyond this spec):
- Update `ARCHITECTURE.md`'s search description (single `LIKE` substring →
  global `/search` boolean query syntax).
- This spec is the syntax reference.

## 10. Optional Follow-ups (not this round)

- `-` short-negation is included; FTS5 / `tsvector` relevance search is not.
- Syntax help in the `?` keyboard-help overlay.
- Apply the syntax to scoped / category / feed search surfaces.
- Search-input `placeholder` example text.
