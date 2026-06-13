# Async Entry-State Writes — Design

**Date:** 2026-06-14
**Branch:** `feat/async-entry-writes`
**Status:** Approved design, pending implementation plan

## Problem

Opening an entry (and toggling read/star) is intermittently slow — measured
**799 ms TTFB** in the user's browser (Firefox: all phases 0 except
`Waiting` = 799 ms; HTTP/2, so it is not connection-pool or transfer related).

Root cause confirmed by measurement + code reading:

- The DB layer (`src/db/pool.rs`) is an **actor-per-connection** model: one
  WRITE connection + one READ connection, each owned by a single actor task
  that processes messages **serially and runs each closure to completion with
  no mid-closure preemption**. The `biased` select only prioritizes *queued*
  user messages; it cannot interrupt the closure currently executing.
- Background feed sync persists **an entire feed's entries in one
  `background()` closure wrapped in a single transaction** (`feed_sync.rs:287`,
  "collapses N per-entry commits into a single commit").
- Opening an entry runs a **write** (`mark_as_read`) via `state.db.user(...)`
  on that same WRITE actor. While a sync's big transaction is executing, the
  user's mark-as-read is queued behind it and the HTTP response waits the whole
  time → ~800 ms. The same applies to **star/unstar and the read/unread
  buttons** (all `user()` writes), so the latency is systemic, not just
  entry-open.
- Local idle measurement of the same handler was ~20 ms, confirming the delay
  is write-actor contention, not query cost or fragment size.

Under WAL, the **READ connection is a separate actor and is not blocked by the
WRITE actor's in-flight transaction** — reads stay fast during a sync.

## Goals

- Make the 5 pure state-flip user writes return in ~tens of ms even while a
  background sync holds the write actor:
  - entry open (`GET /entries/{id}/fragment`, mark-as-read),
  - mark read (`POST /entries/{id}/read`),
  - mark unread (`POST /entries/{id}/unread`),
  - star (`POST /entries/{id}/star`),
  - unstar (`POST /entries/{id}/unstar`).
- Keep the existing response shape (multi-target swap: row + sidebar + pane /
  pane-star-form / flash) so no template/JS/API changes are needed.

## Non-Goals

- **Do not chunk the feed-sync transaction.** It stays one transaction per feed
  (user decision; preserves atomicity).
- Do not change the content-producing / external-service handlers
  (`fetch-full-content`, `save`, `summarize`) — they must await real results and
  cannot be rendered optimistically.
- No HTTP API, template, or `app.js` changes. No new frontend tooling.
- Not addressing the (separate, already-queued) sidebar completed-summary-count
  feature.

## Approach

**Optimistic response from the READ connection + detached (fire-and-forget)
write on the WRITE actor.** Each affected handler:

1. Reads the entry's current state via the **read** connection (`read_user`),
   which is not blocked by the sync.
2. Builds the normal multi-target response, applying the state change
   **in memory** (optimistically) rather than reading it back from a write.
3. Enqueues the actual DB write via a new **non-awaiting** pool call, then
   returns immediately.

### New pool primitive: `DbPool::user_detached`

```rust
/// Enqueue a write-actor closure with User priority and return immediately,
/// without awaiting its result. Ordering is preserved (single mpsc, FIFO),
/// so rapid star→unstar still applies in submission order. The closure
/// returns a `rusqlite::Result<()>`; a failure is logged (a lightweight
/// spawned task awaits the oneshot and `warn!`s on `Err`) but not surfaced
/// to the caller.
pub fn user_detached<F>(&self, f: F)
where
    F: FnOnce(&Connection) -> rusqlite::Result<()> + Send + 'static,
```

Implementation: wrap `f` into a `DbMessage` whose result is the boxed
`rusqlite::Result<()>`, send it to `user_tx` (send completes as soon as the
message is buffered — the 256-slot channel makes this effectively instant; it
does **not** wait for the actor to run the closure), and `tokio::spawn` a tiny
task that awaits the oneshot, downcasts to `rusqlite::Result<()>`, and `warn!`s
on `Err`. FIFO ordering on `user_tx` guarantees submission-order application and
correct ordering relative to other detached/awaited user writes. (`tx` is an
unbounded-enough buffered channel; if `try_send` ever fails because the buffer
is full, fall back to logging and dropping — a dropped state-flip self-heals on
the next poll/reload.)

Why not `tokio::spawn(pool.user(f))` per handler: two rapid spawns race on
`tx.send`, so they can reorder and leave the wrong final state. `user_detached`
sends synchronously in handler order, preserving FIFO.

### Per-handler data flow

**Entry open** (`entry_fragment`, `OpenEntryMulti { pane, r, sidebar_unread_payload_json }`):
- `read_user`: load `EntryWithFeed`, summary status. Determine `was_unread =
  entry.read_at.is_none()`.
- Build `pane` (`build_reading_pane_view` — all reads + CPU sanitize) and the
  row view with `read_at` forced to "now" in memory so the row renders as read.
- Sidebar: `read_user` `unread_counts_per_feed`, then **−1** on the entry's
  `feed_id` if `was_unread` (floor at 0); serialize.
- If `was_unread`: `user_detached(|c| { mark_as_read(c, id); })` and
  `sidebar_cache.bust(user_id)`. If already read: no write, no delta.
- Return.
- Keep the existing `Sec-Fetch-Dest: document` redirect guard unchanged.

**Star / unstar** (`set_starred_state`, `EntryActionMulti { r,
sidebar_unread_payload_json, flash, pane_star_form }`):
- `read_user`: load entry + summary status. `changed = is_starred != desired`.
- Row view with `starred_at` set/cleared in memory to `desired`.
- Sidebar unread is **unaffected** by starring → serialize current counts as-is.
- If `changed`: `user_detached(set_starred...)`. (No flash for star, matching
  current behavior.) `pane_star_form` rendered from the optimistic state so the
  reading-pane button flips.
- Return.

**Read / unread buttons** (`set_read_state`):
- `read_user`: load entry + status. `changed = is_read != desired`.
- Row view with `read_at` set/cleared in memory.
- Sidebar: current counts with **±1** on the feed when `changed` (mark read
  −1, mark unread +1).
- If `changed`: `user_detached(set_read...)` + `sidebar_cache.bust`. The
  "Marked as unread." flash is preserved for the unread action.
- Return.

A shared helper builds the optimistic `EntryActionMulti` from `(EntryWithFeed,
desired_read/desired_starred, summary_status, counts_with_delta)` to avoid
duplicating the read/unread and star/unstar bodies.

### In-memory optimistic mutation

`row_view_from` already maps `EntryWithFeed` → `EntryRowView` using
`entry.read_at` / `entry.starred_at`. The handlers mutate the loaded
`EntryWithFeed` in memory (set/clear `read_at` / `starred_at`) before calling
`row_view_from`, so no new view constructor is needed.

## Consistency & Error Handling (accepted trade-offs)

- **Eventual consistency.** The write is applied shortly after the response.
  Visible state (row, sidebar count) is rendered optimistically and is correct
  on success.
- **Read-your-writes gap.** A second action on the *same* entry issued before
  the prior detached write commits may read stale state from the read
  connection (e.g. open then immediately star → the star response's row may
  briefly show "unread"). Self-heals via the existing 20 s `GET /sidebar/unread`
  poll and on any reload. Accepted (rare, low-stakes).
- **Write failure** is logged (`warn!`) only, not surfaced; the 20 s poll /
  reload reconciles the count and state.
- **Shutdown/crash** may drop detached writes still in the queue (a missed
  mark-as-read or star). Low impact for a reader. (Existing `shutdown()` runs a
  WAL checkpoint; draining the user queue on shutdown is out of scope but noted.)
- **Sidebar count** uses optimistic ±1 so it never lags (no 20 s gap).

## Testing

- **`DbPool::user_detached` unit tests:** the call returns before the closure
  runs; the write is eventually applied; **FIFO ordering** holds (submit A then
  B detached, then await a sentinel `user(noop)` — because the queue is FIFO the
  sentinel completes only after A and B, so asserting DB state after it is
  deterministic). Add a `flush`-style sentinel helper for tests.
- **Handler tests** (`tests/handlers_test.rs`): for each of the 5 handlers,
  assert the response renders the optimistic state (row read/starred, sidebar
  count delta) *and* that after a sentinel flush the DB reflects the write.
  Reuse the existing `test_entry_fragment_renders_reading_pane` scaffold.
- **Idempotency:** opening an already-read entry / starring an already-starred
  entry issues **no** detached write and applies **no** count delta.
- **e2e:** existing `reading.feature` / `triage.feature` scenarios must still
  pass (open marks read, star updates row + sidebar, mark-unread flash). They
  already wait on the resulting DOM/flash, which the optimistic response
  provides synchronously.
- **Verification of the fix (manual probe, not committed):** reproduce write
  contention (seed a slow background write / large sync) and confirm the
  entry-open TTFB drops from ~800 ms to tens of ms. Re-source
  `/tmp/rdrs-env.sh`; `cargo build` before any e2e.

## Files Touched

- `src/db/pool.rs` — add `user_detached` (+ a test-only flush sentinel helper);
  unit tests.
- `src/handlers/entries.rs` — rewrite `entry_fragment`, `set_starred_state`,
  `set_read_state` to optimistic-read + `user_detached`; shared optimistic
  `EntryActionMulti` builder.
- `tests/handlers_test.rs` — optimistic-response + eventual-write assertions.

No template, JS, or route changes.

## Risks & Mitigations

- **Ordering races** → avoided by `user_detached` sending in handler order on a
  single FIFO channel (not `tokio::spawn`).
- **Stale optimistic read** (read-your-writes) → accepted; self-heals via poll;
  documented.
- **Lost write on shutdown** → low impact; logged; reconciled on next session's
  reads.
- **Hidden coupling** (a caller relying on the write having completed before the
  response) → audited: the 5 handlers' only post-write step is building the
  response, which we now build optimistically; `fetch-full-content`/`save`/
  `summarize` are explicitly out of scope and unchanged.
