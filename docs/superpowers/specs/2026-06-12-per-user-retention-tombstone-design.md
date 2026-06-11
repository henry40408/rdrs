# Per-User Read-Entry Retention + Tombstones — Design

Date: 2026-06-12
Branch: `feat/per-user-retention-tombstone`

## Goal

Give rdrs a way to bound unbounded entry growth: a **per-user, opt-in
retention policy** that deletes old *read* entries, backed by a **tombstone**
table that stops deleted entries from being re-imported as unread on the next
feed refresh.

Today rdrs never deletes entries — they accumulate forever, removed only when
their feed is deleted (`ON DELETE CASCADE`). There is no retention, no manual
"flush history", and therefore no tombstone. This design adds the first
deletion source (retention) **and its mandatory tombstone companion** in one
change.

Modeled on miniflux's tombstone design (`entry_tombstones` table + atomic
`WHERE NOT EXISTS` insert guard), adapted to rdrs's data model.

## Out of scope (deliberate, expandable later)

- **Unread retention.** miniflux also archives *unread* entries past a longer
  threshold (`CLEANUP_ARCHIVE_UNREAD_DAYS`, default 180d). Riskier (deletes
  things never seen). v1 does read-only. The design is forward-compatible: add
  a sibling `retention_unread_days` column + form field + a second worker pass;
  the tombstone mechanism is unchanged. See "Future expansion".
- **Manual flush-history** (user-triggered "remove all read"). A second
  deletion source that would reuse the same tombstone machinery. Not built now.
- **Tombstone garbage collection.** Tombstones are kept forever (they are
  `(feed_id, guid)` + a timestamp — lightweight). No TTL/GC worker.

## Key facts grounding this design

- Entry identity is `UNIQUE(feed_id, guid)` (`src/db/schema.rs:76`); rdrs uses
  the feed item's GUID directly — **no hashing** (unlike miniflux's SHA256).
- Entries are feed-scoped; a feed belongs to one user via
  `entry → feed → category → user`. `entry` has **no `user_id`**;
  `read_at`/`starred_at` live directly on `entry`.
- Migrations are in-process via `PRAGMA user_version` (`src/db/schema.rs:182`),
  currently at **7**; this change introduces **version 8**.
- Existing periodic-worker pattern: `start_cleanup_worker(db, interval_hours,
  ttl_hours, cancel_token)` (`src/services/summary_cleanup.rs`), started in
  `src/main.rs:71`, graceful shutdown via `CancellationToken`.
- Existing per-user setting pattern: `user_settings` table with a later-added
  nullable `theme` column + dedicated `update_theme()` fn
  (`src/models/user_settings.rs:140`); settings form is
  `templates/user_settings.html`, update handler `update_user_settings`
  (`src/handlers/user.rs:431`).
- **Every table in the schema carries `created_at TEXT NOT NULL DEFAULT
  (datetime('now'))`** — a universal convention the tombstone table follows.

## Design

### A. Tombstone table (foundation)

New migration block `if version < 8` in `src/db/schema.rs`, plus bump
`LATEST_VERSION` to 8:

```sql
CREATE TABLE IF NOT EXISTS entry_tombstone (
    feed_id    INTEGER NOT NULL REFERENCES feed(id) ON DELETE CASCADE,
    guid       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (feed_id, guid)
) WITHOUT ROWID;
```

- Key `(feed_id, guid)` mirrors `entry`'s `UNIQUE(feed_id, guid)` — reuse GUID
  directly, no hashing.
- `ON DELETE CASCADE` on `feed`: deleting a feed drops its tombstones too,
  consistent with `entry`'s cascade.
- `WITHOUT ROWID`: lightweight composite-PK table.
- `created_at` matches the universal schema convention (for a tombstone, row
  creation == the moment the entry was purged). **No index on it** — there is
  no GC and no query reads it; an index would be speculative future-proofing.
  If GC is ever wanted, adding the index is a one-line migration.

### B. Refresh protection (the other half of the foundation)

Modify `entry::upsert_entry_id` (`src/models/entry/mod.rs:484`). Current logic:
look up existing by `(guid, feed_id)`; if present `UPDATE`, else `INSERT`. Only
the **INSERT path** changes — guard it atomically against the tombstone:

```sql
INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at)
SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
WHERE NOT EXISTS (
    SELECT 1 FROM entry_tombstone WHERE feed_id = ?1 AND guid = ?2
);
```

- `WHERE NOT EXISTS` makes the tombstone check atomic with the insert — a
  retention delete committing between a separate check and the insert cannot
  resurrect the entry as unread (same guarantee as miniflux).
- After the statement, inspect `changes()`: `0` rows ⇒ the GUID is tombstoned
  ⇒ report "skipped".
- Return type changes from `(i64, bool)` (`id`, `is_new`) to an enum:
  ```rust
  pub enum UpsertOutcome {
      Inserted(i64),
      Updated(i64),
      SkippedTombstoned,
  }
  ```
  `feed_sync.rs` (entry loop ~`src/services/feed_sync.rs:299`) maps the three
  arms to `new_entries` / `updated_entries` / a new `skipped_entries` counter
  (logged, for observability).
- The **UPDATE path is untouched**: an entry cannot be simultaneously alive and
  tombstoned, because deletion removes the entry and inserts the tombstone in
  one transaction (see C).

### C. Retention worker (first consumer)

New service `src/services/entry_retention.rs`, mirroring `summary_cleanup.rs`:

```rust
pub fn start_retention_worker(
    db: DbPool,
    interval_hours: u64,
    cancel_token: CancellationToken,
) -> JoinHandle<()>
```

Note: the per-user *threshold* lives in `user_settings` (D), so the worker
takes no `days` argument — it reads each user's value via a join. The worker
always runs on its interval; it is a cheap no-op when no user has retention
enabled.

Each tick, in **batches of 500** inside a single write transaction per batch
(actor pool serializes writes; the transaction gives atomicity so a concurrent
`feed_sync` cannot resurrect a victim):

1. Select up to 500 victims (Rust-side, to avoid SQLite `LIMIT`-without-`ORDER
   BY` picking different rows across two statements):
   ```sql
   SELECT e.id, e.feed_id, e.guid
   FROM entry e
   JOIN feed f           ON f.id = e.feed_id
   JOIN category c       ON c.id = f.category_id
   JOIN user_settings us ON us.user_id = c.user_id
   WHERE us.retention_read_days > 0
     AND e.read_at    IS NOT NULL
     AND e.starred_at IS NULL
     AND e.read_at < datetime('now', '-' || us.retention_read_days || ' days')
   LIMIT 500;
   ```
2. `INSERT INTO entry_tombstone (feed_id, guid) VALUES … ON CONFLICT DO
   NOTHING` for the batch.
3. `DELETE FROM entry WHERE id IN (…batch ids…)`.
4. Loop until a batch returns 0 rows.

`starred_at IS NULL` ⇒ starred entries are **never** deleted. The per-user
threshold is applied per row via the join (`us.retention_read_days`), so one
SQL pass covers all users — no per-user loop.

Started in `src/main.rs` alongside `start_cleanup_worker`, and added to the
graceful-shutdown handle list (`src/main.rs:115`).

### D. Opt-in: per-user setting + UI (default off)

Retention is **opt-in, per user**, configured in the existing settings UI.

**Schema (version 8):**
```sql
ALTER TABLE user_settings ADD COLUMN retention_read_days INTEGER NOT NULL DEFAULT 0;
```
`0` = disabled = the opt-in default. `> 0` = delete read entries older than N
days.

**Model (`src/models/user_settings.rs`):** add `retention_read_days` to the
`UserSettings` struct and `row_to_settings` mapping; add
`update_retention_read_days()` following the `update_theme()` `INSERT … ON
CONFLICT` pattern. Validate `>= 0` (reject negatives); `0` is the off sentinel.

**Handler (`src/handlers/user.rs`):** extend the settings update path
(`update_user_settings`, ~`:431`) to accept `retention_read_days`; add the
field to `UserSettingsResponse` (`:74`).

**UI (`templates/user_settings.html`):** add a number input next to
`entries_per_page` — label e.g. *"Delete read articles older than N days
(0 = never)"*, min 0. SSR + vanilla JS, no new build tooling.

## Future expansion (not built, design-compatible)

- **Unread retention:** add `retention_unread_days` column + form field; the
  worker runs a second pass with `read_at IS NULL` and the unread threshold.
  Tombstone path unchanged.
- **Manual flush-history:** a button that runs the C-style delete-plus-tombstone
  for *all* of a user's read entries (no age filter), reusing `entry_tombstone`
  and the same refresh guard.
- **Tombstone GC:** if tombstones ever need pruning, add an index on
  `created_at` and a TTL sweep. Deliberately deferred.

## Benchmarks (pre-implementation, SQLite 3.51.2)

Synthetic probes on faithful schema + indexes; the SQLite engine is the same as
the bundled rusqlite, so query-planner/B-tree behavior is representative.

**B. Refresh guard — `INSERT … WHERE NOT EXISTS (entry_tombstone)`** (50k new
inserts, ~500B content; marginal cost of the tombstone probe vs a plain INSERT):

| tombstone rows | plain INSERT | INSERT + guard | per-insert overhead |
| --- | --- | --- | --- |
| 0 | 0.123s | 0.129s | 0.13 µs |
| 100k | 0.123s | 0.192s | 1.37 µs |
| 1M | 0.120s | 0.213s | 1.86 µs |

Worst case ~1.9 µs per *new* entry even against a 1M-row tombstone table;
existing entries (UPDATE path) pay nothing. A refresh inserting ~100 new
entries adds <0.2 ms. **Guard kept** — race-proof and negligible.

**C. Retention victim SELECT** (1M entries, ~597k victims, 1 user / 10
categories / 200 feeds; 2000 iterations of the `LIMIT 500` fetch):

| case | per 500-row batch |
| --- | --- |
| disabled (`retention_read_days = 0`) | ~0.075 ms (short-circuits at `SCAN us`) |
| enabled, current indexes | ~0.75 ms |
| enabled, + candidate `idx_entry_retention` | ~0.73 ms (no gain) |

Plan drives from `user_settings`, then range-scans `idx_entry_read_at`
(`read_at > ? AND read_at < ?`). **No new index needed** — the candidate
partial index gave no measurable improvement and is not added. Disabled installs
pay nothing per tick. First full drain (~597k victims ≈ 1194 batches) does not
degrade: deleted rows leave the index front, so each batch stays ~0.75 ms.

## Testing plan

- **Unit (`cargo nextest run`):**
  - `entry_tombstone` migration creates the table (extend the existing
    `test_init_db_*` / version tests in `src/db/schema.rs`).
  - `upsert_entry_id`: returns `SkippedTombstoned` when a `(feed_id, guid)`
    tombstone exists; `Inserted`/`Updated` otherwise; the UPDATE path ignores
    tombstones (cannot happen in practice but assert the INSERT-only guard).
  - `user_settings`: `update_retention_read_days` upserts; default is `0`;
    negatives rejected.
  - retention worker logic (mirroring `summary_cleanup.rs` tests): deletes only
    read + aged + non-starred entries belonging to users with
    `retention_read_days > 0`; writes matching tombstones; respects each user's
    own threshold; no-op when all users are at `0`; stops on cancellation.
  - End-to-end invariant: delete via retention → re-run `feed_sync` with the
    same GUID still present in the feed → entry does **not** reappear.
- **BDD (playwright-bdd):** a settings scenario toggling retention days
  (persists + round-trips in the form). (Worker timing is covered by unit
  tests, not BDD.)

## Verification

- `cargo build` + `cargo nextest run` green; `cargo fmt` before commit.
- Manual: set `retention_read_days` in the UI, seed an old read entry, run the
  worker, confirm deletion + tombstone row + non-resurrection after refresh.
- (Re-source `/tmp/rdrs-env.sh` for OpenSSL env before cargo/e2e on this box.)
