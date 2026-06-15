# Summarization Cancel / Retry / Timeout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users cancel an in-flight/queued summarization and recover a failed one (Retry / Clear) from the reading pane, and add a Kagi request timeout so a hung call can't wedge the single worker.

**Architecture:** A shared per-entry `CancellationToken` registry in `AppState` lets a new `POST /entries/{id}/summarize/cancel` handler abort a job; the worker races the Kagi call against that token and a 90 s timeout via an extracted `run_summary` helper. The reading pane gains a failed-state branch (error banner + Retry + Clear) and an in-flight Cancel button; Retry reuses the existing summarize endpoint, Cancel/Clear share the new cancel endpoint (which deletes the record).

**Tech Stack:** Rust (Axum, Askama, rusqlite, tokio, `tokio_util::sync::CancellationToken`), vanilla CSS, Playwright BDD (e2e).

**Spec:** `docs/superpowers/specs/2026-06-15-summarization-cancel-retry-design.md`

**Before you start — environment:** This is a NixOS box; re-source `/tmp/rdrs-env.sh` before every `cargo`/`npm` command (OpenSSL env vars). Run Rust tests with `RDRS_FAST_HASH=1 cargo nextest run`. Always `cargo fmt` before committing and keep `cargo clippy -- -D warnings` clean.

---

## File Structure

- `src/services/summary_worker.rs` — add `CancelRegistry` type alias, `SUMMARY_TIMEOUT` const, `SummaryOutcome` enum + `run_summary` helper, thread the registry through `start_summary_worker` / `process_summary_job`.
- `src/services/mod.rs` — re-export `CancelRegistry`.
- `src/lib.rs` — `AppState.summary_cancels` field; register the cancel route.
- `src/main.rs` — create the registry, pass to worker + `AppState`.
- `src/handlers/entries.rs` — `SummarizeCleared` response type, `summarize_cancel_form` handler, extend `resolve_summary` + `build_reading_pane_view` with `summary_error`.
- `src/handlers/pages/mod.rs` — add `summary_error` field to `ReadingPaneView`.
- `templates/_reading_pane.html` — Cancel button (in-flight) + failed branch.
- `templates/_summarize_pending.html` — Cancel button.
- `templates/_summary_cleared.html` — **new** empty-container fragment.
- `templates/_icons.html` — `refresh` macro.
- `static/css/app.css` — `.summary-error-banner`.
- `e2e/support/seed.js`, `e2e/steps/*.steps.js`, `e2e/features/reading.feature` — failed-state coverage.
- `ARCHITECTURE.md` + worker doc comments — note cancellation + timeout now exist.

> **Note on `build_reading_pane_view`:** it is the *single* `pub(crate)` builder in `src/handlers/entries.rs` used by the fragment endpoints **and** by `maybe_build_reading_pane` in `src/handlers/pages/mod.rs` for full-page renders. Changing it once covers every reading-pane render path.

---

## Task 1: `run_summary` helper + timeout constant (pure, testable)

**Files:**
- Modify: `src/services/summary_worker.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/services/summary_worker.rs`:

```rust
    #[tokio::test]
    async fn run_summary_completes() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            Ok::<String, String>("hello".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Completed(s) if s == "hello"));
    }

    #[tokio::test]
    async fn run_summary_propagates_failure() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            Err::<String, String>("boom".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Failed(e) if e == "boom"));
    }

    #[tokio::test]
    async fn run_summary_times_out() {
        let token = CancellationToken::new();
        let out = run_summary(&token, std::time::Duration::from_millis(20), async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok::<String, String>("late".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Failed(e) if e == "Summarization timed out"));
    }

    #[tokio::test]
    async fn run_summary_cancels_before_completion() {
        let token = CancellationToken::new();
        token.cancel();
        let out = run_summary(&token, std::time::Duration::from_secs(1), async {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok::<String, String>("never".to_string())
        })
        .await;
        assert!(matches!(out, SummaryOutcome::Cancelled));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs run_summary 2>&1 | tail -20`
Expected: compile error — `run_summary` / `SummaryOutcome` not found.

- [ ] **Step 3: Implement the helper + constant**

Add near the top of `src/services/summary_worker.rs` (after the imports):

```rust
use std::time::Duration;

/// Maximum wall-clock time for a single Kagi summarization request. A hung
/// request would otherwise occupy the single worker indefinitely and block
/// every user's queued summaries. NEVER lower this in production without
/// confirming Kagi's worst-case latency.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// Result of racing a summarization future against cancellation + timeout.
pub(crate) enum SummaryOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
}

/// Race a summarization future against an external cancellation token and a
/// hard timeout. `biased` makes cancellation win deterministically when the
/// token is already cancelled. On timeout the future is dropped (the in-flight
/// HTTP request is aborted) and a `Failed("Summarization timed out")` is
/// returned.
pub(crate) async fn run_summary<F>(
    token: &CancellationToken,
    timeout: Duration,
    fut: F,
) -> SummaryOutcome
where
    F: std::future::Future<Output = Result<String, String>>,
{
    tokio::select! {
        biased;
        _ = token.cancelled() => SummaryOutcome::Cancelled,
        res = tokio::time::timeout(timeout, fut) => match res {
            Ok(Ok(text)) => SummaryOutcome::Completed(text),
            Ok(Err(e)) => SummaryOutcome::Failed(e),
            Err(_elapsed) => SummaryOutcome::Failed("Summarization timed out".to_string()),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs run_summary 2>&1 | tail -20`
Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/services/summary_worker.rs
git commit -S -m "feat: add run_summary cancel/timeout race helper"
```

---

## Task 2: Thread a cancellation registry through the worker

**Files:**
- Modify: `src/services/summary_worker.rs`
- Modify: `src/services/mod.rs`
- Modify: `src/lib.rs` (AppState field)
- Modify: `src/main.rs` (create + wire registry)

- [ ] **Step 1: Add the registry type and re-export**

At the top of `src/services/summary_worker.rs`, extend the imports and add the alias:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
```

Add after the `SummaryJob` struct:

```rust
/// Per-entry cancellation tokens for in-flight / queued summary jobs, keyed by
/// `(user_id, entry_id)`. The cancel handler cancels + removes the token; the
/// worker creates one on dequeue (if absent) and removes it when the job ends.
pub type CancelRegistry = Arc<Mutex<HashMap<(i64, i64), CancellationToken>>>;
```

In `src/services/mod.rs`, extend the `summary_worker` re-export (currently
`pub use summary_worker::{create_summary_channel, recover_incomplete_jobs, start_summary_worker, SummaryJob, ...}`) to also export `CancelRegistry`:

```rust
pub use summary_worker::{
    create_summary_channel, recover_incomplete_jobs, start_summary_worker, CancelRegistry,
    SummaryJob,
};
```

(Keep any other names already in that brace list; add `CancelRegistry` alphabetically.)

- [ ] **Step 2: Add the `cancels` param to `start_summary_worker` and `process_summary_job`**

Change the `start_summary_worker` signature to accept the registry and pass it into both `process_summary_job` call sites:

```rust
pub fn start_summary_worker(
    mut rx: mpsc::Receiver<SummaryJob>,
    cache: Arc<SummaryCache>,
    sidebar_cache: Arc<SidebarCache>,
    db: DbPool,
    cancels: CancelRegistry,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!("Summary worker started");

        loop {
            let job = tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Summary worker stopping, draining remaining jobs...");
                    while let Ok(job) = rx.try_recv() {
                        process_summary_job(&job, &cache, &sidebar_cache, &db, &cancels).await;
                    }
                    break;
                }
                job = rx.recv() => {
                    match job {
                        Some(job) => job,
                        None => break,
                    }
                }
            };

            process_summary_job(&job, &cache, &sidebar_cache, &db, &cancels).await;
        }

        tracing::info!("Summary worker stopped");
    })
}
```

- [ ] **Step 3: Restructure `process_summary_job` to use the registry + `run_summary`**

Replace the whole `process_summary_job` function with an outer (token lifecycle) + inner (work) split:

```rust
async fn process_summary_job(
    job: &SummaryJob,
    cache: &Arc<SummaryCache>,
    sidebar_cache: &Arc<SidebarCache>,
    db: &DbPool,
    cancels: &CancelRegistry,
) {
    let key = (job.user_id, job.entry_id);

    // Get-or-create this job's cancellation token. Covers startup-recovered
    // jobs too (they never pass through the enqueue handler).
    let token = {
        let mut map = cancels.lock().unwrap();
        map.entry(key)
            .or_insert_with(CancellationToken::new)
            .clone()
    };

    // Cancelled while still queued — the cancel handler already deleted the
    // record. Drop the token and skip.
    if token.is_cancelled() {
        cancels.lock().unwrap().remove(&key);
        return;
    }

    run_summary_job_body(job, cache, sidebar_cache, db, &token).await;

    cancels.lock().unwrap().remove(&key);
}

async fn run_summary_job_body(
    job: &SummaryJob,
    cache: &Arc<SummaryCache>,
    sidebar_cache: &Arc<SidebarCache>,
    db: &DbPool,
    token: &CancellationToken,
) {
    tracing::debug!(
        "Processing summary job: user={}, entry={}, link={}",
        job.user_id,
        job.entry_id,
        job.entry_link
    );

    // Mark as processing in both cache and DB
    cache.set_processing(job.user_id, job.entry_id);
    {
        let user_id = job.user_id;
        let entry_id = job.entry_id;
        let _ = db
            .background(move |conn| entry_summary::set_processing(conn, user_id, entry_id))
            .await;
    }

    // Get Kagi config for the user
    let user_id = job.user_id;
    let entry_id = job.entry_id;
    let kagi_config = match db
        .background(move |conn| user_settings::get_save_services_config(conn, user_id))
        .await
    {
        Ok(Ok(config)) => config.kagi,
        Ok(Err(e)) => {
            tracing::error!("Failed to get user settings: {}", e);
            let error_msg = "Failed to load Kagi settings".to_string();
            cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
            let _ = db
                .background(move |conn| {
                    entry_summary::set_failed(conn, user_id, entry_id, &error_msg)
                })
                .await;
            return;
        }
        Err(e) => {
            tracing::error!("Failed to access DB: {}", e);
            let error_msg = "Internal error: DB access failed".to_string();
            cache.set_failed(job.user_id, job.entry_id, error_msg);
            return;
        }
    };

    let kagi_config = match kagi_config {
        Some(c) if c.is_configured() => c,
        _ => {
            let error_msg = "Kagi is not configured".to_string();
            cache.set_failed(job.user_id, job.entry_id, error_msg.clone());
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let _ = db
                .background(move |conn| {
                    entry_summary::set_failed(conn, user_id, entry_id, &error_msg)
                })
                .await;
            return;
        }
    };

    // Race the Kagi call against cancellation + timeout.
    match run_summary(
        token,
        SUMMARY_TIMEOUT,
        summarize_with_kagi(&kagi_config, &job.entry_link),
    )
    .await
    {
        SummaryOutcome::Completed(summary_text) => {
            tracing::debug!(
                "Summary completed for entry {}: {} chars",
                job.entry_id,
                summary_text.len()
            );
            cache.set_completed(job.user_id, job.entry_id, summary_text.clone());
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let _ = db
                .background(move |conn| {
                    entry_summary::set_completed(conn, user_id, entry_id, &summary_text)
                })
                .await;
            // A summary just completed — the sidebar "Summarized" badge must tick up.
            sidebar_cache.bust(job.user_id);
        }
        SummaryOutcome::Failed(error) => {
            tracing::warn!("Summary failed for entry {}: {}", job.entry_id, error);
            cache.set_failed(job.user_id, job.entry_id, error.clone());
            let user_id = job.user_id;
            let entry_id = job.entry_id;
            let _ = db
                .background(move |conn| entry_summary::set_failed(conn, user_id, entry_id, &error))
                .await;
        }
        SummaryOutcome::Cancelled => {
            // The cancel handler owns cleanup (delete + cache remove + sidebar
            // bust). Write nothing back.
            tracing::debug!("Summary cancelled for entry {}", job.entry_id);
        }
    }
}
```

- [ ] **Step 4: Fix the existing worker test call sites**

Every existing `start_summary_worker(...)` call in the test module now needs a
`cancels` argument. Add this helper at the top of the `tests` module and pass
`registry()` as the 5th argument (before `cancel_token`) in
`test_worker_stops_on_cancellation`, `test_worker_stops_when_channel_closed`,
and `test_worker_drains_jobs_on_cancellation`:

```rust
    fn registry() -> CancelRegistry {
        Arc::new(Mutex::new(HashMap::new()))
    }
```

Example edit (apply the same shape to all three call sites):

```rust
        let handle = start_summary_worker(
            rx,
            cache,
            Arc::new(SidebarCache::default()),
            db,
            registry(),
            cancel_token.clone(),
        );
```

- [ ] **Step 5: Wire the registry into `AppState` and `main.rs`**

In `src/lib.rs`, add the field to `AppState` (after `sidebar_cache`):

```rust
    pub summary_cancels: services::CancelRegistry,
```

In `src/main.rs`, create the registry before `start_summary_worker` and pass it
in both places. After the `sidebar_cache` line (`let sidebar_cache = ...`):

```rust
    // Per-entry cancellation tokens for summary jobs (cancel/abort support)
    let summary_cancels: rdrs::services::CancelRegistry =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
```

Update the worker start call to pass `summary_cancels.clone()` as the new 5th arg:

```rust
    let summary_worker_handle = services::start_summary_worker(
        summary_rx,
        summary_cache.clone(),
        sidebar_cache.clone(),
        db.clone(),
        summary_cancels.clone(),
        cancel_token.clone(),
    );
```

Add the field to the `AppState { ... }` initializer (after `sidebar_cache`):

```rust
        summary_cancels,
```

- [ ] **Step 6: Build + run the worker tests**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs summary_worker 2>&1 | tail -25`
Expected: all `summary_worker` tests PASS (existing + Task 1's).
Run: `cargo clippy -- -D warnings 2>&1 | tail -15`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/services/summary_worker.rs src/services/mod.rs src/lib.rs src/main.rs
git commit -S -m "feat: per-entry cancellation registry + timeout in summary worker"
```

---

## Task 3: Cancel / Clear endpoint + cleared-container fragment

**Files:**
- Create: `templates/_summary_cleared.html`
- Modify: `src/handlers/entries.rs` (new `SummarizeCleared` type + `summarize_cancel_form` handler)
- Modify: `src/lib.rs` (route)
- Test: `tests/summary_cancel.rs` (new integration test)

- [ ] **Step 1: Create the cleared-container fragment template**

Create `templates/_summary_cleared.html`:

```html
{# `POST /entries/{id}/summarize/cancel` response. Cancels any in-flight /
   queued job and deletes the summary record, returning the summary container
   to its empty (no-summary) state. Swaps ONLY `#rp-summary-container` so the
   article body is untouched. #}
<template data-swap-target="#rp-summary-container">
    <div class="rp-summary-container" id="rp-summary-container" data-summary-container></div>
</template>
```

- [ ] **Step 2: Add the `SummarizeCleared` response type**

In `src/handlers/entries.rs`, directly after the `SummarizePending` `IntoResponse`
impl (around line 81), add:

```rust
/// Response for `POST /entries/{id}/summarize/cancel`. Swaps
/// `#rp-summary-container` back to its empty state after a cancel / clear.
#[derive(Template)]
#[template(path = "_summary_cleared.html")]
pub struct SummarizeCleared;

impl IntoResponse for SummarizeCleared {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}
```

- [ ] **Step 3: Add the `summarize_cancel_form` handler**

In `src/handlers/entries.rs`, directly after `summarize_entry_form` (ends ~line
639), add:

```rust
/// `POST /entries/{id}/summarize/cancel` — cancel an in-flight / queued
/// summarization (or clear a failed one) and delete the record, returning the
/// summary container to its empty state.
///
/// Cancel (in-flight) and Clear (failed) share this endpoint: both mean "stop
/// and remove this summary". A failed record simply has no live token, so the
/// registry lookup misses and we just delete. Ownership is enforced by
/// `find_by_id_for_user`'s join constraint (404 otherwise).
pub async fn summarize_cancel_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummarizeCleared> {
    let user_id = auth_user.user.id;

    // Validate ownership and delete the record in one write txn.
    state
        .db
        .user(move |conn| {
            entry::find_by_id_for_user(conn, user_id, entry_id)?
                .ok_or(AppError::EntryNotFound)?;
            entry_summary::delete(conn, user_id, entry_id)?;
            Ok::<_, crate::error::AppError>(())
        })
        .await??;

    // Cancel + drop any in-flight / queued token for this entry.
    let token = {
        let mut map = state.summary_cancels.lock().unwrap();
        map.remove(&(user_id, entry_id))
    };
    if let Some(token) = token {
        token.cancel();
    }

    state.summary_cache.remove(user_id, entry_id);
    state.sidebar_cache.bust(user_id);

    Ok(SummarizeCleared)
}
```

> If `entry`, `entry_summary`, `PageAuthUser`, `AppError`, `AxumPath`, `State`,
> `Template`, `Html`, `Response`, `StatusCode` are not already imported in this
> file, they are — `summarize_entry_form` above uses all of them. No new `use`
> lines needed.

- [ ] **Step 4: Register the route**

In `src/lib.rs`, directly after the existing `/entries/{id}/summarize` route
(around line 207-209), add:

```rust
        .route(
            "/entries/{id}/summarize/cancel",
            post(handlers::entries::summarize_cancel_form),
        )
```

- [ ] **Step 5: Write the failing integration test**

Look at an existing integration test under `tests/` to copy the app/login
harness (e.g. how a test boots `create_router(AppState{..})` and authenticates).
Create `tests/summary_cancel.rs` mirroring that harness, with:

```rust
// Use the same test harness as the other tests/ files: build an AppState with
// an in-memory DB + a real CancelRegistry, log in, seed an entry.
//
// Assertions (use the project's existing request helpers):
// 1. Seed a `failed` summary for an owned entry, POST /entries/{id}/summarize/cancel,
//    assert 200 and that entry_summary::find_by_user_and_entry returns None.
// 2. POST /entries/{id}/summarize/cancel for an entry the user does NOT own
//    returns 404.
// 3. Insert a CancellationToken into state.summary_cancels for (user, entry),
//    POST cancel, assert the token is now cancelled() and removed from the map.
```

Implement these three `#[tokio::test]` cases concretely against the harness you
copied. The key assertions in code:

```rust
    // (1) record deleted
    let gone = state
        .db
        .read_user(move |c| rdrs::models::entry_summary::find_by_user_and_entry(c, uid, eid))
        .await
        .unwrap()
        .unwrap();
    assert!(gone.is_none());

    // (3) token cancelled + removed
    assert!(token.is_cancelled());
    assert!(state.summary_cancels.lock().unwrap().get(&(uid, eid)).is_none());
```

- [ ] **Step 6: Run the test to verify it fails, then passes after wiring**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs --test summary_cancel 2>&1 | tail -25`
Expected: FAIL first (route/handler missing) → after Steps 1-4, PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add templates/_summary_cleared.html src/handlers/entries.rs src/lib.rs tests/summary_cancel.rs
git commit -S -m "feat: POST /entries/{id}/summarize/cancel (cancel + clear)"
```

---

## Task 4: Surface `summary_error` through the reading-pane builder

**Files:**
- Modify: `src/handlers/pages/mod.rs` (`ReadingPaneView` struct)
- Modify: `src/handlers/entries.rs` (`resolve_summary` + `build_reading_pane_view`)

- [ ] **Step 1: Add the field to `ReadingPaneView`**

In `src/handlers/pages/mod.rs`, add to the `ReadingPaneView` struct after
`pub summary_in_flight: bool,`:

```rust
    /// `Some(error_message)` when the latest summary attempt failed. Lets the
    /// reading pane render the failed branch (error banner + Retry / Clear) and
    /// distinguishes `failed` from "no summary" (both leave `summary_text` None
    /// and `summary_in_flight` false).
    pub summary_error: Option<String>,
```

- [ ] **Step 2: Extend `resolve_summary` to return the error**

In `src/handlers/entries.rs`, change `resolve_summary`'s return type to a
3-tuple and populate the failed branch:

```rust
async fn resolve_summary(
    state: &AppState,
    user_id: i64,
    entry_id: i64,
) -> AppResult<(Option<String>, bool, Option<String>)> {
    if let Some(cached) = state.summary_cache.get(user_id, entry_id) {
        match cached.status {
            SummaryStatus::Completed => return Ok((cached.summary_text, false, None)),
            SummaryStatus::Pending | SummaryStatus::Processing => return Ok((None, true, None)),
            SummaryStatus::Failed => {
                // Fall through to DB — a retry may have refreshed the row
                // without yet updating the cache.
            }
        }
    }
    let db_entry = state
        .db
        .read_user(move |conn| entry_summary::find_by_user_and_entry(conn, user_id, entry_id))
        .await??;
    match db_entry {
        Some(s) => match s.status {
            SummaryStatus::Completed => Ok((s.summary_text, false, None)),
            SummaryStatus::Pending | SummaryStatus::Processing => Ok((None, true, None)),
            SummaryStatus::Failed => Ok((None, false, s.error_message)),
        },
        None => Ok((None, false, None)),
    }
}
```

- [ ] **Step 3: Pass `summary_error` into the `ReadingPaneView`**

In `build_reading_pane_view` (the `ReadingPaneView { ... }` construction around
line 223-253), update the `resolve_summary` destructuring and add the field:

```rust
    let (summary_text, summary_in_flight, summary_error) =
        resolve_summary(state, user_id, entry_id).await?;
```

and in the struct initializer, after `summary_in_flight,`:

```rust
        summary_error,
```

- [ ] **Step 4: Build to verify it compiles**

Run: `RDRS_FAST_HASH=1 cargo nextest run -p rdrs build_reading_pane 2>&1 | tail -15; cargo build 2>&1 | tail -15`
Expected: compiles. (Template not yet using the field — that's Task 5; the
field is simply unused-but-present, which is fine for a struct field.)

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/handlers/pages/mod.rs src/handlers/entries.rs
git commit -S -m "feat: carry summary_error into ReadingPaneView"
```

---

## Task 5: Reading-pane UI — Cancel button, failed branch, icon, CSS

**Files:**
- Modify: `templates/_icons.html`
- Modify: `static/css/app.css`
- Modify: `templates/_summarize_pending.html`
- Modify: `templates/_reading_pane.html`

- [ ] **Step 1: Add a `refresh` (retry) icon macro**

In `templates/_icons.html`, add a new macro line (after the `revert` macro):

```html
{% macro refresh() %}<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v5h-5"/></svg>{% endmacro %}
```

- [ ] **Step 2: Add the `.summary-error-banner` CSS**

In `static/css/app.css`, directly after the `.summary-box blockquote { ... }`
rule (ends ~line 1735), add:

```css
/* Failed-summary banner — mirrors .banner--error's tinted wash + error
   left-border, sized to sit inside the summary box. */
.summary-error-banner {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    font-size: var(--font-sm);
    line-height: 1.5;
    color: var(--color-text);
    background: light-dark(rgba(185, 28, 28, 0.06), rgba(248, 113, 113, 0.08));
    border: 1px solid light-dark(rgba(185, 28, 28, 0.20), rgba(248, 113, 113, 0.25));
    border-left: var(--border-accent-width) solid var(--color-error);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
}
.summary-error-banner .action-icon {
    flex: none;
    color: var(--color-error);
    display: inline-flex;
}
```

- [ ] **Step 3: Add the Cancel button to the in-flight fragment**

Replace the body of `templates/_summarize_pending.html` with (keep the leading
comment), adding the Cancel form inside the box:

```html
{# `POST /entries/{id}/summarize` response. Swaps ONLY the summary
   container so a Fetch-Full-Content view stays put — the reading
   pane's `<article>` is not touched. The full reading-pane render
   on next page load will surface the completed summary once the
   background worker finishes. #}
{%- import "_icons.html" as icons -%}
<template data-swap-target="#rp-summary-container">
    <div class="rp-summary-container" id="rp-summary-container" data-summary-container>
        <div class="summary-box">
            <div class="summary-actions">
                <form method="post" action="/entries/{{ id }}/summarize/cancel" data-swap="#rp-summary-container">
                    <button type="submit" class="rp-action" aria-label="Cancel summarization"><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span class="action-label">Cancel</span></button>
                </form>
            </div>
            <p class="muted">Summarizing… (refresh to see the result)</p>
        </div>
    </div>
</template>
```

> `SummarizePending` is a unit struct with no fields, so `{{ id }}` is not
> available. Add an `id` field to it. In `src/handlers/entries.rs` change
> `pub struct SummarizePending;` to `pub struct SummarizePending { pub id: i64 }`
> and update the two construction sites in `summarize_entry_form`
> (`Ok(SummarizePending)` → `Ok(SummarizePending { id: entry_id })`). Build to
> confirm.

- [ ] **Step 4: Add the in-flight Cancel button + failed branch to the reading pane**

In `templates/_reading_pane.html`, replace the `#rp-summary-container` block
(lines 80-102) with the three-branch version (in-flight gets a Cancel button;
new failed branch with Retry + Clear + error banner):

```html
        <div class="rp-summary-container" id="rp-summary-container" data-summary-container>
            {% if pane.summary_in_flight %}
            <div class="summary-box">
                <div class="summary-actions">
                    <form method="post" action="/entries/{{ pane.id }}/summarize/cancel" data-swap="#rp-summary-container">
                        <button type="submit" class="rp-action" aria-label="Cancel summarization"><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span class="action-label">Cancel</span></button>
                    </form>
                </div>
                <p class="muted">Summarizing… (refresh to see the result)</p>
            </div>
            {% else if let Some(summary) = pane.summary_text.as_ref() %}
            <div class="summary-box">
                <div class="summary-actions">
                    <button type="button" class="rp-action" data-summary-copy aria-label="Copy summary"><span class="action-icon" aria-hidden="true">{% call icons::copy() %}{% endcall %}</span><span class="action-label">Copy</span></button>
                    <button type="button" class="rp-action" data-summary-dismiss data-entry-id="{{ pane.id }}" aria-label="Dismiss summary"><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span class="action-label">Dismiss</span></button>
                </div>
                <div class="summary-header">
                    <div class="summary-title" data-summary-title>{{ pane.title }}</div>
                    {% if let Some(link) = pane.link.as_ref() %}
                    <a class="summary-link" href="{{ link }}" target="_blank" rel="noopener noreferrer" data-summary-link>{{ link }}</a>
                    {% endif %}
                </div>
                <blockquote class="rp-summary-content">{{ summary|safe }}</blockquote>
            </div>
            {% else if let Some(error) = pane.summary_error.as_ref() %}
            <div class="summary-box">
                <div class="summary-actions">
                    <form method="post" action="/entries/{{ pane.id }}/summarize" data-swap="#rp-summary-container">
                        <button type="submit" class="rp-action" aria-label="Retry summarization"><span class="action-icon" aria-hidden="true">{% call icons::refresh() %}{% endcall %}</span><span class="action-label">Retry</span></button>
                    </form>
                    <form method="post" action="/entries/{{ pane.id }}/summarize/cancel" data-swap="#rp-summary-container">
                        <button type="submit" class="rp-action" aria-label="Clear failed summary"><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span class="action-label">Clear</span></button>
                    </form>
                </div>
                <div class="summary-error-banner" data-summary-error><span class="action-icon" aria-hidden="true">{% call icons::close() %}{% endcall %}</span><span>Summarization failed: {{ error }}</span></div>
            </div>
            {% endif %}
        </div>
```

> The error banner reuses the `close` icon as a generic marker tinted by
> `.summary-error-banner .action-icon { color: var(--color-error) }`. The
> `data-summary-error` hook is for the e2e selector in Task 6.

- [ ] **Step 5: Rebuild assets (mandatory before any E2E / visual check)**

Run: `cargo build 2>&1 | tail -15`
Expected: compiles (Askama validates the templates at build time — a typo'd
field or macro fails here).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add templates/_icons.html static/css/app.css templates/_summarize_pending.html templates/_reading_pane.html src/handlers/entries.rs
git commit -S -m "feat: reading-pane Cancel button + failed-state banner with Retry/Clear"
```

---

## Task 6: E2E coverage for the failed state

**Files:**
- Modify: `e2e/support/seed.js` (failed-summary seeder)
- Modify: `e2e/steps/entries.steps.js` (or the steps file backing `reading.feature`)
- Modify: `e2e/features/reading.feature`

> Only the **failed** state is e2e-tested: `find_incomplete` re-queues only
> `pending`/`processing` rows on startup, so a seeded `failed` row is stable
> (the worker won't touch it and flip it). The in-flight Cancel button is
> covered by the Rust handler test (Task 3); seeding a `pending` row for e2e
> would race the recovery worker.

- [ ] **Step 1: Add a failed-summary seeder**

In `e2e/support/seed.js`, next to `insertSummary` (~line 117), add:

```javascript
  insertFailedSummary(entryId, userId, errorMessage = "Kagi API returned 503.") {
    this.db
      .prepare(
        `INSERT OR IGNORE INTO entry_summary (user_id, entry_id, status, error_message)
         VALUES (?, ?, 'failed', ?)`,
      )
      .run(userId, entryId, errorMessage);
  }
```

- [ ] **Step 2: Add the Given step**

In the steps file that defines `the entry titled "X" has a summary` (find it:
`rg -n "has a summary" e2e/steps`), add a sibling step. Mirror the existing
step's lookup of the entry id + user id, then call the new seeder:

```javascript
Given('the entry titled {string} has a failed summary', async function (title) {
  const entryId = this.seed.entryIdByTitle(title); // reuse however the sibling step resolves it
  this.seed.insertFailedSummary(entryId, this.userId);
});
```

> Match the exact helper names the sibling `has a summary` step uses to resolve
> `entryId` / `userId` in this codebase — copy that line verbatim, only swapping
> `insertSummary` for `insertFailedSummary`.

- [ ] **Step 3: Add the scenario**

In `e2e/features/reading.feature`, add after the existing summary scenarios
(~line 46):

```gherkin
  Scenario: Failed summary shows an error with Retry and Clear
    Given Kagi is configured
    And the entry titled "Test Entry 3" has a failed summary
    When I open the entry titled "Test Entry 3"
    Then I see the summary error banner
    And I see a "Retry" summary action
    And I see a "Clear" summary action
    When I click the "Clear" summary action
    Then I do not see the summary error banner
```

> Reuse existing steps where they already exist (`Kagi is configured` →
> `configureKagi`; "open the entry titled" likely exists for the reading pane).
> For the assertion steps, add thin steps keyed off selectors:
> `[data-summary-error]` for the banner, and button text for the actions
> (`page.getByRole('button', { name })`). The "click ... summary action" step
> submits the form and waits for the `#rp-summary-container` swap.

- [ ] **Step 4: Add the missing assertion/click steps**

In the same steps file, add (adjust the `World`/`page` accessor to match the
file's convention):

```javascript
Then('I see the summary error banner', async function () {
  await expect(this.page.locator('[data-summary-error]')).toBeVisible();
});

Then('I do not see the summary error banner', async function () {
  await expect(this.page.locator('[data-summary-error]')).toHaveCount(0);
});

Then('I see a {string} summary action', async function (label) {
  await expect(
    this.page.locator('#rp-summary-container').getByRole('button', { name: label }),
  ).toBeVisible();
});

When('I click the {string} summary action', async function (label) {
  await this.page
    .locator('#rp-summary-container')
    .getByRole('button', { name: label })
    .click();
  await this.page.waitForLoadState('networkidle');
});
```

- [ ] **Step 5: Rebuild + regenerate specs + run the scenario**

```bash
cargo build                       # embed the new templates/CSS
cd e2e && npx bddgen
npx playwright test --grep "Failed summary shows an error"
```
Expected: the scenario passes. If a step is reported undefined, align its text
with the existing step file's phrasing.

- [ ] **Step 6: Commit**

```bash
cd .. && git add e2e/support/seed.js e2e/features/reading.feature e2e/steps
git commit -S -m "test(e2e): failed-summary banner with Retry/Clear"
```

---

## Task 7: Docs + screenshots check

**Files:**
- Modify: `ARCHITECTURE.md` (and/or the `summary_worker.rs` module doc comment)
- Possibly: `screenshots/` (only if a default capture changed)

- [ ] **Step 1: Update prose that claims no cancellation/timeout**

Run: `rg -n -i "no (cancel|timeout)|cannot be (cancel|abort)|无|無取消" ARCHITECTURE.md src/services/summary_worker.rs`
Update any sentence stating the summary worker has no cancellation or no
timeout to reflect: per-entry cancellation via the registry + a 90 s Kagi
timeout (timeout → `failed`). Keep edits to the affected sentences only.

- [ ] **Step 2: Regenerate screenshots and check for diffs**

```bash
cargo build
cd e2e && npm run screenshots && cd ..
git status --porcelain screenshots/
```
Expected: likely **no** change — the default captures are the unread list +
reading pane (completed/empty) and the keyboard-help overlay, none of which show
the in-flight Cancel or failed states. If `git status` shows modified images,
include them in the commit; otherwise skip.

- [ ] **Step 3: Final full verification**

```bash
cargo fmt --all -- --check
cargo clippy -- -D warnings 2>&1 | tail -15
RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25
cd e2e && npx playwright test --grep-invert "@skip" 2>&1 | tail -25; cd ..
```
Expected: fmt clean, no clippy warnings, all Rust tests pass, e2e green.

- [ ] **Step 4: Commit any doc/screenshot changes**

```bash
git add ARCHITECTURE.md src/services/summary_worker.rs
# add screenshots/ only if Step 2 produced diffs
git commit -S -m "docs: note summary worker cancellation + timeout"
```

---

## Self-Review Notes

- **Spec coverage:** cancel pending+processing (Tasks 2-3 + `run_summary` Cancelled path), delete-on-cancel (Task 3), 90 s timeout→failed (Task 1-2), reading-pane-only controls (Task 5), failed banner = `.banner--error` palette (Task 5), Retry reuses `/summarize` (Task 5 form action), Cancel/Clear share `/summarize/cancel` (Tasks 3,5), `summary_error` field (Task 4), tests + e2e (Tasks 1,3,6), docs/screenshots (Task 7). All covered.
- **Type consistency:** `CancelRegistry = Arc<Mutex<HashMap<(i64,i64), CancellationToken>>>` used identically in worker, `AppState`, `main.rs`, and handler. `SummaryOutcome { Completed, Failed, Cancelled }` matches the worker `match`. `resolve_summary` returns a 3-tuple consumed by the single destructuring in `build_reading_pane_view`. `SummarizePending { id }` updated at both construction sites.
- **Race safety:** `run_summary` uses `biased` + `token.cancelled()` first, so a cancelled job never writes `completed`/`failed`; the handler's `delete` serializes after `set_processing` on the single write connection.
