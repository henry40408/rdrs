# SSE Live Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 20 s sidebar polling and the "refresh to see the result" summary flow with a single Server-Sent-Events stream that live-updates the sidebar unread counts, the reading-pane summary, and the entry-list summarization icons — and tears down instantly on Ctrl+C.

**Architecture:** A `tokio::sync::broadcast` event bus (`EventBus`) carries tiny per-user signals (`Sidebar` / `Summary{entry_id,status}`). A single authenticated `GET /events` SSE endpoint subscribes, filters by `user_id`, and `select!`s on the global `CancellationToken` so SIGINT ends every stream. Mutating paths (summary worker, summarize/cancel/dismiss, mark read/unread, entry open, background sync) emit events. The browser opens one `EventSource`: `sidebar` → refetch `/api/sidebar` (notify-and-fetch); `summary` → rewrite the row badge from the event's status and, if the entry is open, swap a new `/entries/{id}/summary/fragment` into `#rp-summary-container`.

**Tech Stack:** Rust, axum 0.8, `tokio::sync::broadcast`, `async-stream`, `tokio-stream`, Askama templates, vanilla ES-module JS (`EventSource`), Playwright BDD for E2E.

## Global Constraints

- Format gate: `cargo fmt --all -- --check` must pass; run `cargo fmt` before committing.
- Lint gate: `cargo clippy -- -D warnings` — warnings fail the build.
- Tests run with `cargo nextest run` (NOT `cargo test`). Use `RDRS_FAST_HASH=1` locally for the auth-heavy suite.
- Embedded assets: templates/CSS/JS compile into the binary via `include_str!`/`include_bytes!`. **Run `cargo build` before any E2E/screenshot run** or stale assets are tested.
- New dependencies: a crate version published <7 days ago MUST NOT be used; pick the newest version that is ≥7 days old (check `cargo info <crate>`). `async-stream` and `tokio-stream` are years-old and stable — verify the cooldown at pin time anyway.
- Git: work stays on branch `feat/sse-live-updates`. Commits are GPG-signed. Stage files explicitly by name — never `git add -A`/`.`. End commit messages with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- All GitHub-facing content in English.
- Bug fixes/features must update related tests and docs in the same change.

---

### Task 1: Event bus module + AppState wiring

**Files:**
- Create: `src/services/events.rs`
- Modify: `src/services/mod.rs` (add `pub mod events;` + re-exports)
- Modify: `src/lib.rs:46-55` (add two `AppState` fields)
- Modify: `src/main.rs:44-95` (construct `EventBus`, pass clone into `AppState`)
- Modify (compile-fix, two-line additions each): `tests/pages_test.rs`, `tests/summary_cancel.rs`, `tests/statistics_test.rs`, `tests/entry_handlers_test.rs`, `tests/auth_test.rs`, `tests/greader_test.rs` (2 sites), `tests/compression_test.rs`, `tests/handlers_test.rs` (3 sites), `tests/etag_test.rs`
- Test: inline `#[cfg(test)]` module in `src/services/events.rs`

**Interfaces:**
- Produces:
  - `pub struct UserEvent { pub user_id: i64, pub kind: EventKind }`
  - `pub enum EventKind { Sidebar, Summary { entry_id: i64, status: Option<SummaryStatus> } }`
  - `pub struct EventBus` with `pub fn new(capacity: usize) -> Self`, `pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<UserEvent>`, `pub fn emit_sidebar(&self, user_id: i64)`, `pub fn emit_summary(&self, user_id: i64, entry_id: i64, status: Option<SummaryStatus>)`. Derives `Clone`.
  - `AppState` gains `pub events: services::EventBus` and `pub shutdown: tokio_util::sync::CancellationToken`.

- [ ] **Step 1: Write the failing test** — create `src/services/events.rs` with only the test module so it fails to compile/run.

```rust
use crate::services::SummaryStatus;
use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct UserEvent {
    pub user_id: i64,
    pub kind: EventKind,
}

#[derive(Clone, Debug)]
pub enum EventKind {
    /// Sidebar counts changed; the client refetches `/api/sidebar`.
    Sidebar,
    /// Summary status for an entry changed. `status == None` means cleared.
    Summary {
        entry_id: i64,
        status: Option<SummaryStatus>,
    },
}

/// JSON payload shape sent in the SSE `data:` line for a summary event.
#[derive(Serialize)]
pub struct SummaryEventData {
    pub entry_id: i64,
    /// "pending" | "processing" | "completed" | "failed" | null
    pub status: Option<&'static str>,
}

/// Thin, cheaply-clonable wrapper over a `broadcast::Sender<UserEvent>`.
#[derive(Clone)]
pub struct EventBus(broadcast::Sender<UserEvent>);

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        EventBus(tx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UserEvent> {
        self.0.subscribe()
    }

    /// Emit best-effort. `send` errors only when there are no receivers
    /// (no open tabs) — nothing to notify, so the error is ignored.
    fn emit(&self, ev: UserEvent) {
        let _ = self.0.send(ev);
    }

    pub fn emit_sidebar(&self, user_id: i64) {
        self.emit(UserEvent {
            user_id,
            kind: EventKind::Sidebar,
        });
    }

    pub fn emit_summary(&self, user_id: i64, entry_id: i64, status: Option<SummaryStatus>) {
        self.emit(UserEvent {
            user_id,
            kind: EventKind::Summary { entry_id, status },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscriber_receives_emitted_event() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit_sidebar(42);
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.user_id, 42);
        assert!(matches!(ev.kind, EventKind::Sidebar));
    }

    #[tokio::test]
    async fn summary_event_carries_entry_and_status() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.emit_summary(7, 99, Some(SummaryStatus::Completed));
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.user_id, 7);
        match ev.kind {
            EventKind::Summary { entry_id, status } => {
                assert_eq!(entry_id, 99);
                assert_eq!(status, Some(SummaryStatus::Completed));
            }
            _ => panic!("expected Summary"),
        }
    }

    #[tokio::test]
    async fn emit_with_no_subscribers_does_not_panic() {
        let bus = EventBus::new(16);
        bus.emit_sidebar(1); // no receivers — must be a silent no-op
    }

    #[test]
    fn summary_event_data_serializes_to_expected_json() {
        let with = SummaryEventData {
            entry_id: 5,
            status: Some("completed"),
        };
        assert_eq!(
            serde_json::to_string(&with).unwrap(),
            r#"{"entry_id":5,"status":"completed"}"#
        );
        let cleared = SummaryEventData {
            entry_id: 5,
            status: None,
        };
        assert_eq!(
            serde_json::to_string(&cleared).unwrap(),
            r#"{"entry_id":5,"status":null}"#
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p rdrs events::tests 2>&1 | tail -20`
Expected: compile error — `SummaryStatus` needs `PartialEq` for `assert_eq!`, or module not declared yet. Resolve in Step 3.

- [ ] **Step 3: Wire the module and derive PartialEq on SummaryStatus**

In `src/services/mod.rs`, add after `pub mod entry_retention;` (keep alphabetical-ish grouping):
```rust
pub mod events;
```
and add to the re-export block:
```rust
pub use events::{EventBus, EventKind, SummaryEventData, UserEvent};
```

In `src/models/entry_summary.rs:9-16`, ensure the enum derives `PartialEq, Eq` (add if missing):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}
```
(`SummaryStatus` is re-exported from `services` via `summary_cache`; the `services::SummaryStatus` path in `events.rs` resolves to this type.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p rdrs events::tests 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Add the two AppState fields and wire main.rs**

In `src/lib.rs:46-55`:
```rust
#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub config: Arc<Config>,
    pub webauthn: Arc<Webauthn>,
    pub summary_cache: Arc<SummaryCache>,
    pub summary_tx: mpsc::Sender<SummaryJob>,
    pub sidebar_cache: Arc<SidebarCache>,
    pub summary_cancels: services::CancelRegistry,
    pub events: services::EventBus,
    pub shutdown: tokio_util::sync::CancellationToken,
}
```

In `src/main.rs`, after the `cancel_token` is created (line 45) add:
```rust
    // Event bus for SSE live updates (sidebar + summary). Capacity covers a
    // burst of mutations without lagging a slow subscriber; a lagged receiver
    // recovers via a sidebar resync signal.
    let events = services::EventBus::new(256);
```
Then in the `AppState { … }` literal (lines 87-95) add the two fields:
```rust
        summary_cancels,
        events: events.clone(),
        shutdown: cancel_token.clone(),
```

- [ ] **Step 6: Fix every test AppState literal**

In each of these files, the `let state = AppState { … };` literal must gain the two fields. Add to every literal (count per file in parentheses): `tests/pages_test.rs` (1), `tests/summary_cancel.rs` (1), `tests/statistics_test.rs` (1), `tests/entry_handlers_test.rs` (1), `tests/auth_test.rs` (1), `tests/greader_test.rs` (2), `tests/compression_test.rs` (1), `tests/handlers_test.rs` (3), `tests/etag_test.rs` (1):
```rust
        events: rdrs::services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
```
(Each test file already imports `rdrs::{… AppState …}`. `tokio_util` is a workspace dep available to integration tests; if a file lacks the import, use the fully-qualified `tokio_util::sync::CancellationToken::new()` as written.)

- [ ] **Step 7: Run the full suite to verify nothing else broke**

Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25`
Expected: PASS (existing tests + 3 new event tests). Then `cargo fmt && cargo clippy -- -D warnings`.

- [ ] **Step 8: Commit**

```bash
git add src/services/events.rs src/services/mod.rs src/lib.rs src/main.rs src/models/entry_summary.rs \
  tests/pages_test.rs tests/summary_cancel.rs tests/statistics_test.rs tests/entry_handlers_test.rs \
  tests/auth_test.rs tests/greader_test.rs tests/compression_test.rs tests/handlers_test.rs tests/etag_test.rs
git commit -m "feat(events): add per-user broadcast EventBus and wire into AppState"
```

---

### Task 2: SSE endpoint `GET /events` + router layer split + shutdown teardown

**Files:**
- Modify: `Cargo.toml` (add `async-stream`, `tokio-stream`)
- Create: `src/handlers/events.rs`
- Modify: `src/handlers/mod.rs` (add `pub mod events;`)
- Modify: `src/lib.rs:57-300` (`create_router`: register `/events` OUTSIDE the four wrapping layers)
- Modify: `src/main.rs:113-122` (cancel the token from inside the graceful-shutdown future)
- Test: inline `#[cfg(test)]` in `src/handlers/events.rs`; auth-gate test added to `tests/handlers_test.rs`

**Interfaces:**
- Consumes: `EventBus::subscribe`, `UserEvent`, `EventKind`, `SummaryEventData` (Task 1); `AppState.events`, `AppState.shutdown`; `PageAuthUser` extractor (`crate::middleware::auth::PageAuthUser`).
- Produces:
  - `pub fn user_event_stream(rx: broadcast::Receiver<UserEvent>, user_id: i64, shutdown: CancellationToken) -> impl Stream<Item = Result<Event, Infallible>>`
  - `pub async fn events_stream(auth_user: PageAuthUser, State(state): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>>`

- [ ] **Step 1: Add dependencies**

Check cooldown, then in `Cargo.toml` `[dependencies]`:
```toml
async-stream = "0.3"
tokio-stream = "0.1"
```
Run: `cargo info async-stream | grep -i 'published\|version' ; cargo info tokio-stream | grep -i 'published\|version'`
Expected: chosen versions are ≥7 days old (both crates are years old).

- [ ] **Step 2: Write the failing test** (stream factory) — create `src/handlers/events.rs` with the test module first.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{EventBus, SummaryStatus};
    use std::time::Duration;
    use tokio_stream::StreamExt;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn stream_filters_by_user_and_ends_on_shutdown() {
        let bus = EventBus::new(16);
        let shutdown = CancellationToken::new();
        let mut stream = Box::pin(user_event_stream(bus.subscribe(), 1, shutdown.clone()));

        // Another user's event is filtered out; user 1's event is delivered.
        // axum's `Event` exposes no public getters, so we assert on stream
        // *control* (exactly one item reaches us, proving the filter dropped
        // user 2) rather than inspecting the event payload — the wire payload
        // is covered by `summary_event_data_serializes_to_expected_json`.
        bus.emit_sidebar(2);
        bus.emit_summary(1, 50, Some(SummaryStatus::Completed));

        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("stream should yield within 1s")
            .expect("an item is present")
            .expect("Ok event");
        let _ = first; // delivered = user 1's event (user 2's was filtered)

        // Cancellation ends the stream promptly.
        shutdown.cancel();
        let next = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("should resolve promptly after shutdown");
        assert!(next.is_none(), "stream must terminate on shutdown");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p rdrs handlers::events 2>&1 | tail -20`
Expected: FAIL — `user_event_stream` not defined.

- [ ] **Step 4: Implement the handler + stream factory**

Prepend to `src/handlers/events.rs` (above the test module):
```rust
use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use crate::middleware::auth::PageAuthUser;
use crate::services::{EventKind, SummaryEventData, UserEvent};
use crate::AppState;

/// Map a domain event to its SSE wire form. `Sidebar` carries no data (the
/// client just refetches); `Summary` carries `{entry_id, status}`.
fn to_sse_event(ev: &UserEvent) -> Event {
    match &ev.kind {
        EventKind::Sidebar => Event::default().event("sidebar").data("1"),
        EventKind::Summary { entry_id, status } => {
            let payload = SummaryEventData {
                entry_id: *entry_id,
                status: status.map(|s| s.as_str()),
            };
            // serde_json::to_string never fails for this fixed shape.
            Event::default()
                .event("summary")
                .data(serde_json::to_string(&payload).unwrap_or_default())
        }
    }
}

/// A `sidebar` resync nudge, emitted when the broadcast receiver lags so the
/// client refetches and converges.
fn sidebar_resync_event() -> Event {
    Event::default().event("sidebar").data("1")
}

/// Build the per-connection SSE stream: deliver this user's events, drop other
/// users', resync on lag, and END when `shutdown` fires (so SIGINT tears the
/// connection down and graceful shutdown can complete).
pub fn user_event_stream(
    mut rx: Receiver<UserEvent>,
    user_id: i64,
    shutdown: CancellationToken,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                recv = rx.recv() => match recv {
                    Ok(ev) if ev.user_id == user_id => yield Ok(to_sse_event(&ev)),
                    Ok(_) => {}                                  // another user's event
                    Err(RecvError::Lagged(_)) => yield Ok(sidebar_resync_event()),
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
}

/// `GET /events` — one SSE stream per tab. Authenticated via the session
/// cookie (`PageAuthUser`); events are filtered to the authenticated user.
/// Registered OUTSIDE the ETag/Date/Compression/Timeout layers in
/// `create_router` (those buffer or time out a long-lived stream).
pub async fn events_stream(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = user_event_stream(
        state.events.subscribe(),
        auth_user.user.id,
        state.shutdown.clone(),
    );
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
```
Add to `src/handlers/mod.rs`:
```rust
pub mod events;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo nextest run -p rdrs handlers::events 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Register `/events` outside the wrapping layers**

In `src/lib.rs`, restructure `create_router` so the four response-buffering/timeout layers wrap everything EXCEPT `/events`. Replace the tail of the function (from `.with_state(state)` through the final `.layer(...)`, lines 292-299) so the existing big `Router::new().route(...)...` chain becomes a `core` router that gets the layers, and a thin outer router carries `/events`:

```rust
pub fn create_router(state: AppState) -> Router {
    // `core` holds every existing route. The ETag/Date/Compression/Timeout
    // layers below buffer the response body or abort after SERVER_REQUEST_TIMEOUT
    // — both fatal to a long-lived SSE stream — so they wrap `core` only.
    let core = Router::new()
        // ... ALL existing .route(...) / .merge(...) / .nest(...) / .fallback(...)
        //     calls stay here unchanged ...
        .layer(middleware::ETagLayer::new())
        .layer(middleware::DateHeaderLayer::new())
        .layer(CompressionLayer::new().gzip(true).br(true))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            SERVER_REQUEST_TIMEOUT,
        ));

    Router::new()
        // SSE lives outside the layers above. It still gets `state` via the
        // shared `.with_state` below.
        .route("/events", get(handlers::events::events_stream))
        .merge(core)
        .with_state(state)
}
```
Mechanical detail: today the chain is `Router::new().route(...)....fallback(...).with_state(state).layer(...)`. Move `.with_state(state)` OFF `core` (the outer router applies it once after `.merge`), and keep the four `.layer(...)` calls on `core`. `core` is therefore `Router<AppState>` (state not yet provided) — this compiles because `.layer` is state-agnostic and `.merge` unifies the state type before `.with_state`.

- [ ] **Step 7: Make Ctrl+C tear down SSE streams**

In `src/main.rs`, the graceful-shutdown future must cancel the token so open SSE streams (which `select!` on it) end and let `axum::serve` return. Replace lines 113-117:
```rust
    // Start server with graceful shutdown. Cancelling the token from inside
    // the shutdown future ends every in-flight SSE stream so the server does
    // not hang waiting on long-lived connections.
    let shutdown_token = cancel_token.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_token.cancel();
        })
        .await
        .expect("Server failed");
```
The existing `cancel_token.cancel();` at line 122 stays (idempotent — already cancelled) and continues to stop the background workers.

- [ ] **Step 8: Add an auth-gate integration test**

In `tests/handlers_test.rs`, add (it already has a `create_test_app`-style helper and request plumbing — mirror an existing unauthenticated-route test):
```rust
#[tokio::test]
async fn events_endpoint_requires_auth() {
    let app = create_test_app().await; // use this file's existing helper name
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/events")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // PageAuthUser redirects unauthenticated browsers to /login.
    assert!(
        resp.status().is_redirection() || resp.status() == axum::http::StatusCode::UNAUTHORIZED,
        "unauthenticated /events must not stream; got {}",
        resp.status()
    );
}
```
(If `PageAuthUser`'s unauth behavior differs, assert the actual status — confirm by reading `src/middleware/auth.rs`'s `PageAuthUser` rejection.)

- [ ] **Step 9: Verify, format, lint**

Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25` → PASS.
Run: `cargo fmt && cargo clippy -- -D warnings` → clean.

- [ ] **Step 10: Manually verify Ctrl+C teardown**

Run: `cargo run` in one terminal; in another: `curl -N -H "Cookie: <a valid session>" http://localhost:3000/events &` then send SIGINT to the server (Ctrl+C). Expected: server logs "Received Ctrl+C", the curl stream closes, and "Graceful shutdown complete" prints within ~1 s (NOT after the 30 s background-task timeout). Show the log output.

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml Cargo.lock src/handlers/events.rs src/handlers/mod.rs src/lib.rs src/main.rs tests/handlers_test.rs
git commit -m "feat(events): SSE /events endpoint with shutdown-aware teardown"
```

---

### Task 3: Emit events from summary + read/unread mutation paths

**Files:**
- Modify: `src/services/summary_worker.rs` (thread `EventBus` through; emit on status changes)
- Modify: `src/main.rs:61-68` (pass `events.clone()` into `start_summary_worker`)
- Modify: `src/handlers/entries.rs` (`summarize_entry_form`, `summarize_cancel_form`, `set_read_state`, `entry_fragment`)
- Modify: `src/handlers/entry.rs:284-317` (`delete_entry_summary`)
- Test: extend `src/services/summary_worker.rs` tests; add an emission integration test to `tests/summary_cancel.rs`

**Interfaces:**
- Consumes: `EventBus::{emit_sidebar, emit_summary}`, `SummaryStatus` (Task 1); `AppState.events`.
- Produces: `start_summary_worker` gains a trailing `events: EventBus` parameter (new signature below).

- [ ] **Step 1: Write the failing worker test** — prove the worker emits a `Summary{Completed}`+`Sidebar` pair. Add to `src/services/summary_worker.rs` tests, and update the local helpers.

Add a bus to the existing `registry()`-style helpers and a new test:
```rust
    #[tokio::test]
    async fn worker_emits_processing_then_terminal_event() {
        use crate::services::{EventBus, EventKind};
        let (tx, rx) = create_summary_channel(10);
        let cache = Arc::new(SummaryCache::new(100, 24));
        let db = setup_test_db();
        let cancel_token = CancellationToken::new();
        let bus = EventBus::new(32);
        let mut sub = bus.subscribe();

        // Seed a user + entry + pending summary so set_processing finds a row.
        let (user_id, entry_id) = db
            .user(|conn| {
                let u = user::create_user(conn, "emit", "hash", Role::User).unwrap().id;
                let cat = category::create_category(conn, u, "Tech").unwrap().id;
                let feed_id = feed::create_feed(conn, &feed::CreateFeedParams {
                    category_id: cat, url: "https://example.com/feed.xml", title: Some("F"),
                    description: None, site_url: None, custom_user_agent: None,
                    http2_disabled: None, custom_referrer: None,
                }).unwrap().id;
                let (e, _) = entry::upsert_entry(conn, feed_id, "g", Some("T"),
                    Some("https://example.com/a"), None, None, None, None).unwrap();
                entry_summary::upsert_pending(conn, u, e.id).unwrap();
                (u, e.id)
            })
            .await
            .unwrap();

        let handle = start_summary_worker(
            rx, cache, Arc::new(SidebarCache::default()), db, registry(),
            cancel_token.clone(), bus,
        );
        tx.send(SummaryJob { user_id, entry_id, entry_link: "https://example.com/a".into() })
            .await
            .unwrap();

        // First event must be Summary{Processing} for this entry. (Kagi is not
        // configured in tests, so the job then fails — we assert only the
        // processing emission, which is deterministic.)
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), sub.recv())
            .await
            .expect("an event should be emitted")
            .unwrap();
        assert_eq!(ev.user_id, user_id);
        assert!(matches!(
            ev.kind,
            EventKind::Summary { entry_id: e, status: Some(SummaryStatus::Processing) } if e == entry_id
        ));

        cancel_token.cancel();
        let _ = handle.await;
    }
```
Also update the three existing `start_summary_worker(...)` calls in this test module (`test_worker_stops_on_cancellation`, `test_worker_stops_when_channel_closed`, `test_worker_drains_jobs_on_cancellation`) to pass a final `EventBus::new(8)` argument.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p rdrs summary_worker 2>&1 | tail -20`
Expected: FAIL — arity mismatch (`start_summary_worker` takes 6 args) / `EventBus` unused.

- [ ] **Step 3: Thread EventBus through the worker and emit**

In `src/services/summary_worker.rs`:
- Add `use crate::services::EventBus;` (near the other `use super::…` imports).
- Change `start_summary_worker` signature to add `events: EventBus` and pass `&events` into `process_summary_job`:
```rust
pub fn start_summary_worker(
    mut rx: mpsc::Receiver<SummaryJob>,
    cache: Arc<SummaryCache>,
    sidebar_cache: Arc<SidebarCache>,
    db: DbPool,
    cancels: CancelRegistry,
    cancel_token: CancellationToken,
    events: EventBus,
) -> JoinHandle<()> {
```
Inside, both `process_summary_job(&job, …)` call sites gain `&events`:
```rust
                        process_summary_job(&job, &cache, &sidebar_cache, &db, &cancels, &events).await;
```
- `process_summary_job` and `run_summary_job_body` each gain `events: &EventBus` and forward it.
- Emit at each status transition in `run_summary_job_body`:
  - After `cache.set_processing(...)` (line 159): `events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Processing));`
  - In the `Completed` arm, in the success branch (after `sidebar_cache.bust(...)`, line 232):
    ```rust
    events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Completed));
    events.emit_sidebar(job.user_id); // "Summarized" count ticked up
    ```
  - In each `set_failed` branch (the early Kagi-config failures AND the `Failed` outcome arm — lines 172, 192, 247) emit after writing failed:
    ```rust
    events.emit_summary(job.user_id, job.entry_id, Some(SummaryStatus::Failed));
    ```
    (Skip emission only on the row-deleted/NotFound branches that call `cache.remove` — a cancelled job's UI is owned by the cancel handler.)
  - In the `Cancelled` arm: no emission (the cancel handler emits).

`SummaryStatus` is already imported in this file via `summary_cache`. If not in scope, add `use crate::services::SummaryStatus;`.

- [ ] **Step 4: Update main.rs worker construction**

In `src/main.rs:61-68`, add the trailing argument:
```rust
    let summary_worker_handle = services::start_summary_worker(
        summary_rx,
        summary_cache.clone(),
        sidebar_cache.clone(),
        db.clone(),
        summary_cancels.clone(),
        cancel_token.clone(),
        events.clone(),
    );
```

- [ ] **Step 5: Run worker test to verify it passes**

Run: `cargo nextest run -p rdrs summary_worker 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Emit from the handler mutation paths**

In `src/handlers/entries.rs`:
- `summarize_entry_form` — after the enqueue `send(...)` (line 655), before `Ok(SummarizePending { id: entry_id })`:
  ```rust
  state
      .events
      .emit_summary(user_id, entry_id, Some(SummaryStatus::Pending));
  ```
- `summarize_cancel_form` — after `state.sidebar_cache.bust(user_id);` (line 695):
  ```rust
  state.events.emit_summary(user_id, entry_id, None);
  state.events.emit_sidebar(user_id); // a completed summary may have been cleared
  ```
- `set_read_state` — inside the `if changed { … }` block (after `state.sidebar_cache.bust(user_id);`, line 568):
  ```rust
  state.events.emit_sidebar(user_id);
  ```
- `entry_fragment` — inside the `if was_unread { … }` block (after `state.sidebar_cache.bust(user_id);`, line 165):
  ```rust
  state.events.emit_sidebar(user_id);
  ```

In `src/handlers/entry.rs` `delete_entry_summary` — after `state.sidebar_cache.bust(user_id);` (line 314):
```rust
    state.events.emit_summary(user_id, id, None);
    state.events.emit_sidebar(user_id);
```

- [ ] **Step 7: Add a handler-level emission test**

In `tests/summary_cancel.rs` (it already builds `AppState` and exercises summarize/cancel), subscribe to the bus before issuing a summarize POST and assert a `Summary{Pending}` event arrives. Add near the existing summarize test:
```rust
#[tokio::test]
async fn summarize_emits_pending_event() {
    let app = setup().await; // this file's existing harness returning state+router
    let mut sub = app.state.events.subscribe();
    // ... create user/session/entry exactly as the existing tests do, capture entry_id + auth cookie ...
    // POST /entries/{entry_id}/summarize with the session cookie via app.router.oneshot(...)
    let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .expect("event emitted")
        .unwrap();
    assert!(matches!(
        ev.kind,
        rdrs::services::EventKind::Summary { status: Some(rdrs::services::SummaryStatus::Pending), .. }
    ));
}
```
(Reuse this file's existing user/session/entry setup helpers — do not invent new ones. `app.state.events` is the `EventBus` added in Task 1.)

- [ ] **Step 8: Verify, format, lint**

Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25` → PASS.
Run: `cargo fmt && cargo clippy -- -D warnings` → clean.

- [ ] **Step 9: Commit**

```bash
git add src/services/summary_worker.rs src/main.rs src/handlers/entries.rs src/handlers/entry.rs tests/summary_cancel.rs
git commit -m "feat(events): emit sidebar/summary events from mutation paths"
```

---

### Task 4: Background sync emits sidebar events for affected users

**Files:**
- Modify: `src/models/feed.rs` (add `owner_user_ids_for_feeds` query + test)
- Modify: `src/services/background.rs` (accept `EventBus` + `sidebar_cache` + emit after a bucket sync)
- Modify: `src/main.rs:54,87-102` (move `sidebar_cache` creation before the sync start; pass `sidebar_cache.clone()` + `events.clone()` into `start_background_sync`)
- Test: `src/models/feed.rs` unit test; `src/services/background.rs` keeps its cancellation tests (update signatures)

**Interfaces:**
- Consumes: `feed::list_by_bucket`, `SyncResult { new_entries, updated_entries }`, `EventBus::emit_sidebar`, `SidebarCache::bust`.
- Produces:
  - `feed::owner_user_ids_for_feeds(conn: &Connection, feed_ids: &[i64]) -> AppResult<Vec<i64>>` (distinct owning user ids; a feed → its category → `category.user_id`).
  - `start_background_sync(db, user_agent, cancel_token, sidebar_cache: Arc<SidebarCache>, events: EventBus)` (new signature).

- [ ] **Step 1: Write the failing model test** — distinct owners for a set of feeds.

In `src/models/feed.rs` tests:
```rust
    #[test]
    fn owner_user_ids_for_feeds_returns_distinct_owners() {
        let conn = setup(); // this module's existing in-memory db setup
        let u1 = user::create_user(&conn, "u1", "h", Role::User).unwrap().id;
        let u2 = user::create_user(&conn, "u2", "h", Role::User).unwrap().id;
        let c1 = create_test_category(&conn, u1, "A");
        let c2 = create_test_category(&conn, u2, "B");
        let f1 = create_feed(&conn, &CreateFeedParams { category_id: c1, url: "https://a/f", title: Some("a"), description: None, site_url: None, custom_user_agent: None, http2_disabled: None, custom_referrer: None }).unwrap().id;
        let f2 = create_feed(&conn, &CreateFeedParams { category_id: c2, url: "https://b/f", title: Some("b"), description: None, site_url: None, custom_user_agent: None, http2_disabled: None, custom_referrer: None }).unwrap().id;

        let mut owners = owner_user_ids_for_feeds(&conn, &[f1, f2]).unwrap();
        owners.sort_unstable();
        assert_eq!(owners, vec![u1, u2]);
        assert!(owner_user_ids_for_feeds(&conn, &[]).unwrap().is_empty());
    }
```
(Match the test module's actual `setup`/`create_test_category` helper names — they already exist in this file.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p rdrs feed::tests::owner_user_ids 2>&1 | tail -20`
Expected: FAIL — `owner_user_ids_for_feeds` not defined.

- [ ] **Step 3: Implement the query**

In `src/models/feed.rs` (near the other `pub fn` query functions):
```rust
/// Distinct owning user ids for the given feeds (a feed belongs to one
/// category, which belongs to one user). Empty input → empty output.
pub fn owner_user_ids_for_feeds(conn: &Connection, feed_ids: &[i64]) -> AppResult<Vec<i64>> {
    if feed_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?").take(feed_ids.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT DISTINCT c.user_id \
         FROM feeds f JOIN categories c ON c.id = f.category_id \
         WHERE f.id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(feed_ids.iter());
    let rows = stmt
        .query_map(params, |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
```
(Confirm the table/column names `feeds`, `categories`, `f.category_id`, `c.user_id` against `src/db/schema.rs`; adjust if the schema uses different identifiers.)

- [ ] **Step 4: Run model test to verify it passes**

Run: `cargo nextest run -p rdrs feed::tests::owner_user_ids 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Emit from background sync**

In `src/services/background.rs`:
- Imports:
  ```rust
  use std::sync::Arc;
  use super::sidebar_cache::SidebarCache;
  use crate::services::EventBus;
  use crate::models::feed;
  ```
- New signature:
  ```rust
  pub fn start_background_sync(
      db: DbPool,
      user_agent: String,
      cancel_token: CancellationToken,
      sidebar_cache: Arc<SidebarCache>,
      events: EventBus,
  ) -> JoinHandle<()> {
  ```
- After the `for (feed_id, result) in &results { … }` error-logging loop (line 48), add:
  ```rust
                    // Feeds that gained unread entries this cycle: bust each
                    // owner's sidebar cache and nudge their open tabs to refetch.
                    let changed_feed_ids: Vec<i64> = results
                        .iter()
                        .filter(|(_, r)| matches!(r, Ok(s) if s.new_entries > 0))
                        .map(|(id, _)| *id)
                        .collect();
                    if !changed_feed_ids.is_empty() {
                        match db
                            .background(move |conn| feed::owner_user_ids_for_feeds(conn, &changed_feed_ids))
                            .await
                        {
                            Ok(Ok(user_ids)) => {
                                for uid in user_ids {
                                    sidebar_cache.bust(uid);
                                    events.emit_sidebar(uid);
                                }
                            }
                            Ok(Err(e)) => error!("sidebar owner lookup failed: {e}"),
                            Err(e) => error!("sidebar owner lookup DB error: {e}"),
                        }
                    }
  ```

- [ ] **Step 6: Update main.rs**

`sidebar_cache` is created at `src/main.rs:54` (before the worker), so it is already in scope at the `start_background_sync` call (line 98-102). Update that call:
```rust
    let background_handle = services::start_background_sync(
        db.clone(),
        config.user_agent.clone(),
        cancel_token.clone(),
        sidebar_cache.clone(),
        events.clone(),
    );
```
Note: `sidebar_cache` is moved into `AppState` at line 91. Ensure the `start_background_sync` call (which clones it) stays — `.clone()` on the `Arc` before the move-into-state is fine because `AppState` takes `sidebar_cache` (the binding) at line 91 which is *before* line 98. **Reorder if needed:** move the `start_background_sync` call to before the `AppState { … }` literal, OR change the `AppState` literal to use `sidebar_cache.clone()`. Choose: change the `AppState` field to `sidebar_cache: sidebar_cache.clone()` is not possible (it's `sidebar_cache` shorthand). Simplest: keep the existing order (AppState built at 87-95, background sync at 98) and pass `state.sidebar_cache.clone()` is unavailable. **Resolution:** bind `let sidebar_cache = Arc::new(...)` then build everything with `sidebar_cache.clone()`; in the `AppState` literal write `sidebar_cache: sidebar_cache.clone(),` explicitly (not shorthand) so the binding survives for the line-98 call. Apply that explicit clone in the `AppState` literal.

- [ ] **Step 7: Update background.rs cancellation tests**

The two existing tests call `start_background_sync(db, "Test-Agent/1.0".to_string(), cancel_token.clone())`. Update both to pass the two new args:
```rust
        let handle = start_background_sync(
            db,
            "Test-Agent/1.0".to_string(),
            cancel_token.clone(),
            Arc::new(SidebarCache::default()),
            EventBus::new(8),
        );
```
Add `use crate::services::EventBus;` and `use super::sidebar_cache::SidebarCache;` to the test module if not already imported via `use super::*;`.

- [ ] **Step 8: Verify, format, lint**

Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25` → PASS.
Run: `cargo fmt && cargo clippy -- -D warnings` → clean.

- [ ] **Step 9: Commit**

```bash
git add src/models/feed.rs src/services/background.rs src/main.rs
git commit -m "feat(events): emit sidebar events from background sync for affected users"
```

---

### Task 5: Summary fragment endpoint + template extraction

**Files:**
- Create: `templates/_summary_container_inner.html` (the summary if/else block, extracted)
- Create: `templates/_summary_fragment.html` (swap-template wrapper around the container)
- Modify: `templates/_reading_pane.html:80-117` (replace inline block with `{% include %}`)
- Modify: `src/handlers/entries.rs` (add `SummaryFragment` template struct + `summary_fragment` handler; make `resolve_summary` `pub(crate)`)
- Modify: `src/lib.rs` (register `GET /entries/{id}/summary/fragment` inside `core`)
- Test: add a fragment test to `tests/entry_handlers_test.rs`

**Interfaces:**
- Consumes: `build_reading_pane_view` (existing `pub(crate)`), `ReadingPaneView`, `PageAuthUser`.
- Produces: `pub async fn summary_fragment(auth_user, State<AppState>, AxumPath<i64>) -> AppResult<SummaryFragment>` returning a `<template data-swap-target="#rp-summary-container">` body.

- [ ] **Step 1: Extract the summary block into a shared partial**

Create `templates/_summary_container_inner.html` with the exact inner content currently at `templates/_reading_pane.html:81-116` (everything *inside* `<div id="rp-summary-container">`):
```html
{%- import "_icons.html" as icons -%}
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
```

Then in `templates/_reading_pane.html`, replace lines 81-116 (the inner block) so the container becomes:
```html
        <div class="rp-summary-container" id="rp-summary-container" data-summary-container>
            {% include "_summary_container_inner.html" %}
        </div>
```
(Askama `{% include %}` shares the parent template's context, so `pane` resolves.)

- [ ] **Step 2: Create the fragment template**

Create `templates/_summary_fragment.html`:
```html
{# GET /entries/{id}/summary/fragment — re-renders #rp-summary-container so an
   SSE summary event can swap the reading pane without a full reload. #}
<template data-swap-target="#rp-summary-container">
    <div class="rp-summary-container" id="rp-summary-container" data-summary-container>
        {% include "_summary_container_inner.html" %}
    </div>
</template>
```

- [ ] **Step 3: Write the failing handler test**

In `tests/entry_handlers_test.rs` (it has user/session/entry setup + summary helpers — mirror them):
```rust
#[tokio::test]
async fn summary_fragment_renders_completed_summary() {
    // ... create user + session cookie + entry, then set a completed summary
    //     in the DB (entry_summary::upsert_pending + set_completed) ...
    let resp = get_with_cookie(&app, &format!("/entries/{entry_id}/summary/fragment"), &cookie).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(r#"data-swap-target="#rp-summary-container""#));
    assert!(body.contains("rp-summary-content")); // completed summary blockquote
}

#[tokio::test]
async fn summary_fragment_404_for_other_users_entry() {
    // ... entry owned by user A, request with user B's cookie ...
    let resp = get_with_cookie(&app, &format!("/entries/{entry_id}/summary/fragment"), &cookie_b).await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
}
```
(Use this file's existing request/cookie helpers; the names above are placeholders for whatever it already defines.)

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo nextest run summary_fragment 2>&1 | tail -20`
Expected: FAIL — route/handler missing (404 for both, or compile error referencing the helper).

- [ ] **Step 5: Implement the template struct + handler**

In `src/handlers/entries.rs`, add the template struct near `SummarizePending`:
```rust
/// `GET /entries/{id}/summary/fragment` — re-renders `#rp-summary-container`
/// for the entry's current summary state. Used by the SSE client to refresh
/// the open reading pane when a `summary` event arrives.
#[derive(Template)]
#[template(path = "_summary_fragment.html")]
pub struct SummaryFragment {
    pub pane: ReadingPaneView,
}

impl IntoResponse for SummaryFragment {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}
```
Add the handler (after `entry_fragment`):
```rust
/// `GET /entries/{id}/summary/fragment` — returns the summary container swap
/// fragment for the entry. Ownership enforced by `find_by_id_for_user` (404
/// otherwise). Does NOT mark the entry read (unlike `entry_fragment`).
pub async fn summary_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummaryFragment> {
    let user_id = auth_user.user.id;
    let ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    // has_save/has_kagi are irrelevant to the summary container; pass false.
    let pane = build_reading_pane_view(&state, user_id, &ewf, false, false).await?;
    Ok(SummaryFragment { pane })
}
```
(`build_reading_pane_view` already resolves the summary state via `resolve_summary`. No visibility change to `resolve_summary` is needed since we reuse `build_reading_pane_view`.)

- [ ] **Step 6: Register the route**

In `src/lib.rs`, inside `core`, register the literal-segment route BEFORE the bare `/entries/{id}` param routes so the trie resolves `summary/fragment` first. Add next to the other `/entries/{id}/summarize*` routes (after line 214):
```rust
        .route(
            "/entries/{id}/summary/fragment",
            get(handlers::entries::summary_fragment),
        )
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo build && cargo nextest run summary_fragment 2>&1 | tail -20`
Expected: PASS (both fragment tests).

- [ ] **Step 8: Verify, format, lint**

Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25` → PASS.
Run: `cargo fmt && cargo clippy -- -D warnings` → clean.

- [ ] **Step 9: Commit**

```bash
git add templates/_summary_container_inner.html templates/_summary_fragment.html templates/_reading_pane.html src/handlers/entries.rs src/lib.rs tests/entry_handlers_test.rs
git commit -m "feat(events): add summary container fragment endpoint for SSE swaps"
```

---

### Task 6: Frontend SSE wiring + remove polling

**Files:**
- Modify: `static/js/app.js` (add SSE installer + summary badge renderer; remove `installSidebarPolling` + its `setInterval` + its call)
- Modify: `src/lib.rs` (remove `/sidebar/unread` route)
- Modify: `src/handlers/entries.rs` (remove `sidebar_unread_fragment` + `SidebarUnreadFragment`; remove `build_sidebar_unread` if now unused)
- Delete: `templates/_sidebar_unread.html`
- (No new Rust test; behavior is covered by Task 7 E2E. JS is verified manually + E2E.)

**Interfaces:**
- Consumes: `EventSource`, existing module-private helpers `currentPaneEntryId()`, `performSwap()`, and `document.querySelector('rdrs-sidebar')?.refresh()`.
- Produces: `installSse()` (module-private), `renderSummaryBadge(row, status)` (module-private).

- [ ] **Step 1: Add the SSE installer + badge renderer to app.js**

Place this block in `static/js/app.js` right where `installSidebarPolling()` is defined (lines 647-679) — it replaces that function and its call. The summary-icon SVG markup mirrors the `_icons.html` `summary` macro (filled vs outline) and the badge classes mirror `templates/_entry_row.html:14-17`.
```js
// Live updates over a single SSE stream (replaces the old 20s sidebar poll).
// `sidebar` → refetch /api/sidebar (notify-and-fetch). `summary` → update the
// row badge from the event's status and, if the entry is open, swap the
// reading pane's summary container. EventSource reconnects natively; on
// (re)connect we resync the sidebar to catch anything missed while offline.
const SUMMARY_ICON_FILLED =
    '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></svg>';
const SUMMARY_ICON_OUTLINE =
    '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><g transform="translate(1.2 1.2) scale(0.9)"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></g></svg>';
// status -> [badge class, title, filled?]; null clears the badge.
const SUMMARY_BADGE = {
    completed:  ['summary-badge', 'Has Summary', true],
    pending:    ['summary-badge-pending', 'Pending', false],
    processing: ['summary-badge-processing', 'Processing', false],
    failed:     ['summary-badge-failed', 'Failed', true],
};
const BADGE_SELECTOR =
    '.summary-badge, .summary-badge-pending, .summary-badge-processing, .summary-badge-failed';

function renderSummaryBadge(row, status) {
    const existing = row.querySelector(BADGE_SELECTOR);
    if (!status || !SUMMARY_BADGE[status]) { existing?.remove(); return; }
    const [cls, title, filled] = SUMMARY_BADGE[status];
    const svg = filled ? SUMMARY_ICON_FILLED : SUMMARY_ICON_OUTLINE;
    if (existing) {
        existing.className = cls;
        existing.title = title;
        existing.innerHTML = svg;
        return;
    }
    // Insert before the <time> element so badge ordering matches the SSR row.
    const span = document.createElement('span');
    span.className = cls;
    span.title = title;
    span.setAttribute('aria-hidden', 'true');
    span.innerHTML = svg;
    const statusCluster = row.querySelector('.entry-status');
    const time = statusCluster?.querySelector('.entry-time');
    if (statusCluster && time) statusCluster.insertBefore(span, time);
    else statusCluster?.appendChild(span);
}

function refreshSidebar() {
    document.querySelector('rdrs-sidebar')?.refresh();
}

function onSummaryEvent(data) {
    const { entry_id, status } = data;
    const row = document.querySelector(`[data-entry-row][data-entry-id="${entry_id}"]`);
    if (row) renderSummaryBadge(row, status);
    // If the affected entry is the one open in the reading pane, swap its
    // summary container to reflect the new state (replaces "refresh to see").
    if (String(currentPaneEntryId()) === String(entry_id)) {
        performSwap(`/entries/${entry_id}/summary/fragment`, { method: 'GET' }, '#rp-summary-container');
    }
}

function installSse() {
    // Only on the logged-in surface (the sidebar element is the marker).
    if (!document.querySelector('rdrs-sidebar')) return;
    let es;
    try {
        es = new EventSource('/events', { withCredentials: true });
    } catch {
        return; // EventSource unavailable — no live updates, page still works.
    }
    es.addEventListener('open', () => refreshSidebar());
    es.addEventListener('sidebar', () => refreshSidebar());
    es.addEventListener('summary', (e) => {
        try { onSummaryEvent(JSON.parse(e.data)); } catch {}
    });
    // EventSource auto-reconnects on transient errors; nothing to do here.
}
installSse();
```
Delete the entire old `installSidebarPolling` function (lines 647-678) and its `installSidebarPolling();` call (line 679). Keep the `rdrs:swap-complete` → `refresh()` listener (lines 681-691) unchanged.

- [ ] **Step 2: Remove the now-dead `/sidebar/unread` polling endpoint**

In `src/lib.rs`, delete the route block (lines 82-87):
```rust
        // GET /sidebar/unread — SSR polling target ...
        .route(
            "/sidebar/unread",
            get(handlers::entries::sidebar_unread_fragment),
        )
```
In `src/handlers/entries.rs`, delete `SidebarUnreadFragment` (struct + `IntoResponse`, lines 579-594) and `sidebar_unread_fragment` (lines 596-606). Then check whether `build_sidebar_unread` (the non-delta one, lines 366-372) has any remaining callers:
```bash
rg -n "build_sidebar_unread\b" src
```
If the only references are its own definition (action handlers use `build_sidebar_unread_with_delta`), delete `build_sidebar_unread` too. If anything else uses it, leave it.
Delete `templates/_sidebar_unread.html`.

- [ ] **Step 3: Rebuild (assets are embedded) and run the suite**

Run: `cargo build 2>&1 | tail -15`
Expected: clean build (no references to the deleted template/handler).
Run: `RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -25` → PASS.
Run: `cargo fmt && cargo clippy -- -D warnings` → clean.

- [ ] **Step 4: Manual smoke test**

Run `cargo run`, log in, open the unread list with the reading pane open on an entry. In a second browser/tab, mark a different entry read → the first tab's sidebar count updates within ~1 s (no 20 s wait). Summarize an entry → its row badge cycles pending→completed and the open pane swaps to the summary, all without a manual refresh. Show what you observed.

- [ ] **Step 5: Commit**

```bash
git add static/js/app.js src/lib.rs src/handlers/entries.rs
git rm templates/_sidebar_unread.html
git commit -m "feat(events): drive sidebar + summary UI from SSE; remove polling"
```

---

### Task 7: E2E coverage, screenshots, and docs

**Files:**
- Create: `e2e/features/sse-live-updates.feature` (+ step defs if new steps are needed under `e2e/steps/`)
- Modify: `ARCHITECTURE.md` and/or `CLAUDE.md` (document the SSE cross-cut)
- Verify (no change expected): `screenshots/` PNGs

**Interfaces:**
- Consumes: the running app's `/events` stream and existing E2E fixtures/login helpers.

- [ ] **Step 1: Write the BDD feature**

Create `e2e/features/sse-live-updates.feature`, following the existing feature/step conventions in `e2e/`:
```gherkin
Feature: Live updates via SSE

  Background:
    Given a logged-in user with a feed that has unread entries

  Scenario: Reading pane and row badge update when a summary completes
    Given Kagi summarization is configured to return a canned summary
    When I open an entry in the reading pane
    And I click Summarize
    Then the entry row shows a pending summary badge
    And without reloading, the reading pane shows the completed summary
    And the entry row shows the completed summary badge

  Scenario: Sidebar unread count updates live without polling
    When another session marks one of my unread entries as read
    Then within 5 seconds my sidebar unread count decreases by one without a page reload
```
Implement only the steps that don't already exist. For the "another session marks read" step, drive a second `request` context (or a direct authenticated `POST /entries/{id}/read`) as other E2E specs do. For the canned-Kagi step, reuse the test harness's existing Kagi stub if present; otherwise assert on the `pending`→`completed` badge transition driven by the real worker against a mock endpoint already used elsewhere. If no Kagi stub exists, tag the summary scenario `@skip` and leave a comment — the sidebar scenario is the must-have.

- [ ] **Step 2: Build, then run the new feature**

Run (from `e2e/`): `cd e2e && npm ci` (first time only), then from repo root `cargo build`, then `cd e2e && npx playwright test --grep "Live updates via SSE" 2>&1 | tail -30`
Expected: PASS (or the sidebar scenario passes and the summary scenario is `@skip` if no Kagi stub).

- [ ] **Step 3: Confirm screenshots are unaffected**

Run: `cargo build && cd e2e && npm run screenshots`
Then: `git status --porcelain screenshots/`
Expected: NO changes (the icons/markup are unchanged; SSE is behavioral). If any screenshot changed, investigate — a diff means an unintended visual regression; fix it before proceeding. Revert any noise with `git checkout -- screenshots/` only after confirming the change is spurious.

- [ ] **Step 4: Document the SSE cross-cut**

In `ARCHITECTURE.md` (and the cross-cutting list in `CLAUDE.md` if appropriate), add a short paragraph: a single authenticated `GET /events` SSE endpoint (`handlers/events.rs`) streams per-user `sidebar`/`summary` signals from an in-memory `EventBus` (`services/events.rs`); mutation paths emit, the browser's one `EventSource` reacts (notify-and-fetch for the sidebar, badge rewrite + container swap for summaries); the stream `select!`s on the global `CancellationToken` so SIGINT tears it down. Note `/events` is registered outside the ETag/Compression/Timeout layers.

- [ ] **Step 5: Commit**

```bash
git add e2e/features/sse-live-updates.feature ARCHITECTURE.md CLAUDE.md
# include any new/edited step-definition files explicitly by name
git commit -m "test(events): E2E for SSE live updates; document the cross-cut"
```

---

## Verification (whole-feature)

- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `RDRS_FAST_HASH=1 cargo nextest run` all green
- [ ] `cd e2e && npx playwright test --grep-invert "@skip"` green (after `cargo build`)
- [ ] Manual: Ctrl+C with an open `/events` curl shuts the server down in ~1 s, not after the 30 s background-task timeout
- [ ] Manual: sidebar count + summary pane + row badge all update live, no manual refresh, no 20 s polling delay
- [ ] `git grep -n "installSidebarPolling\|/sidebar/unread\|_sidebar_unread" src static templates` returns nothing (polling fully removed)
