# Composite Cursor Pagination — Design

**Issue:** [#164](https://github.com/henry40408/rdrs/issues/164) — paginate `stream/contents` with `(sort_ts, id)` composite cursor instead of bare `e.id < c`.

## Problem

The current pagination cursor is the **id** of the last visible entry, with the next-page predicate `e.id < ?`. The query's `ORDER BY` is the sort field (`COALESCE(published_at, created_at)`, `read_at`, or `starred_at`) with `e.id DESC` as a tiebreaker. The cursor only works when id-order correlates with sort-field order — which breaks under:

- OPML / backfill imports that insert older entries with new (high) ids.
- Feeds that publish back-dated or future-dated items.
- `read_at` / `starred_at` sorts where the user marks/stars entries out of publish order.

When correlation breaks, `e.id < ?` **silently skips** entries that have a higher id but a sort-timestamp older than the boundary. PoC measurement (200K rows, 5% out-of-order injection) shows the legacy cursor sees only **96K / 200K (48%)** entries on a full walk.

## Solution

Switch the cursor to a composite of `(sort_ts, id)` so it remains correct regardless of insertion order.

### Cursor format

Encode in the existing opaque string field as:

```
<iso_8601_ts>|<id>
```

- `iso_8601_ts` — the entry's sort-field value (TEXT, the same format SQLite stores in `published_at` / `read_at` / `starred_at`).
- `id` — entry id as decimal integer.
- Separator `|`. Not `:` (issue body's suggestion) — ISO 8601 contains `:`, which makes splitting ambiguous.

For `PublishedAt` sort, `iso_8601_ts` is the value of `COALESCE(published_at, created_at)` (i.e. whichever timestamp was used for ordering).

### SQL predicate

Use the **bounded-OR** form (PoC v4 confirmed this is the only form SQLite's planner can convert to an indexed range scan when the column is an expression like `COALESCE(...)`):

```sql
-- DESC sort (newest first)
sort_ts_expr <= ?ts
  AND (sort_ts_expr < ?ts OR e.id < ?id)

-- ASC sort (oldest first)
sort_ts_expr >= ?ts
  AND (sort_ts_expr > ?ts OR e.id > ?id)
```

Where `sort_ts_expr` matches the existing `ORDER BY`:

| `EntrySortOrder` | `sort_ts_expr` |
|---|---|
| `PublishedAt` (default) | `COALESCE(e.published_at, e.created_at)` |
| `ReadAt` | `e.read_at` |
| `StarredAt` | `e.starred_at` |

PoC measurement (200K rows, 50-row page, mid-table cursor):

| Path | Today (legacy `id<?`) | #164 bounded-OR |
|---|---|---|
| `ReadAt` (plain column) | 0.47 ms | **0.013 ms** |
| `PublishedAt` (`COALESCE`, current schema) | 5.3 ms | 12.7 ms ⚠️ |
| `PublishedAt` (`COALESCE`, with new expr index) | 2.4 ms | **0.017 ms** |

### New index

To avoid regressing the `PublishedAt` path, add an expression index:

```sql
CREATE INDEX IF NOT EXISTS idx_entry_sort_ts
    ON entry(COALESCE(published_at, created_at));
```

Rationale: SQLite's query planner cannot use `idx_entry_published_at` for `WHERE COALESCE(published_at, created_at) < ?`. With the expression index, the bounded-OR predicate becomes `SEARCH entry USING COVERING INDEX idx_entry_sort_ts (<expr><?)` — an indexed range scan. As a side benefit, the **legacy** path (still used during the grace period) also speeds up from 5.3 ms → 2.4 ms.

This is added as a startup-time `CREATE INDEX IF NOT EXISTS` in `src/db/schema.rs`, mirroring the existing pattern. No formal migration framework is needed; SQLite builds the index on first run.

### Backwards compatibility

The continuation field is opaque to clients (the SPA round-trips it; external GReader clients also treat it opaque). However, in-flight cursors may exist in browser URLs / JS state at deploy time. Parse rules:

| Cursor value | Action |
|---|---|
| Contains `\|` | Parse as `<iso_8601_ts>\|<id>` → use composite predicate (V2 bounded OR) |
| Bare `i64` (no `\|`) | Best-effort fallback: parse as id, use legacy `e.id < ?` predicate |
| Anything else | Treat as no cursor (return first page) |

Next-page emission **always** uses the new format. The grace path can be deleted in a follow-up release.

## Affected components

| File | Change |
|---|---|
| `src/db/schema.rs` | Add `CREATE INDEX IF NOT EXISTS idx_entry_sort_ts` on `COALESCE(published_at, created_at)` |
| `src/models/entry.rs` | Replace `ContinuationParams.continuation_id: Option<i64>` with a richer `Option<ContinuationCursor>` enum (`Composite { sort_ts, id }` \| `LegacyId(i64)`); rewrite `apply_continuation_condition` to emit V2 bounded-OR using the right `sort_ts_expr` per `sort_order` |
| `src/handlers/greader/item.rs` | Two cursor-handling sites (`stream_contents` line 47, `stream_item_ids` line 173): parse `c` query param → `ContinuationCursor`; emit new format from last visible entry |
| `src/handlers/pages.rs` | `fetch_entries_for_ssr_with_sort`: same parse + emit changes |

No DB column changes. No API contract changes (cursor is opaque). No new dependencies.

## Test plan

### Unit tests (`src/models/entry.rs`)
- New test: `list_by_user_with_continuation` walks all entries with composite cursor under non-monotonic `id ↔ published_at` data; asserts no skips and no duplicates.
- New test: same for `ReadAt` and `StarredAt` sorts with non-monotonic timestamps.
- New test: legacy bare-i64 cursor still works (grace path).

### Integration tests
- `tests/pages_test.rs`: SSR Load More on a seeded fixture with old-timestamp / high-id rows — assert no skip across page boundary.
- `tests/greader_test.rs` (if present, otherwise extend item handler test): same for `stream/contents`.

### E2E (`e2e/tests/ssr-no-double-render.spec.ts`)
- Extend with a seeded fixture that interleaves "old timestamp, high id" entries; assert page 2 contains them.

### Performance regression guard
Not adding a benchmark to CI. PoC numbers are documented above; expression-index is the perf safety net.

## Out of scope

- Removing the `LegacyId` grace path (follow-up release once we're confident no in-flight cursors remain).
- Backfilling `published_at` to eliminate the `COALESCE` (orthogonal cleanup).
- Adding a CI perf benchmark for pagination.
- ROW VALUE tuple syntax — PoC showed planner can't use it against expression indexes.
