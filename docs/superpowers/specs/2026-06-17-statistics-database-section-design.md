# Statistics: admin-only "Database" section

**Date:** 2026-06-17
**Status:** Approved (design)
**Branch:** `feat/statistics-database-section`

## Summary

Add a new **Database** section to the `/statistics` page that surfaces
database storage and `entry`-table record statistics. It is **admin-only**
(same gating as the existing *Site-wide Statistics* block) and renders directly
below it. All metrics are **period-independent** — they describe the whole
database, not the selected date range.

This mirrors the storage/health stats pattern from the sibling `noadd` project
(`StorageStats` / `DbHealth`), pared down to a single flat struct.

## Motivation

The `/statistics` page already shows per-user stats to everyone and a small
admin-only *Site-wide Statistics* block. There is currently no visibility into
the physical database (file size, reclaimable space) or aggregate record
counts/growth. Admins running a single-binary SQLite deployment benefit from
seeing capacity, fragmentation, and growth at a glance.

## Scope & access

- **Page:** `/statistics` (`handlers/pages/mod.rs::statistics_page`,
  `PageAuthUser` — login required).
- **Visibility:** rendered only when `show_admin_stats == true`, i.e.
  the user is an admin **and** is not masquerading. Identical to the existing
  `admin` block (`{% if let Some(a) = admin %}`).
- **Placement:** a second `.stats-admin-section` immediately after the
  Site-wide section.

## Metrics

All values are period-independent (not affected by the 7d/30d/90d/All/custom
picker).

### Storage (whole DB file, via `PRAGMA`)

| Field | Computation | Unit |
|-------|-------------|------|
| `db_size_bytes` | `page_count * page_size` | bytes |
| `reclaimable_bytes` | `freelist_count * page_size` | bytes |
| `fragmentation_ratio` | `reclaimable_bytes / db_size_bytes`, `0.0` when size is 0 | 0.0–1.0 |

### Records (`entry` / `entry_tombstone`, across all users)

| Field | Computation | Unit |
|-------|-------------|------|
| `total_entries` | `COUNT(*) FROM entry` | rows |
| `avg_new_entries_per_day` | `total_entries / age_days`, where `age_days = julianday('now') - julianday(MIN(created_at))`; `0.0` when `age_days <= 0` or table empty | rows/day |
| `coverage_days` | `julianday(MAX(created_at)) - julianday(MIN(created_at))`; `0.0` when fewer than 2 rows | days |
| `tombstone_count` | `COUNT(*) FROM entry_tombstone` | rows |

`entry.created_at` (`TEXT NOT NULL DEFAULT (datetime('now'))`) is the
row-insertion time — the rdrs analogue of noadd's log timestamp. `tombstone_count`
is the cumulative number of entries pruned by the per-user retention worker that
are still being blocked from re-import (tombstones are never GC'd; they only
cascade-delete with their feed).

## Data model (`src/models/statistics.rs`)

New struct + getter, following the existing `get_admin_*(conn: &Connection)`
pattern:

```rust
/// Admin database storage + record stats (period-independent).
pub struct AdminDatabaseStats {
    pub db_size_bytes: i64,
    pub reclaimable_bytes: i64,
    pub fragmentation_ratio: f64,
    pub total_entries: i64,
    pub avg_new_entries_per_day: f64,
    pub coverage_days: f64,
    pub tombstone_count: i64,
}

pub fn get_admin_database_stats(conn: &Connection) -> AppResult<AdminDatabaseStats>;
```

Queries (run on the read-only connection inside the existing `read_user`
closure):

1. `PRAGMA page_count`, `PRAGMA page_size`, `PRAGMA freelist_count` → storage.
2. `SELECT COUNT(*) FROM entry` → `total_entries`.
3. `SELECT MIN(created_at) FROM entry` and `SELECT MAX(created_at) FROM entry`
   as **separate single-aggregate statements** (critical: a combined
   `COUNT+MIN+MAX` defeats SQLite's MIN/MAX index-endpoint optimization).
4. `SELECT COUNT(*) FROM entry_tombstone` → `tombstone_count`.

`MIN(created_at)` / `MAX(created_at)` MUST be fetched **bare** (not wrapped in
`julianday(...)` or arithmetic) — the benchmark confirms the index-endpoint
optimization only fires for a lone `SELECT MIN(x)` / `SELECT MAX(x)`. The bare
`TEXT` timestamps are then parsed in Rust (chrono) to compute day deltas:
`coverage_days = max - min` and `age_days = now - min`, with
`avg_new_entries_per_day = total_entries / age_days`. Guard all divisions
(`fragmentation_ratio`, `avg_new_entries_per_day`) and the empty-table case
(`MIN`/`MAX` return `NULL`) → derived values `0.0`.

## Performance: new index

Benchmark (synthetic DB, 500k entries, 540 MB, warm cache):

| Query | No index | With `idx_entry_created_at` |
|-------|----------|------------------------------|
| Combined `COUNT+MIN+MAX` (single statement) | 143 ms | 36 ms |
| `MIN(created_at)` alone | 128 ms | ~0 ms (index endpoint) |
| `MAX(created_at)` alone | 133 ms | ~0 ms |
| **Split COUNT + MIN + MAX** | 265 ms | **~4 ms** (COUNT-bound) |
| `COUNT(*) FROM entry_tombstone` | 0.4 ms | (unchanged) |

`COUNT(*) FROM entry` already uses the existing covering index
`idx_entry_sort_ts` (~4 ms) and needs no new index. The existing
`idx_entry_sort_ts` is on `COALESCE(published_at, created_at)` (an expression),
so it cannot serve `MIN/MAX(created_at)`.

**Decision:** add one index and split the query.

```sql
CREATE INDEX idx_entry_created_at ON entry(created_at);
```

- **Read:** 143 ms → ~4 ms (~35×); MIN/MAX become constant-time as the table grows.
- **Storage cost:** ~14 MB at 500k rows (~28 B/row, ~2.6% of DB).
- **Write cost:** +0.7 µs/insert (A/B measured). `created_at` is monotonic, so
  the index appends to the right edge with no page splits — negligible for the
  feed-sync hot path (tens–hundreds of inserts per sync).

Added via a schema migration that bumps `PRAGMA user_version` in
`src/db/schema.rs`.

## Handler (`src/handlers/pages/mod.rs::statistics_page`)

Inside the existing `state.db.read_user(move |c| { ... })` closure, add a gated
fetch alongside `admin_counts` / `admin_entry_stats`:

```rust
let admin_db_stats = if show_admin_stats {
    crate::models::statistics::get_admin_database_stats(c).ok()
} else {
    None
};
```

Pre-format for display in the handler (consistent with existing pre-formatting
there): byte values → human-readable (e.g. MB) strings, `fragmentation_ratio`
→ integer percent for the "X% of file" sub-line. Pass into `StatisticsTemplate`
as a new optional field.

## Template (`templates/statistics.html`)

After the `{% if let Some(a) = admin %}` Site-wide block, add a parallel
`{% if let Some(db) = admin_db_stats %}` block:

```html
<div class="stats-admin-section">
  <h2>Database</h2>
  <div class="stats-cards stats-cards--db">
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.size_fmt }}</div><div class="stats-card-label">Database Size</div></div>
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.reclaimable_fmt }}</div><div class="stats-card-sub">{{ db.frag_pct }}% of file</div><div class="stats-card-label">Reclaimable</div></div>
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.total_entries }}</div><div class="stats-card-label">Total Entries</div></div>
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.avg_per_day_fmt }}</div><div class="stats-card-label">Avg New / Day</div></div>
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.coverage_fmt }}</div><div class="stats-card-label">Coverage</div></div>
    <div class="stats-card stats-card-admin"><div class="stats-card-value">{{ db.tombstone_count }}</div><div class="stats-card-label">Pruned Entries</div></div>
  </div>
</div>
```

Card order: storage (Size, Reclaimable) then records (Total Entries, Avg/Day,
Coverage, Pruned Entries), flowing across a fixed 3×2 grid.

## CSS (`static/css/app.css`)

1. **Modify** `.stats-admin-section` — remove the divider and heavy top spacing
   (applies to both the existing Site-wide section and the new Database section,
   per design decision):
   ```css
   /* was: border-top: 1px solid var(--color-border); padding-top: var(--space-6); margin-top: var(--space-6); */
   .stats-admin-section { margin-top: var(--space-6); }
   .stats-admin-section h2 { margin-top: 0; }
   ```
2. **Add** the fixed 3-up grid for the Database section:
   ```css
   .stats-cards--db { grid-template-columns: repeat(3, 1fr); }
   @media (max-width: 768px) { .stats-cards--db { grid-template-columns: repeat(2, 1fr); } }
   @media (max-width: 480px) { .stats-cards--db { grid-template-columns: 1fr; } }
   ```
3. **Add** the value sub-line:
   ```css
   .stats-card-sub { font-family: var(--font-ui); font-size: var(--font-xs);
       color: var(--color-accent); margin-top: var(--space-1); font-weight: 500; }
   ```

## Testing

- **Unit tests** (`src/models/statistics.rs`, in-memory DB, existing pattern):
  - empty DB → all derived values `0` (no panic, no division by zero);
  - seeded entries → correct `total_entries`, `tombstone_count`;
  - `0.0 <= fragmentation_ratio <= 1.0`;
  - `coverage_days` reflects the seeded `created_at` span.
- **E2E:** confirm existing `/statistics` BDD scenarios still pass; the new
  cards reuse `stats-card` markup, so existing selectors are unaffected. Add
  `data-testid` hooks on new cards if an admin assertion is desired (optional).
- Rebuild (`cargo build`) before any E2E run — assets are `include_str!`'d.

## Non-goals / deferred

- Per-table size breakdown via `dbstat` (would need its own query + caching).
- Including the `-wal` file size — only the main `.db` file is measured
  (`page_count * page_size`), matching noadd. Recent WAL-resident writes may not
  be reflected until checkpoint.
- No caching: queries are cheap (~4 ms) and the page is admin-only/infrequent.

## Out-of-scope confirmations

- `/statistics` is **not** among the four README screenshots (unread list +
  keyboard-help, light/dark), so no screenshot regeneration is required.
- Version fields and `CHANGELOG.md` are not touched.
