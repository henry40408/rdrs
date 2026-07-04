# Scoped Search — Design

**Date:** 2026-07-05
**Branch:** `feat/scoped-search`
**Status:** Approved, ready for implementation planning

## Goal

Let a user search for entries by keyword **within a single category or feed**, then
either **mark the matching entries as read** or **read through them** (the filtered
list is itself the focused reading set). Example: inside the "動畫" category, search
"超少女", get the matching entries, and mark them all as read in one action.

The work also corrects a latent flaw in the existing global `/search`: search currently
matches raw HTML, which both over-matches (hits inside tags/attributes — "phantom
matches") and under-matches (a term split across tags like `超<b>少女</b>` is never a
candidate). We fix this at the source by matching against stored plain text.

## Non-Goals (YAGNI / possible follow-ups)

- No search box on the global lists (Unread / All / Starred / Summarized). Scoped
  search is category/feed only for this iteration.
- No change to the global `/search` page's UI/shape — only its matching foundation and
  a handler simplification.
- No FTS5. For CJK substring search, `LIKE %term%` over plain text is the right tool;
  FTS5 would need a trigram tokenizer and is a far larger change.

## Key Decisions (from brainstorming)

1. **Entry point:** a search box added to the existing category/feed entries pages,
   scoped to the current category/feed. Reuses the entries layout, reading pane, and
   the mark-as-read controls.
2. **Match fields:** `title` + `content` (plain text). Not `summary`, `author`, or
   `link`.
3. **Plain-text strategy:** store a new `entry.content_text` column (HTML stripped via
   the existing `strip_to_plain_text`) and match `LIKE` against it. Applies to both the
   new scoped search and the existing `/search`.
4. **Search interaction:** server-side filtering, triggered by a **debounced
   auto-submit** (~250ms) that swaps the list; falls back to Enter/submit without JS.
5. **Mark matching as read:** a **server-side** action that marks *all* entries
   matching (scope + search) as read — not just the rows currently loaded on screen.

## Architecture

Three parts, buildable in order. Part A is a prerequisite for the correctness of both
`/search` and scoped search.

### Part A — Plain-text search foundation

**A1. Shared stripper.** Move `strip_to_plain_text` out of
`src/handlers/pages/search_text.rs` (currently a private fn) into `src/utils/text.rs`
as `pub`. Rationale: the sync service must not import from a handler module (layering).
`search_text.rs` and the sync path both import it. `build_snippet` / `highlight_html`
stay where they are and call the shared util.

**A2. Schema migration v10** (`src/db/schema.rs`, bump `LATEST_VERSION` 9 → 10):
- Add `content_text TEXT` to the `CREATE TABLE entry` block (for fresh databases).
- Add an `if version < 10 { … }` block: `ALTER TABLE entry ADD COLUMN content_text
  TEXT`, then **batch-backfill** existing rows (`SELECT id, content` → `strip` →
  `UPDATE entry SET content_text = ? WHERE id = ?`). `entry` is the largest table;
  backfill in batches to avoid one long transaction.
- Model the block on the v4 (`feed.bucket`) migration, which is the canonical
  add-column-plus-backfill template.

**A3. Populate on sync.** Add a `content_text` parameter to `upsert_entry_id`
(`src/models/entry/mod.rs`); include it in both the UPDATE `SET` list and the INSERT
column/SELECT lists (both use `prepare_cached`, so update the cached SQL text).
`feed_sync.rs` computes `content_text` from the derived `content` (near line 310) and
passes it in. Update the `upsert_entry` wrapper accordingly.

**A4. Change the search predicate** (`src/models/entry/filters.rs:81`):
`(e.title LIKE ? … OR e.content LIKE ? …)` → `(e.title LIKE ? … OR e.content_text LIKE
? …)`. This is the single line that fixes both phantom matches and tag-spanning misses.
`content_text` is `NULL` for rows not yet backfilled/synced; `LIKE` on `NULL` yields no
match, and `title` still matches — acceptable, and the migration backfills existing
rows anyway.

**A5. Simplify `search_page`** (`src/handlers/pages/mod.rs:1666`). Because the SQL now
matches true plain text, remove the OFFSET-paged phantom-filter loop (the
`TARGET/BATCH/MAX_ITERATIONS/MAX_SCANNED` scan). Replace with a single
`entry::list_by_user(..., 50, 0)` query; pagination/count become exact. `build_snippet`
is still used to render each result's snippet.

### Part B — Scoped search on category / feed pages

**B1. Query param.** Add `q: Option<String>` to `EntriesQuery`
(`src/handlers/pages/mod.rs:305`). In `category_entries_page` and `feed_entries_page`,
set `filter.search = query.q.clone().filter(|s| !s.trim().is_empty())`. Because
`build_entries_page` already consumes the `EntryFilter`, the paginated list picks up the
`LIKE` predicate with no query-layer changes.

**B2. Layout context + template.** Add to `EntriesLayoutContext`
(`src/handlers/pages/mod.rs:162`):
- `search: Option<String>` — the current `q`, for prefilling the box and hidden inputs.
- `search_action: Option<String>` — the form action path; `Some` only for category/feed
  pages, which is what gates rendering the search box (so it never appears on global
  lists).

In `templates/_entries_layout.html`, when `search_action` is `Some`, render a GET
`<form data-swap="[data-entries-list]">` in the filter-bar with a text input prefilled
from `search`. The form also carries a hidden `status` input set to the active tab, so
searching respects the current unread/read/all filter rather than resetting it. Add a
hidden `q` input to the Load-More form (mirroring how `after` / `fragment` / `status`
are already forwarded). Thread `q` into `EntriesFragmentTemplate` too, so fragment loads
keep the filter.

**B3. Debounced auto-submit** (`static/js/`). Add a small `debounce` helper to
`utils.js`. Add `installEntriesSearch()` to `app.js`: on `input` of the search box,
debounce ~250ms, then `form.requestSubmit()`. The existing `installSwap` GET-form path
serializes the form to the query string and swaps `[data-entries-list]`. Re-init on the
`rdrs:swap-complete` event (the established convention). The search box lives **outside**
the swapped `[data-entries-list]` container, so focus and caret position survive swaps.
Without JS, Enter/submit still navigates.

### Part C — Mark matching as read (server-side)

**C1. Model fn.** `mark_read_by_filter(conn, user_id, &EntryFilter) -> AppResult<i64>`
in `src/models/entry/mod.rs`. Reuse `apply_filter_conditions` (it emits `e.` / `c.`
aliases and assumes `?1 = user_id`), seeding `conditions = ["c.user_id = ?1"]` and
`params_vec = [user_id]` exactly as `list_by_user` does. Shape it like
`mark_all_read_by_user`:

```sql
UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now')
WHERE id IN (
  SELECT e.id FROM entry e
  INNER JOIN feed f ON e.feed_id = f.id
  INNER JOIN category c ON f.category_id = c.id
  WHERE {where_clause}
) AND read_at IS NULL
```

Returns the affected-row count.

**C2. Routes + handler.** `POST /categories/{id}/entries/mark-read` and
`POST /feeds/{id}/entries/mark-read` (shared handler core). Body carries `q`. Build the
scoped `EntryFilter` (category_id/feed_id + search), call `mark_read_by_filter`, then
respond with `FlashRedirect` back to the originating page (preserving `q`) and call
`emit_sidebar` so unread counts refresh.

**C3. Template button.** When `search` is non-empty, render a "Mark N matching as Read"
submit button (a small POST form with a hidden `q`). `N` comes from
`count_by_user(&filter)` — one extra count query, only when a search is active. The
existing "Mark as Read… (older than)" age dropdown is unchanged and still targets the
whole scope.

## Data Flow

```
User types "超少女" in the 動畫 category search box
  → debounce 250ms → GET /categories/{id}/entries?q=超少女&status=unread (data-swap)
  → category_entries_page sets filter.category_id + filter.search
  → build_entries_page → list_by_user_with_continuation
       WHERE c.user_id=? AND e.feed_id∈(cat) AND read_at IS NULL
             AND (e.title LIKE %超少女% OR e.content_text LIKE %超少女%)
  → swap [data-entries-list] with matching rows; search box keeps focus
  → j/k now navigate only the matching rows (focused reading, for free)

User clicks "Mark N matching as Read"
  → POST /categories/{id}/entries/mark-read  (q=超少女)
  → mark_read_by_filter(user_id, {category_id, search})
  → FlashRedirect back with ?q=超少女 ; emit_sidebar
```

## Error Handling / Edge Cases

- Empty/whitespace `q` → `filter.search = None`; behaves like the plain category/feed
  list. Search box renders empty.
- `content_text` `NULL` (pre-backfill / content-less entries) → no content match; title
  still matches. Migration backfills existing rows; sync always sets it going forward.
- `content` is `NULL` but `summary` has text → `content_text` is `NULL`; not searched.
  This matches the "title + content, not summary" decision.
- Ownership is always enforced by the `c.user_id = ?1` seed and the join chain, in both
  the list query and `mark_read_by_filter`.
- Sanitization / SSRF / image-proxy paths are untouched.

## Testing

- **Unit (`utils/text.rs`):** `strip_to_plain_text` keeps its existing coverage via
  `build_snippet`; add a direct test if the move warrants it.
- **Model (`filters` / `entry`):** search now matches `content_text`; add a tag-spanning
  case (content HTML `超<b>少女</b>`, `content_text = "超少女"`, query "超少女" matches;
  a phantom case where the term appears only in an `href` does *not* match).
  `mark_read_by_filter` marks only rows that are matching **and** owned by the user
  **and** currently unread; returns the correct count.
- **Schema:** migration test asserts `user_version == 10`; a v9→v10 upgrade adds
  `content_text` and backfills.
- **Handler:** `category_entries_page` / `feed_entries_page` with `?q=` filter the list;
  the mark-read action marks all matching entries across pages (not just the loaded
  page).
- **E2E (BDD, `e2e/`):** open a category, type a keyword, assert the list narrows to
  matches, click "Mark matching as Read", assert those entries leave the unread set.
- **Screenshots:** the search box appears only on category/feed pages; the four README
  screenshots (unread-list, keyboard-help) are unaffected — **no regeneration needed**.

## Cost / Trade-offs Accepted

- `content_text` roughly doubles per-entry content storage. Accepted in exchange for
  correct, fast, plain-text `LIKE` matching and a simpler search handler.
- One extra `count_by_user` query per page render when a search is active (for the
  "Mark N matching" label).

## Touched Files (map)

- `src/utils/text.rs` (new / moved `strip_to_plain_text`)
- `src/db/schema.rs` (migration v10, `content_text` column)
- `src/models/entry/mod.rs` (`upsert_entry_id` param, `mark_read_by_filter`)
- `src/models/entry/filters.rs` (predicate → `content_text`)
- `src/services/feed_sync.rs` (compute + pass `content_text`)
- `src/handlers/pages/mod.rs` (`EntriesQuery.q`, `EntriesLayoutContext` fields,
  category/feed handlers, `search_page` simplification, mark-read handler)
- `src/handlers/pages/search_text.rs` (import the moved stripper)
- `src/lib.rs` (register mark-read routes)
- `templates/_entries_layout.html` (search form, hidden `q`, mark-matching button)
- `static/js/utils.js` (`debounce`), `static/js/app.js` (`installEntriesSearch`)
- tests + one E2E `.feature`
