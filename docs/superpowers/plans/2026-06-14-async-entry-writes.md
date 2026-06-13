# Async Entry-State Writes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the 5 pure state-flip entry writes (open=mark-read, read, unread, star, unstar) return in ~tens of ms even while a background feed sync holds the single write actor, by responding optimistically from the read connection and enqueuing the write fire-and-forget.

**Architecture:** Add a non-awaiting `DbPool::user_detached` that enqueues a write-actor closure (FIFO, User priority) and returns immediately. Rewrite the affected handlers to read current state via the read connection (not blocked by the sync's write transaction under WAL), build the existing multi-target response with the state change applied **in memory**, and enqueue the real write via `user_detached`. Feed sync keeps its single-transaction-per-feed behaviour.

**Tech Stack:** Rust (Axum + Askama), rusqlite, tokio mpsc actor DB pool (`src/db/pool.rs`).

---

## Notes for the implementer

- **NixOS box:** `source /tmp/rdrs-env.sh` before EVERY cargo command (OpenSSL env).
- **`pwd` first** for build/test/git; expect `/home/nixos/Develop/claude/rdrs`.
- **Tests:** `cargo nextest run` (NOT `cargo test`), with `RDRS_FAST_HASH=1`.
- **`cargo fmt`** before committing Rust.
- **Commits GPG-signed** (`git commit -S`); end each message with the
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- **Stage files explicitly by name** — never `git add -A` / `git add .`.
- Current branch is `feat/async-entry-writes` (already created).
- **Key invariant:** the affected handlers must NOT `.await` any `state.db.user(...)`
  / `state.db.background(...)` (write actor) call on the request path. Reads use
  `state.db.read_user(...)` (read actor, not blocked by the sync's write txn);
  the write goes through `user_detached` (no await).

## Background facts (verified)

- `DbPool` (`src/db/pool.rs`): write actor (channels `user_tx` / `bg_tx`) + read
  actor (`read_user_tx` / `read_bg_tx`). `user_tx: mpsc::Sender<DbMessage>`,
  buffer 256. `DbMessage { work: BoxedDbFn, respond: oneshot::Sender<...> }`.
- `entry::find_by_id_for_user(conn, user_id, id) -> AppResult<Option<EntryWithFeed>>`.
- `entry::mark_as_read(conn, id) -> AppResult<Entry>`.
- `entry::set_read_for_user(conn, user_id, id, desired) -> AppResult<Option<(EntryWithFeed, bool)>>` (bool = changed).
- `entry::set_starred_for_user(conn, user_id, id, desired) -> AppResult<Option<(EntryWithFeed, bool)>>`.
- `entry::unread_counts_per_feed(conn, user_id) -> AppResult<Vec<entry::UnreadCount>>`; `UnreadCount { feed_id: i64, unread: i64 }` is `serde::Serialize`, fields `pub`.
- `Entry.read_at: Option<DateTime<Utc>>`, `Entry.starred_at: Option<DateTime<Utc>>`; `EntryWithFeed { entry: Entry, feed_title, feed_id (via entry.feed_id), feed_has_icon, category_id, category_name, ... }`.
- `row_view_from(&EntryWithFeed, Option<SummaryStatus>) -> EntryRowView` reads `entry.read_at` / `entry.starred_at`.
- `build_reading_pane_view`, `load_pane_action_flags`, `resolve_summary` all use `read_user` only (read actor) — safe.
- Handlers: `entry_fragment` (returns `OpenEntryMulti`), `set_starred_state` (returns `EntryActionMulti`, sets `pane_star_form`), `set_read_state` (returns `EntryActionMulti`, "Marked as unread." flash when `!desired_read && changed`).

---

## Task 1: Add `DbPool::user_detached` (fire-and-forget write)

**Files:**
- Modify: `src/db/pool.rs` (add method after `background`, ~line 191; add tests in the `mod tests` block)

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `src/db/pool.rs` (before the closing `}` of the module):

```rust
    #[tokio::test]
    async fn test_user_detached_eventually_applies() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE d (id INTEGER PRIMARY KEY, v INTEGER);")
            .unwrap();
        let (pool, _h) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        // Fire-and-forget: returns immediately (no .await on the write itself).
        pool.user_detached(|conn| {
            conn.execute("INSERT INTO d (v) VALUES (1)", []).unwrap();
        });

        // Flush via a FIFO sentinel: a subsequent user() call runs on the same
        // write actor AFTER the detached write, so the row is guaranteed present.
        let count = pool
            .user(|conn| {
                conn.query_row("SELECT COUNT(*) FROM d", [], |r| r.get::<_, i64>(0))
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_user_detached_preserves_submission_order() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE seq (n INTEGER);").unwrap();
        let (pool, _h) = DbPool::new(conn, Connection::open_in_memory().unwrap());

        for i in 0..5 {
            pool.user_detached(move |conn| {
                conn.execute("INSERT INTO seq (n) VALUES (?1)", [i]).unwrap();
            });
        }

        // Sentinel flush, then assert FIFO order (0,1,2,3,4).
        let rows: Vec<i64> = pool
            .user(|conn| {
                let mut stmt = conn.prepare("SELECT n FROM seq ORDER BY rowid").unwrap();
                stmt.query_map([], |r| r.get::<_, i64>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(rows, vec![0, 1, 2, 3, 4]);
    }
```

- [ ] **Step 2: Run to verify they FAIL**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs user_detached 2>&1 | tail -20
```
Expected: compile error — `user_detached` not found.

- [ ] **Step 3: Implement `user_detached`**

In `src/db/pool.rs`, add `use tokio::sync::mpsc::error::TrySendError;` to the
imports (the `use tokio::sync::{mpsc, oneshot};` line can stay; add the error
import on its own line). Then add this method to `impl DbPool` right after the
`background` method (~line 191):

```rust
    /// Enqueue a write-actor closure with User priority and return
    /// immediately, WITHOUT awaiting its result. Ordering is preserved (single
    /// FIFO `user_tx`), so rapid star→unstar still applies in submission order.
    ///
    /// Fire-and-forget: the closure must log its own errors (the caller cannot
    /// observe success/failure). Used for optimistic state-flip writes whose
    /// HTTP response is rendered before the write lands; a dropped write
    /// self-heals on the next sidebar poll / page reload.
    pub fn user_detached<F>(&self, f: F)
    where
        F: FnOnce(&Connection) + Send + 'static,
    {
        // The response receiver is dropped immediately; the actor's
        // `msg.respond.send(...)` then fails silently (already handled in
        // `process_message`). `try_send` never blocks the caller.
        let (resp_tx, _resp_rx) = oneshot::channel();
        let msg = DbMessage {
            work: Box::new(move |conn| {
                f(conn);
                Box::new(()) as Box<dyn std::any::Any + Send>
            }),
            respond: resp_tx,
        };
        match self.user_tx.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                error!("user_detached: write queue full; dropping write (self-heals on next poll)")
            }
            Err(TrySendError::Closed(_)) => {
                error!("user_detached: db actor stopped; write dropped")
            }
        }
    }
```

(`error!` is already imported via `use tracing::{debug, error, info};`.)

- [ ] **Step 4: Run to verify they PASS**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs user_detached 2>&1 | tail -20
cargo build 2>&1 | tail -5
```
Expected: both tests PASS; clean build.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/db/pool.rs
git commit -S -m "feat(db): add user_detached fire-and-forget write (FIFO, non-blocking)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Optimistic sidebar-count helper

**Files:**
- Modify: `src/handlers/entries.rs` (add `build_sidebar_unread_with_delta` next to `build_sidebar_unread`, ~line 340)

- [ ] **Step 1: Implement the helper**

Add after `build_sidebar_unread` in `src/handlers/entries.rs`:

```rust
/// Like `build_sidebar_unread`, but applies an in-memory `delta` to one feed's
/// unread count so an optimistic response (whose DB write hasn't landed yet)
/// shows the correct number. `delta` is `-1` (marked read), `+1` (marked
/// unread), or `0` (no change, e.g. star/unstar). Mirrors
/// `unread_counts_per_feed`'s "positive counts only" shape.
pub(crate) async fn build_sidebar_unread_with_delta(
    state: &AppState,
    user_id: i64,
    feed_id: i64,
    delta: i64,
) -> AppResult<String> {
    let mut counts = state
        .db
        .read_user(move |conn| entry::unread_counts_per_feed(conn, user_id))
        .await??;
    if delta != 0 {
        match counts.iter_mut().find(|c| c.feed_id == feed_id) {
            Some(c) => c.unread = (c.unread + delta).max(0),
            None if delta > 0 => counts.push(entry::UnreadCount {
                feed_id,
                unread: delta,
            }),
            None => {}
        }
        counts.retain(|c| c.unread > 0);
    }
    Ok(serde_json::to_string(&counts).unwrap_or_else(|_| "[]".to_string()))
}
```

- [ ] **Step 2: Do NOT build or commit this standalone**

The helper is unused until Task 3, and CI runs `cargo clippy -- -D warnings`, so
a standalone commit would be clippy-dirty (dead_code). **Leave it uncommitted**
and proceed directly to Task 3, which uses it and commits both together (both
live in `src/handlers/entries.rs`, so one `git add` covers them).

---

## Task 3: Convert entry-open (`entry_fragment`) to optimistic + detached

(Commits the Task 2 helper together with this change.)

**Files:**
- Modify: `src/handlers/entries.rs` (`entry_fragment`, ~lines 87-144)
- Modify: `tests/handlers_test.rs` (extend `test_entry_fragment_renders_reading_pane`)

- [ ] **Step 1: Add the eventual-write assertion (failing on timing? no — additive)**

In `tests/handlers_test.rs`, `test_entry_fragment_renders_reading_pane` already
asserts the entry is read in the DB after the request. Because the write becomes
async, change that read-back to go through the **write actor** (`app.db.user`,
not `read_user`) so FIFO guarantees the detached write has landed. Find the
existing post-request read of `read_at` (the block around
`app.db.read_user(... read_at ...)`) and ensure it uses `app.db.user(...)`.
If it already uses `app.db.user`, no change. Add (if not present):

```rust
    // The mark-as-read write is now async (user_detached); a user() read-back
    // is FIFO-ordered behind it, so it observes the applied write.
    let read_at: Option<String> = app
        .db
        .user(move |conn| {
            conn.query_row("SELECT read_at FROM entry WHERE id = ?1", [entry_id], |r| {
                r.get::<_, Option<String>>(0)
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert!(read_at.is_some(), "entry must be marked read after open");
```

- [ ] **Step 2: Run it (should still PASS pre-change, establishing the read-back works)**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_entry_fragment_renders_reading_pane 2>&1 | tail -15
```
Expected: PASS (current sync impl already marks read; we're just hardening the read-back to `user()`).

- [ ] **Step 3: Rewrite `entry_fragment`**

Replace the body of `entry_fragment` (keep the signature and the
`Sec-Fetch-Dest: document` redirect guard) with:

```rust
pub async fn entry_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    if headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document") {
        return Ok(Redirect::to(&format!("/entries?entry={entry_id}")).into_response());
    }

    // Read current state on the READ connection (not blocked by a background
    // sync's write transaction under WAL).
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let was_unread = ewf.entry.read_at.is_none();
    let feed_id = ewf.entry.feed_id;

    // Optimistically reflect the read state in the rendered row + pane.
    if was_unread {
        ewf.entry.read_at = Some(chrono::Utc::now());
    }

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;
    let pane = build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi).await?;
    let row = row_view_from(&ewf, status);
    let sidebar_unread_payload_json =
        build_sidebar_unread_with_delta(&state, user_id, feed_id, if was_unread { -1 } else { 0 })
            .await?;

    // Enqueue the real write off the critical path (only when it changes state).
    if was_unread {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::mark_as_read(conn, entry_id) {
                tracing::warn!("async mark_as_read failed for entry {entry_id}: {e}");
            }
        });
        state.sidebar_cache.bust(user_id);
    }

    Ok(OpenEntryMulti {
        pane,
        r: row,
        sidebar_unread_payload_json,
    }
    .into_response())
}
```

- [ ] **Step 4: Run the open tests + the reading suite**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_entry_fragment 2>&1 | tail -20
RDRS_FAST_HASH=1 cargo nextest run -p rdrs reading 2>&1 | tail -20
```
Expected: PASS. If a test read the entry's read state via `read_user` and now
races the async write, switch that read-back to `app.db.user(...)` (FIFO flush).

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/handlers/entries.rs tests/handlers_test.rs
git commit -S -m "perf(entries): open entry optimistically, mark read off critical path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Convert star/unstar (`set_starred_state`) to optimistic + detached

**Files:**
- Modify: `src/handlers/entries.rs` (`set_starred_state`, ~lines 365-398)
- Modify: `tests/handlers_test.rs` (star/unstar idempotency tests — harden read-backs to `app.db.user`)

- [ ] **Step 1: Rewrite `set_starred_state`**

Replace `set_starred_state` with:

```rust
/// Shared core for the idempotent star/unstar handlers. Renders the response
/// optimistically and enqueues the write off the critical path.
async fn set_starred_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_starred: bool,
) -> AppResult<EntryActionMulti> {
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let changed = ewf.entry.starred_at.is_some() != desired_starred;

    // Optimistically reflect the new starred state in the row + pane button.
    ewf.entry.starred_at = if desired_starred {
        Some(ewf.entry.starred_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    // Starring does not affect unread counts (delta = 0).
    let payload_json = build_sidebar_unread_with_delta(&state, user_id, ewf.entry.feed_id, 0).await?;
    let pane_star_form = Some(PaneStarFormView {
        id: ewf.entry.id,
        is_starred: ewf.entry.starred_at.is_some(),
    });

    if changed {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::set_starred_for_user(conn, user_id, entry_id, desired_starred) {
                tracing::warn!("async set_starred failed for entry {entry_id}: {e}");
            }
        });
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
        flash: None,
        pane_star_form,
    })
}
```

- [ ] **Step 2: Harden star test read-backs**

In `tests/handlers_test.rs`, any star/unstar test that reads `starred_at` back
from the DB after the POST must use `app.db.user(...)` (write actor, FIFO behind
the detached write), not `read_user`. Find those read-backs and switch them.
(The HTML-level assertions on `aria-label="Unstar"` etc. already pass from the
optimistic response and need no change.)

- [ ] **Step 3: Run star/unstar tests**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs star 2>&1 | tail -20
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/handlers/entries.rs tests/handlers_test.rs
git commit -S -m "perf(entries): star/unstar optimistically, write off critical path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Convert read/unread buttons (`set_read_state`) to optimistic + detached

**Files:**
- Modify: `src/handlers/entries.rs` (`set_read_state`, ~lines 423-463)
- Modify: `tests/handlers_test.rs` (read/unread read-backs → `app.db.user`)

- [ ] **Step 1: Rewrite `set_read_state`**

Replace `set_read_state` with:

```rust
/// Shared core for the two idempotent read/unread handlers. Renders the
/// response optimistically and enqueues the write off the critical path.
async fn set_read_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_read: bool,
) -> AppResult<EntryActionMulti> {
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let changed = ewf.entry.read_at.is_some() != desired_read;
    let feed_id = ewf.entry.feed_id;

    // Optimistically reflect the new read state in the row.
    ewf.entry.read_at = if desired_read {
        Some(ewf.entry.read_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    // Unread count: -1 when newly read, +1 when newly unread, else unchanged.
    let delta = if changed {
        if desired_read {
            -1
        } else {
            1
        }
    } else {
        0
    };
    let payload_json = build_sidebar_unread_with_delta(&state, user_id, feed_id, delta).await?;

    let flash = if !desired_read && changed {
        Some(FlashPayload {
            level: "success",
            message: "Marked as unread.".to_string(),
        })
    } else {
        None
    };

    if changed {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::set_read_for_user(conn, user_id, entry_id, desired_read) {
                tracing::warn!("async set_read failed for entry {entry_id}: {e}");
            }
        });
        state.sidebar_cache.bust(user_id);
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
        flash,
        pane_star_form: None,
    })
}
```

- [ ] **Step 2: Harden read/unread test read-backs**

In `tests/handlers_test.rs`, switch any post-POST `read_at` DB read-back in the
read/unread tests to `app.db.user(...)`. The "Marked as unread." flash assertion
and row-class assertions pass from the optimistic response unchanged.

- [ ] **Step 3: Run read/unread + the broader entries suite**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs unread 2>&1 | tail -20
RDRS_FAST_HASH=1 cargo nextest run -p rdrs entries 2>&1 | tail -20
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/handlers/entries.rs tests/handlers_test.rs
git commit -S -m "perf(entries): mark read/unread optimistically, write off critical path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Full regression + latency verification

**Files:** none (verification only)

- [ ] **Step 1: Full Rust suite + fmt**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt --check
RDRS_FAST_HASH=1 cargo nextest run -p rdrs 2>&1 | tail -25
```
Expected: all tests PASS, fmt clean. Pay attention to any test that reads entry
state back via `read_user` after a state-flip POST/GET and now flakes — convert
it to `app.db.user(...)` (FIFO flush) and re-run.

- [ ] **Step 2: e2e — reading + triage + responsive**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -5
cd e2e && npx playwright test reading triage responsive 2>&1 | tail -30
```
Expected: all PASS (open marks read, star updates row + sidebar, mark-unread
flash, summarize/fetch unchanged). These assertions read the DOM/flash that the
optimistic response provides synchronously.

- [ ] **Step 3: Manual latency probe (NOT committed) — prove the fix**

Reproduce write-actor contention and confirm entry-open TTFB drops. Run a throwaway
server with a large seeded DB, hold the write actor with a slow background write,
and measure. Example skeleton (adapt ports/paths; clean up after):

```bash
pwd
source /tmp/rdrs-env.sh
rm -f /tmp/rdrs-probe.sqlite3*
DATABASE_URL=/tmp/rdrs-probe.sqlite3 SERVER_PORT=8799 SIGNUP_ENABLED=true \
  IMAGE_PROXY_SECRET=probe RDRS_FAST_HASH=1 ./target/debug/rdrs &
# register+login (curl cookie jar), seed a feed+entries + a target entry via
# python3/sqlite3 (see the prior probe in the session transcript), then while a
# large background write is in flight, measure:
#   curl -s -o /dev/null -b cookies -w '%{time_starttransfer}\n' \
#     http://localhost:8799/entries/<id>/fragment
# Expected: tens of ms even under contention (was ~800ms).
pkill -9 -f '/target/debug/rdrs'; rm -f /tmp/rdrs-probe.sqlite3*
```

This step is for confidence only; it produces no commit. Report the measured
before/after numbers in the task summary.

---

## Self-Review

**Spec coverage:**
- `DbPool::user_detached` (FIFO, non-blocking, logs/drops on full/closed) → Task 1. ✅
- Optimistic sidebar ±1 delta → Task 2 (`build_sidebar_unread_with_delta`), used in Tasks 3 & 5. ✅
- Entry open: read-path + optimistic read state + detached mark-read → Task 3. ✅
- Star/unstar: optimistic + detached, sidebar delta 0, pane_star_form from optimistic state → Task 4. ✅
- Read/unread: optimistic + detached, ±1 delta, "Marked as unread." flash preserved → Task 5. ✅
- Idempotency: no write / no delta when state unchanged (`changed` / `was_unread` guards) → Tasks 3-5. ✅
- Feed sync single transaction untouched (no edits to `feed_sync.rs`). ✅
- No template/JS/route changes (only `pool.rs`, `entries.rs`, tests). ✅
- Eventual-consistency test technique (FIFO `user()` sentinel) → Task 1 tests + hardened read-backs in Tasks 3-5. ✅
- Latency verification → Task 6 Step 3. ✅

**Placeholder scan:** No TBD/TODO; every code step is concrete. Task 2 Step 2
intentionally defers the helper's verification to the handler tasks (stated
explicitly), not a placeholder.

**Type/name consistency:** `user_detached<F: FnOnce(&Connection) + Send + 'static>`
defined Task 1, called identically in Tasks 3-5. `build_sidebar_unread_with_delta(state, user_id, feed_id, delta: i64)` defined Task 2, called in Tasks 3 (delta -1/0) & 5 (±1) and with delta 0 in Task 4. `entry::set_read_for_user` / `set_starred_for_user` / `mark_as_read` signatures match the verified facts. `ewf.entry.read_at` / `starred_at` are `Option<DateTime<Utc>>`, mutated with `chrono::Utc::now()`. ✅
