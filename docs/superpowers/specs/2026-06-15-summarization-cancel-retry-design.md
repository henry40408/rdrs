# Summarization: Cancel, Timeout & Failed-State Recovery

**Date:** 2026-06-15
**Status:** Approved (design)

## Problem

AI summarization currently cannot be stopped once requested, and a failed
summary has no recovery path in the UI:

- The summary worker is a single Tokio task draining an MPSC channel one job at
  a time. There is **no per-job cancellation handle** — only a global
  graceful-shutdown `CancellationToken`. A job that is queued or in-flight
  cannot be aborted.
- `summarize_with_kagi` has **no timeout**. A hung Kagi request occupies the one
  worker indefinitely, blocking every user's subsequent summaries.
- In the reading pane, the `failed` status renders **nothing** — the
  `#rp-summary-container` only handles `summary_in_flight` and the
  completed-`summary_text` branches. Users cannot see the error, retry, or clear
  it from the UI. (A `DELETE /api/entries/{id}/summary` JSON endpoint exists, but
  it is only wired to the completed-summary "Dismiss" button via JS.)

## Goals

1. Allow an in-progress or queued summarization to be **cancelled** from the UI.
2. Give the **failed** state a visible error message plus **Retry** and
   **Clear** actions in the UI.
3. Add a request **timeout** so a hung Kagi call cannot wedge the worker.

## Non-Goals

- No `cancelled` status / no schema migration — cancelling deletes the record.
- No controls in the list view; the list `summary-badge-*` stays indicator-only.
- No change to the completed-summary "Dismiss" path (existing JS + DELETE API).
- No parallelism / priority changes to the worker.

## Behavioural Decisions

| Item | Decision |
|---|---|
| Cancel scope | Both `pending` (queued) and `processing` (in-flight) |
| State after cancel | Record deleted → back to "no summary" (no badge) |
| Timeout | Kagi request times out at **90 s** → `failed` with message `Summarization timed out` |
| Control placement | Reading pane only |
| Failed presentation | Banner-style error box reusing the app's `--color-error` palette (mirrors `.banner--error`) |

## Architecture

### Backend — cancellation registry

Add a shared, per-entry token registry to `AppState`:

```rust
// (user_id, entry_id) -> CancellationToken
summary_cancels: Arc<Mutex<HashMap<(i64, i64), CancellationToken>>>
```

Reuses `tokio_util::sync::CancellationToken` (already used for worker shutdown).
The registry is created in `main.rs` and cloned into both the worker and
`AppState`. A `std::sync::Mutex` is sufficient — critical sections are a single
insert / remove / clone.

Because processing is sequential there is at most one in-flight job, but the
channel can hold up to 100 queued jobs, so a per-key registry (not a single
"current job" slot) is required to cancel a *specific* queued entry.

### Backend — worker changes (`services/summary_worker.rs`)

`process_summary_job` becomes:

1. Get-or-insert the `CancellationToken` for `(user_id, entry_id)` from the
   registry. (Startup-restored jobs from `find_incomplete` flow through here
   too — no special-casing needed.)
2. If `token.is_cancelled()` → the cancel handler already cleaned up; remove the
   token and skip to the next job.
3. `entry_summary::set_processing` + `cache.set_processing`.
4. Race cancellation against the timed Kagi call:

   ```rust
   tokio::select! {
       _ = token.cancelled() => {
           // Handler owns cleanup (delete + cache remove + sidebar bust).
           // Just drop the Kagi future; never write a result back.
       }
       res = tokio::time::timeout(SUMMARY_TIMEOUT, summarize_with_kagi(...)) => {
           match res {
               Ok(Ok(text)) => /* set_completed + cache + sidebar bust */,
               Ok(Err(e))   => /* set_failed(e) */,
               Err(_elapsed) => /* set_failed("Summarization timed out") */,
           }
       }
   }
   ```

   `SUMMARY_TIMEOUT` = `Duration::from_secs(90)`, a module constant.
5. Remove the token from the registry on completion / failure.

**Race safety:** once `token.cancelled()` is ready, `tokio::select!` drops the
Kagi branch, so `set_completed`/`set_failed` never run for a cancelled job. DB
writes serialize on the single write connection, so the handler's `delete` lands
after any in-flight `set_processing`; the net result is a deleted record.

### Backend — endpoints (`handlers/entries.rs`)

| Action | Endpoint | Behaviour |
|---|---|---|
| **Retry** | reuse `POST /entries/{id}/summarize` | Existing `upsert_pending` resets `failed` → `pending`, clears `summary_text`/`error_message`, re-enqueues. No new code. |
| **Cancel / Clear** | **new** `POST /entries/{id}/summarize/cancel` | Validate ownership (`find_by_id_for_user` → 404). If a token exists, `token.cancel()` and remove it. `entry_summary::delete` + `summary_cache.remove` + `sidebar_cache.bust`. Return the empty `#rp-summary-container` fragment. |

Cancel and Clear share one endpoint: both mean "stop and remove this summary." A
`failed` record simply has no live token, so the token lookup misses and the
handler just deletes. SSR form-POST; no new JS.

### Frontend

**`ReadingPaneView` (`handlers/pages/mod.rs`)** — add:

```rust
pub summary_error: Option<String>,  // Some(error_message) ⟹ failed
```

`Some` distinguishes `failed` from "no summary" (both currently
`summary_text = None`, `summary_in_flight = false`). The builder populates it
with `entry_summary.error_message` when status is `Failed`.

**`templates/_reading_pane.html`** — `#rp-summary-container` becomes three
branches (priority order):

1. `summary_in_flight` → `summary-box` with `Summarizing… (refresh to see the
   result)` **plus a Cancel button** (`rp-action`, form `POST
   /entries/{id}/summarize/cancel`, `data-swap="#rp-summary-container"`).
2. `summary_text` (completed) → **unchanged** (Copy / Dismiss).
3. `summary_error` (failed) → `summary-box` containing:
   - `summary-actions` with **Retry** (form `POST /entries/{id}/summarize`) and
     **Clear** (form `POST /entries/{id}/summarize/cancel`), both `rp-action`,
     buttons left-aligned, each `data-swap="#rp-summary-container"`.
   - a **`.summary-error-banner`** element showing the error message.

**`templates/_summarize_pending.html`** — add the Cancel button to the in-flight
fragment so a freshly-queued summary is immediately cancellable.

**New fragment** — the cancel/clear response: an empty `#rp-summary-container`
(`<template data-swap-target="#rp-summary-container">` wrapping the empty
container) so the swap helper returns the pane to the no-summary state.

**`static/css/app.css`** — add `.summary-error-banner`, mirroring
`.banner--error`:

```css
.summary-error-banner {
    display: flex; align-items: flex-start; gap: var(--space-2);
    font-size: var(--font-sm); color: var(--color-text); line-height: 1.5;
    background: light-dark(rgba(185,28,28,0.06), rgba(248,113,113,0.08));
    border: 1px solid light-dark(rgba(185,28,28,0.20), rgba(248,113,113,0.25));
    border-left: var(--border-accent-width) solid var(--color-error);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
}
.summary-error-banner .action-icon { color: var(--color-error); flex: none; }
```

**`templates/_icons.html`** — add a `refresh`/`retry` SVG icon for the Retry
button (Cancel/Clear reuse the existing `close` icon; the in-flight Cancel and
failed Clear both use `close`).

## Testing

**Rust (`cargo nextest run`, `RDRS_FAST_HASH=1` locally):**

- `POST /entries/{id}/summarize/cancel`:
  - pending path — record deleted, cache cleared, returns empty container.
  - processing path — token cancelled, record deleted, no result written.
  - failed path (Clear) — record deleted.
  - non-owner → 404.
- Worker: a cancelled token causes the job to be skipped / aborted with no
  `completed`/`failed` write.
- Worker: a Kagi call exceeding the timeout transitions the record to `failed`
  with the timeout message. (Inject a slow/stub summarizer or a small test-only
  timeout.)
- Retry: re-`POST /summarize` on a `failed` record resets it to `pending`.

**E2E (Playwright BDD, `e2e/`):**

- Failed summary renders the error banner with Retry + Clear; Clear removes the
  box; Retry returns it to the in-flight state.
- In-flight summary shows a Cancel button; cancelling returns the pane to the
  no-summary state.

## Build & Docs

- UI changes → `cargo build` then `cd e2e && npm run screenshots`; refresh the
  four `screenshots/` images **only if** a default screenshot scenario is
  affected (the generator captures the unread list + reading pane and the
  keyboard-help overlay — the summary states are not in the default capture, so
  screenshots likely do **not** change; verify and update only if they do).
- Update doc comments / `ARCHITECTURE.md` text that states the summary worker
  has no cancellation and no timeout.

## Files Touched (summary)

- `src/main.rs` — create the cancel registry; pass it to the worker.
- `src/lib.rs` — `AppState.summary_cancels` field; register `POST
  /entries/{id}/summarize/cancel` next to the existing `summarize_entry_form`
  route (line ~208).
- `src/services/summary_worker.rs` — get-or-insert token, `select!` with
  timeout + cancellation, token cleanup, `SUMMARY_TIMEOUT` const.
- `src/handlers/entries.rs` — new `summarize_cancel_form` handler.
- `src/handlers/pages/mod.rs` — `ReadingPaneView.summary_error` + builder fill.
- `templates/_reading_pane.html`, `templates/_summarize_pending.html`,
  new cancel-response fragment, `templates/_icons.html`.
- `static/css/app.css` — `.summary-error-banner`.
- Tests under `tests/` + `e2e/features/`.
