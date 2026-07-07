# Multi-Database Support (SQLite + PostgreSQL) via sqlx — Migration Design

**Date:** 2026-07-07
**Branches:** `chore/sqlx-multi-db-spike` (feasibility spike, pushed), `refactor/db-repo-seam` (Phase A)
**Status:** Feasibility proven by spike; phased migration approved, Phase A in progress

## Goal

Let a self-hoster run rdrs against **their own PostgreSQL server** as an alternative
to the built-in SQLite. Many self-hosters already operate a Postgres instance and want
to reuse it for backups, HA, and central ops.

The backend is chosen **once at startup** from `DATABASE_URL` — SQLite *or* Postgres.
There is **no mid-flight switching, no cross-backend data migration, no dual-write.**
This premise removes the need for `sqlx::Any` and lets the whole design collapse to a
single `enum Db` built once at boot.

Route chosen: **unify both backends on `sqlx`** (async), replacing rusqlite entirely —
not a rusqlite + parallel-Postgres trait layer. sqlx gives one async API, one pooling
story, one `FromRow`/type-mapping system, one `sqlx::Error`, and each driver is
independently battle-tested.

## Non-Goals (YAGNI / possible follow-ups)

- No runtime backend switching or data migration tool between SQLite and Postgres.
- No `sqlx::Any` driver (its type support is narrower; the boot-time choice makes it
  unnecessary).
- No compile-time query macros (`query!`) — they validate against a single live
  `DATABASE_URL` and cannot cover two backends. We use runtime `query_as`/`QueryBuilder`,
  accepting the loss of compile-time SQL checking.
- No MySQL / other backends.
- No change to the SSR/Askama/Axum layers, the GReader API, or the SSE stream.

## What the spike proved (feasibility)

Standalone crate `spike/sqlx-slice/` (isolated — see the hard constraint below),
7 tests green against SQLite in-memory **and** a live PostgreSQL 17:

- **Dispatch:** one `enum Db { Sqlite(SqlitePool), Postgres(PgPool) }`, built once.
- **Row mapping:** one `#[derive(FromRow)]` decodes both `SqliteRow` and `PgRow`.
- **Type mapping:** `DateTime<Utc>` maps to SQLite TEXT and PG `timestamptz` — *iff the
  app binds `Utc::now()`* instead of a `datetime('now')` column DEFAULT (so
  encode/decode formats agree). **Decision: bind timestamps, never DEFAULT.**
- **`RETURNING`** works on both (SQLite ≥ 3.35, bundled) → removes the SQLite-only
  `last_insert_rowid()` pattern; the insert becomes identical across backends.
- **Unified constraint errors:** one `sqlx::error::ErrorKind::UniqueViolation` check
  replaces the ~26 per-driver `rusqlite::Error::SqliteFailure { ConstraintViolation }`
  matches.
- **Placeholders:** a single `$N`-style string runs on both (SQLite's parameter syntax
  is a superset), so per-arm SQL is *not* duplicated for placeholder reasons.
- **Dynamic query builder** (`entry/filters.rs`): one generic `sqlx::QueryBuilder<DB>`
  with the three genuine dialect forks isolated in a `Dialect` type (below).
- **Priority scheduler:** Postgres uses two independently-capped pools; SQLite keeps a
  thin biased two-channel gate in front of the single write pool.

### The three genuine dialect forks (everything else is portable)

| Concern | SQLite | Postgres |
|---|---|---|
| Case-insensitive search | `LIKE … COLLATE NOCASE` | `ILIKE` |
| Epoch from a timestamp | `CAST(strftime('%s', …) AS INTEGER)` | `EXTRACT(EPOCH FROM …)` |
| Query-level index hint | ` INDEXED BY idx_…` | *(none — Postgres has no hint; trust planner)* |

Note the search semantics differ subtly: `COLLATE NOCASE` folds ASCII only; `ILIKE` is
locale-aware. Documented, accepted.

## Hard constraint (drives the whole strategy)

**rusqlite 0.40 and sqlx-sqlite 0.9 cannot coexist in one binary.** Both declare
`links = "sqlite3"` with incompatible `libsqlite3-sys` majors (`^0.38` vs `<0.38`), so
Cargo refuses to build both. Consequence: the SQLite driver swap is a **one-shot
cutover** — there is no "run both drivers, migrate model-by-model" period. This is why
the spike lives in an isolated crate, and why the migration is phased so the cutover is
a single contained change rather than a long-lived broken branch.

## Current-state audit (Phase A findings)

- Models are **synchronous free functions** taking `conn: &rusqlite::Connection` and
  returning domain structs (`AppResult<Category>` etc.), across 13 model files, ~290
  raw-SQL sites. `entry/mod.rs` alone has ~69; `statistics.rs` ~29.
- Handlers/services reach the DB via **~146 closure call sites** of the shape
  `state.db.user(move |conn| category::create_category(conn, …)).await`
  (`user` / `read_user` / `background` / `read_background` / `user_detached`). The sync
  actor closure API (`db/pool.rs`) is the seam; it disappears entirely under async sqlx.
- Domain errors do **not** leak rusqlite to callers (handlers see `AppError::CategoryExists`,
  not the driver error); the ~26 constraint matches are contained inside model bodies.
- **The `db.user(|conn| …)` closure IS the unit-of-work / transaction boundary.** Many
  closures compose *several* model functions on one `conn` (e.g. OPML import loops
  find-or-create category → create feed in a single closure), and several use explicit
  `conn.unchecked_transaction()` (`entry/mod.rs`, `greader/tag.rs`,
  `content_text_backfill.rs`, `feed_sync.rs`). A single model fn like `create_category`
  is called from ~24 sites, mostly *inside* such composed closures — it is a building
  block, not a standalone call.
- Timestamps are stored as **TEXT via `datetime('now')` DEFAULT** and parsed back to
  `DateTime<Utc>` in Rust (`parse_datetime`). Migrations are versioned by
  `PRAGMA user_version` in `db/schema.rs`; partial/sort indexes added in v4/v5.
- 17 test files build fixtures with `Connection::open_in_memory()`.

## Seam decision: why NOT a per-model-function facade

A first design idea was a per-model-function async facade
(`db.create_category(…).await`) whose body delegates to the rusqlite actor in Phase A
and swaps to sqlx in Phase B. **Rejected** after the audit above: turning each model fn
into its own `db.user(|conn| …)` round-trip **breaks the composed closures' atomicity** —
the OPML importer's find-or-create-category-then-create-feed would split into separate
connections/turns, so a category could commit while its feed insert fails. The
unit-of-work is the *closure*, not the function.

The only atomicity-preserving facade would be **coarse, use-case-grained** (one method
per closure, ~146 of them). But those method bodies get rewritten *again* in Phase B —
close to double the work, bought only "incremental releasability." Not worth it.

**Decision:** the model layer and the closures that compose it are one tightly-coupled
unit (shared `&Connection`, sync→async boundary, transaction boundaries) and **flip
together in Phase B**. Phase A does not attempt to pre-decouple call sites; it invests in
the safe, non-throwaway prep that genuinely shrinks Phase B risk.

## Phased plan

### Phase A — safe prep (stays on rusqlite; every PR releasable to main)
Scope deliberately narrow — no call-site decoupling (see the seam decision above).
1. **A4 (done):** classify `DATABASE_URL` into a `Backend` (SQLite vs Postgres) in
   `config.rs`; SQLite behaves exactly as today, `postgres://` returns a clear "not yet
   supported" error. Zero behavior change. *(Committed on `refactor/db-repo-seam`.)*
2. **A2:** switch timestamp writes from `datetime('now')` DEFAULT to app-bound
   `Utc::now()` (doable under rusqlite; removes the biggest Phase-B datetime divergence).
3. **A5 (inventory):** classify the ~146 `db.<accessor>(|conn| …)` closures — single-model
   vs multi-model, and which hold an explicit `unchecked_transaction()`. The multi-model
   / transactional ones become sqlx `transaction()` blocks in Phase B; this inventory is
   the Phase B work-list, not code.

**Gate:** `cargo nextest run` green, `cargo clippy -D warnings`, E2E green, no behavior
change.

### Phase B — cutover (one PR: rusqlite → sqlx)
This is where the model functions **and** the ~146 call-site closures flip from sync
`&Connection` to async sqlx together (guided by the A5 inventory); multi-model /
`unchecked_transaction()` closures become sqlx `transaction()` blocks.
- **B1** `Cargo.toml`: drop rusqlite, add sqlx; re-run `cargo deny`.
- **B2** `db/pool.rs`: `enum Db` over `SqlitePool`/`PgPool`; WAL/tuning via
  `SqliteConnectOptions`; SQLite biased gate; PG dual pool sizing (user 8 / bg 2).
- **B3** models: async bodies, `#[derive(FromRow)]`, `RETURNING`, unified `ErrorKind`
  (signatures unchanged from Phase A).
- **B4** `entry/filters.rs` + `entry/mod.rs`: `QueryBuilder<DB>` + `Dialect` (spike is the
  template).
- **B5** `error.rs`: `Database(rusqlite::Error)` → `Database(sqlx::Error)`.
- **B6** `db/schema.rs` → `migrations/{sqlite,postgres}/` numbered `.sql`, two
  `sqlx::migrate!` migrators selected by backend (PG partial indexes for the v4/v5 sort
  indexes).
- **B7** test fixtures: in-memory sqlx pool + migrator.

**Gate:** SQLite suite green (== today); binary boots; E2E green. (PG not yet in CI.)

### Phase C — Postgres enablement & hardening
- **C1** CI PG lane (testcontainers-rs or a service container); local SQLite lane needs
  no Docker (PG tests `#[ignore]` + env-gated, as in the spike).
- **C2** *(optional)* run SSR E2E against both backends for near-free double coverage.
- **C3** docs: `README.md`, `ARCHITECTURE.md`, `CLAUDE.md` (PG opt-in; SQLite stays the
  zero-config single-binary default).
- **C4** `deny.toml`: review sqlx's transitive licenses/advisories (sqlx 0.9.0, published
  2026-05-21, already past the 7-day cooldown).
- **C5** ops: SQLite deploy unchanged; PG via `DATABASE_URL`; Docker image unchanged.

## Risks to watch (Phase B)
1. **`INDEXED BY` removal** — SQLite perf crutches; build equivalent (incl. partial)
   Postgres indexes and confirm adoption with `EXPLAIN`.
2. **`statistics.rs` date grouping** — `strftime` aggregates need PG equivalents
   (`date_trunc` / `to_char`).
3. **Transactions** — multi-statement writes (e.g. feed + initial entries) → sqlx
   transactions.
4. **Type affinity** — SQLite `0/1 INTEGER` vs PG `BOOLEAN`; DDL must use correct types
   (Rust `bool` maps via sqlx).
5. **Search semantics** — `COLLATE NOCASE` (ASCII) vs `ILIKE` (locale-aware) on non-ASCII.

## Effort
Phase A ≈ 1–2 days (A4 + A5 done; A2 folds into B) · Phase B ≈ 2–3 weeks (absorbs the
model + closure flip) · Phase C ≈ 3–5 days. ≈ 3–4 weeks total.
Endpoint: one code path, two backends, SQLite deploy experience unchanged.

## Appendix — A5 closure inventory (Phase B work-list)

Per-file count of `db.<accessor>(|conn| …)` call sites (the units that flip to async
sqlx in Phase B). `pool.rs` is excluded (that's the actor definition/tests). `TX` marks
files with an explicit `unchecked_transaction()` — those closures become sqlx
`transaction()` blocks; the rest are mechanical single-round-trip conversions.

| File | user | read_user | bg | read_bg | detach | TX |
|---|---|---|---|---|---|---|
| handlers/pages/mod.rs | 2 | 16 | | | | |
| handlers/entries.rs | 2 | 12 | | | 3 | |
| services/feed_sync.rs | 7 | | 10 | | | 2 |
| services/summary_worker.rs | 5 | 1 | 6 | 1 | | |
| handlers/passkey.rs | 9 | 3 | | | | |
| handlers/user.rs | 6 | 5 | | | | |
| handlers/feeds.rs | 5 | 3 | | | | |
| handlers/greader/subscription.rs | 5 | 3 | | | | |
| handlers/admin.rs | 5 | | | | | |
| handlers/greader/tag.rs | 4 | 1 | | | | 1 |
| handlers/entry.rs | 2 | 4 | | | | |
| handlers/greader/item.rs | | 4 | | | | |
| services/summary_cleanup.rs | 3 | | 2 | | | |
| handlers/categories.rs | 3 | | | | | |
| handlers/auth.rs | 3 | | | | | |
| services/content_text_backfill.rs | 2 | | 2 | | | 1 |
| middleware/auth.rs | 2 | 3 | | | | |
| handlers/greader/auth.rs | 2 | 1 | | | | |
| services/entry_retention.rs | 1 | | 3 | | | |
| middleware/forward_auth.rs | 1 | 1 | | | | |
| handlers/feed.rs | | 1 | | | | |
| handlers/greader/user.rs | | 1 | | | | |
| services/background.rs | | | | 1 | | |

**Transaction / multi-model closures (need sqlx `transaction()`):**
`feed_sync.rs` (×2), `content_text_backfill.rs`, `greader/tag.rs` (tag rename), plus the
in-model `entry/mod.rs` `unchecked_transaction()`. Also atomicity-sensitive even without
an explicit BEGIN: the OPML importers in `feeds.rs` and `greader/subscription.rs`
(find-or-create category → create feeds in one closure).

**Hotspots (largest single-file surface):** `pages/mod.rs` (18), `entries.rs` (17),
`feed_sync.rs` (17), `summary_worker.rs` (13). Sequence Phase B model-by-model but expect
these four files to carry most of the closure-conversion churn.
