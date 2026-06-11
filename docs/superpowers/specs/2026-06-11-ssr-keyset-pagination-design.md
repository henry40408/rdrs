# SSR List Pages — OFFSET → Keyset Pagination — Design

**Context:** Deferred follow-up to PR #290 (DB-operations perf). Reuses the
composite-cursor machinery from [2026-04-26-composite-cursor-pagination-design.md](2026-04-26-composite-cursor-pagination-design.md)
(#164), which today serves only the Google-Reader `stream/contents` API.

## Problem

The server-rendered list pages paginate "Load More" with an **integer offset**
(`?after=<N>&fragment=1`). Each page is fetched via `entry::list_by_user(...,
LIMIT page_size+1 OFFSET N)`. SQLite must produce-and-discard all `N` rows
before returning the page, so page _k_ costs O(k · page_size). Deep scrolls on a
large inbox get progressively slower.

The keyset (composite-cursor) path that fixes this already exists
(`list_by_user_with_continuation` + `ContinuationCursor` + `fetch_sort_ts`) and
is used by the GReader API — the SSR pages simply do not use it.

### Benchmark (real crate functions, 50k entries, file-backed WAL, min-of-20)

| Page | OFFSET (current) | keyset+hint (proposed) |
|---|---|---|
| `/entries` page 0 | 0.16 ms | **0.056 ms** |
| `/entries` page 200 | 13.7 ms | **0.21 ms** (64×) |
| unread page 200 | 127.6 ms | **0.24 ms** (542×) |
| read page 200 | 13.3 ms | **0.23 ms** (58×) |

Deep pages improve 58–542×; page 0 is unchanged-to-slightly-faster.

## Solution

Route the SSR list pages through `list_by_user_with_continuation`. The only
client-visible change: the `after` form value stops being an integer offset and
becomes the existing **opaque composite cursor** (`"<sort_ts>|<id>"`). The
append-style Load-More swap and the `after` parameter *name* are unchanged.

### Mandatory prerequisite: index hints on the continuation query

`list_by_user` applies `INDEXED BY` hints via `published_sort_entry_hint`
(`idx_entry_sort_ts` / `idx_entry_starred_sort` / `idx_entry_read_sort`).
`list_by_user_with_continuation` does **not**. On the **page-0** request (no
cursor predicate), the hint-less query drops to a `category → feed → entry` scan
+ temp B-tree sort over the whole corpus.

Measured on the unfiltered `/entries` page 0 (50k entries):

| | page 0 | EXPLAIN |
|---|---|---|
| no hint | **60.5 ms** | `… USE TEMP B-TREE FOR ORDER BY` |
| `+ INDEXED BY idx_entry_sort_ts` | **0.056 ms** | `SCAN e USING INDEX idx_entry_sort_ts` |

So converting `/entries` and read pages **without** the hint would regress page 0
(the most-loaded page) ~350×. The conversion is gated on adding the same hint to
the continuation builder.

**Cost of the hint:** none at runtime. The three sort indexes already exist
(no new index, no extra write cost); the hint only directs SQLite to use an index
it already has — exactly the directive `list_by_user` already relies on. The
keyset predicate `sort_ts_expr < ?` is the textbook range-scan use of that index,
so it is a safer fit than the OFFSET case. The change is shared with GReader's
`stream/contents` and **incidentally fixes the same latent ~60 ms page-0 cost on
its first (cursorless) call.**

## Scope

In scope — every index-ordered SSR list page (7 handlers):

- `/` → `unread_page`, `/entries` → `entries_page`
- `/entries/read` → `read_entries_page`, `/entries/starred` →
  `starred_entries_page`, `/entries/summarized` → `summarized_entries_page`
- `/feeds/{id}/entries` → `feed_entries_page`, `/categories/{id}/entries` →
  `category_entries_page` (incl. their `?status=` filter tabs)

Out of scope:

- **Search** (`/entries?q=`). Its cost is the `title/content LIKE %q%` full scan,
  which cannot use the sort index; keyset would not help. Left on its current
  bounded-scan path. A separate effort if/when search perf is addressed.
- **`⑤a` is this work.** No other items from the PR #290 review.

All in-scope pages sort by `COALESCE(published_at, created_at) DESC`
(`EntrySortOrder::PublishedAt`); behavior is preserved exactly. A single cursor
scheme covers them.

## Detailed design

### 1. `build_entries_page` (the single shared builder)

`src/handlers/pages/mod.rs`. Change the parameter `offset: i64` →
`cursor: Option<ContinuationCursor>`; return type `(Vec<EntryRowView>,
Option<String>)` (cursor token instead of next offset).

```rust
let params = ContinuationParams {
    oldest_first: false,
    limit: page_size + 1,
    continuation: cursor,
    ot: None, nt: None,
    sort_order,
};
let rows = entry::list_by_user_with_continuation(conn, user_id, &filter, &params)?;
let has_more = rows.len() as i64 > page_size;
let kept = &rows[..rows.len().min(page_size as usize)];
let next_cursor = if has_more {
    kept.last()
        .and_then(|e| entry::fetch_sort_ts(conn, e.entry.id, sort_order).ok().flatten()
            .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id)))
} else { None };
```

This is a line-for-line mirror of `greader/item.rs::stream_contents`.

### 2. Index hint on `list_by_user_with_continuation`

`src/models/entry/mod.rs`. Apply `published_sort_entry_hint(filter)` (already
exists, currently used only by `list_by_user`) to the continuation query's
`FROM entry e{hint}` for the `PublishedAt` sort order — identical to how
`list_by_user` does it. The `ReadAt` / `StarredAt` sort orders (GReader-only) keep
their current no-hint behavior (no dedicated index exists for them; unchanged).

### 3. Query + template types

- `EntriesQuery.after`: `Option<i64>` → `Option<String>`. Parse with
  `ContinuationCursor::parse` (returns `None` for empty/garbage → first page).
  Same change for the feed / category page query structs that carry `after`.
- Every page-template struct's `next_cursor` field: `Option<i64>` →
  `Option<String>` (`UnreadTemplate`, `EntriesFragmentTemplate`, the read/starred/
  summarized/feed/category templates).
- Templates `_entries_layout.html` / `_entries_fragment.html` are unchanged:
  `name="after" value="{{ after }}"` already renders a string.

### 4. Client JS

No change. `app.js` treats `after` as an opaque hidden input it echoes back; the
append-style multi-target swap is preserved.

## Behavior & edge cases

- **Snapshot (#289) decoupled.** `read_after` is only threaded into the neighbors
  API, never into Load-More (`grep` confirmed). Keyset is strictly more robust:
  reading entries mid-session no longer shifts an offset and skips unread items (a
  latent OFFSET bug).
- **Back-compat.** An in-flight integer `after=50` from a page rendered just
  before deploy parses as `ContinuationCursor::LegacyId(50)` — the same one-time
  grace path the GReader cursor already handles. After one Load-More it becomes a
  composite cursor.
- **Unparseable / absent cursor** → first page (graceful).
- **Deep-page semantics** improve: new entries arriving at the top between page
  loads no longer cause the offset to duplicate/shift rows; the cursor continues
  strictly after the last `sort_ts`.

## Testing

- Unit: `build_entries_page` returns a composite `next_cursor` and the next call
  with that cursor returns the following page with no overlap/gap (seed > 2 pages).
- Unit: hint applied — `list_by_user_with_continuation` page-0 EXPLAIN uses
  `idx_entry_sort_ts` (mirror the existing `test_init_db_*_index_exists` style, or
  assert plan via `EXPLAIN QUERY PLAN`).
- Update `tests/pages_test.rs` assertions that currently expect an integer
  `after` in the Load-More form to expect a composite-cursor token.
- e2e `reading.feature` / Load-More steps: unchanged — the button/flow are the
  same; the cursor value is opaque to them. Confirm they still pass.

## Verification (per the perf rule)

Before/after benchmark of all four page shapes (unread / all / read / starred) at
page 0 and depth, on the **implemented** code, captured and compared. No
conversion lands if any page shape regresses at page 0.

## Risks

- **`INDEXED BY` is a hard directive.** Removes planner freedom. Mitigated:
  `list_by_user` already accepts this exact tradeoff with the same indexes; the
  keyset predicate is the ideal range-scan use of the index; the schema comments
  document why the hint is correct for this single-user-owns-everything workload.
- **Shared function blast radius.** `list_by_user_with_continuation` is used by
  GReader; adding the hint changes its plan. This is a benefit (fixes GReader's
  latent page-0 cost) but the GReader tests must stay green — covered by the
  existing `greader_test.rs` suite plus the new plan assertion.

## Non-goals

- Changing sort order of any page.
- Touching Search pagination.
- Removing the `after` parameter name or the append-style Load-More mechanism.
