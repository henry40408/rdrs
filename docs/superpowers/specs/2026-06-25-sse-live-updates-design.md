# Live Updates via SSE (replace sidebar polling & summary refresh)

**Date:** 2026-06-25
**Status:** Approved (design)

## Problem

The UI relies on client polling and manual refresh for state that changes
server-side:

- **Sidebar unread counts** are refreshed by a 20 s `setInterval` in
  `static/js/app.js` (`installSidebarPolling()`), which fetches the
  `/sidebar/unread` HTML fragment and dispatches `rdrs:sidebar-unread`. The
  `<rdrs-sidebar>` element separately fetches `/api/sidebar` (JSON) on load and
  on `rdrs:swap-complete`. Two parallel mechanisms, and the steady-state cost is
  a request every 20 s per open tab regardless of whether anything changed.
- **AI summaries** never update live. The reading pane shows
  *"Summarizing… (refresh to see the result)"* after a form swap and only
  reflects completion when the user manually reloads. The summary worker runs in
  the background with no way to notify the page.
- **Entry-list summarization icons** (`templates/_entry_row.html`) are rendered
  once at page load from the DB status and never change afterward — a row that is
  `pending`/`processing` at load stays visually stale even after the worker
  finishes.

There is currently **no SSE, WebSocket, or broadcast** infrastructure
(`AppState` has `db`, caches, `summary_tx`, `summary_cancels` only). The server
already wires `axum::serve(...).with_graceful_shutdown(shutdown_signal())` and a
global `CancellationToken`, but a long-lived streaming connection would block
graceful shutdown unless it observes that token.

## Goals

1. Push live updates over a **single SSE connection** per tab, replacing the
   20 s sidebar polling entirely.
2. **Reading pane** auto-updates when a summary completes/fails — no manual
   refresh, replacing the "refresh to see the result" path.
3. **Entry-list summarization icons** update live as the worker progresses
   (pending → processing → completed/failed) and when a summary is cleared.
4. **Ctrl+C / SIGINT instantly tears down SSE connections** so the server shuts
   down gracefully without waiting on long-lived streams.

## Non-Goals

- No WebSocket; SSE (server→client only) is sufficient — all client→server
  actions stay as existing form POSTs / JSON endpoints.
- No polling fallback. EventSource's native reconnection is the only recovery
  path (per decision below); the `setInterval` polling is removed.
- No new summary states or schema migration.
- No change to the summarize/cancel/dismiss *action* endpoints themselves
  (they gain event emission, not new behaviour).

## Behavioural Decisions

| Item | Decision |
|---|---|
| Endpoint topology | Single `GET /events` carrying typed events |
| Sidebar payload | **Notify-and-fetch**: SSE sends a lightweight `sidebar` signal; client refetches `/api/sidebar` only when signalled |
| Summary icon update | Event carries `entry_id` + `status`; client rewrites the badge in JS — no extra request |
| Reading-pane update | If the signalled `entry_id` is the open entry, client fetches a summary fragment and swaps `#rp-summary-container` |
| Fallback | Pure SSE; rely on native EventSource reconnect. On (re)connect the client triggers one sidebar refresh to resync |
| Lagged subscriber | Treated as a `sidebar` resync signal — client refetches and converges |
| Auth | `PageAuthUser` (cookie session); events filtered by `user_id` |
| Shutdown | Stream `select!`s on the global `CancellationToken`; cancelled → stream ends |

## Architecture

### Backend — event bus

New module `src/services/events.rs`:

```rust
#[derive(Clone, Debug)]
pub struct UserEvent {
    pub user_id: i64,
    pub kind: EventKind,
}

#[derive(Clone, Debug)]
pub enum EventKind {
    /// Sidebar counts changed; client should refetch /api/sidebar.
    Sidebar,
    /// Summary status for an entry changed. `status == None` means cleared.
    Summary { entry_id: i64, status: Option<SummaryStatus> },
}

/// Thin wrapper over `broadcast::Sender<UserEvent>` with cheap clone.
#[derive(Clone)]
pub struct EventBus(broadcast::Sender<UserEvent>);

impl EventBus {
    pub fn new(capacity: usize) -> Self { /* broadcast::channel */ }
    pub fn subscribe(&self) -> broadcast::Receiver<UserEvent> { ... }
    pub fn emit_sidebar(&self, user_id: i64) { ... }
    pub fn emit_summary(&self, user_id: i64, entry_id: i64, status: Option<SummaryStatus>) { ... }
}
```

- `tokio::sync::broadcast` channel (capacity ~256). `send` errors when there are
  no receivers — ignored (no open tabs ⇒ nothing to notify).
- Added to `AppState` as `pub events: EventBus`. Created in `main.rs`.
- A clone of the existing global `cancel_token` is also added to `AppState` as
  `pub shutdown: CancellationToken`, giving the SSE handler the shutdown signal.

### Backend — SSE handler `GET /events`

New handler in `src/handlers/events.rs`, registered in `src/lib.rs`:

```rust
pub async fn events_stream(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let user_id = auth_user.user.id;
    let mut rx = state.events.subscribe();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                recv = rx.recv() => match recv {
                    Ok(ev) if ev.user_id == user_id => yield Ok(to_sse_event(&ev)),
                    Ok(_) => {}                                   // other user
                    Err(RecvError::Lagged(_)) => yield Ok(sidebar_resync_event()),
                    Err(RecvError::Closed) => break,
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
```

- Each SSE `Event` uses a named `event:` type (`sidebar` / `summary`) with a JSON
  `data:` payload. The summary payload is `{ "entry_id": i64, "status": "pending"|
  "processing"|"completed"|"failed"|null }`.
- `keep_alive` emits comment heartbeats so idle connections and intermediaries
  stay open.
- **Shutdown:** `shutdown.cancelled()` wins the `select!` on SIGINT/SIGTERM,
  ending every stream immediately so `with_graceful_shutdown` returns promptly.

### Backend — emission sites

| Where | Emits |
|---|---|
| `services/summary_worker.rs` — on `set_processing` / `set_completed` / `set_failed` | `Summary { entry_id, status }`; **plus** `Sidebar` on completion (summarized count changes) |
| `handlers/entries.rs::summarize_entry_form` | `Summary { entry_id, status: Some(Pending) }` |
| `handlers/entries.rs::summarize_cancel` (and the JSON dismiss in `handlers/entry.rs`) | `Summary { entry_id, status: None }` |
| mark read / unread handlers (`handlers/entries.rs`) | `Sidebar` |
| `services/background.rs` / `feed_sync.rs` — where new entries are inserted and `sidebar_cache` is busted | `Sidebar` for each affected `user_id` |

`start_summary_worker` gains an `EventBus` parameter (wired in `main.rs`
alongside the existing caches).

### Frontend

New ES module `static/js/sse.js`, imported from `app.js`:

```js
function installSse() {
  const es = new EventSource('/events', { withCredentials: true });
  es.addEventListener('open', () => refreshSidebar());          // resync on (re)connect
  es.addEventListener('sidebar', () => refreshSidebar());
  es.addEventListener('summary', (e) => onSummaryEvent(JSON.parse(e.data)));
  // EventSource auto-reconnects on error; no manual retry needed.
}
```

- `refreshSidebar()` → `document.querySelector('rdrs-sidebar')?.refresh()` (reuses
  the existing `/api/sidebar` fetch + `_updateBadges` path).
- `onSummaryEvent({entry_id, status})`:
  1. **List icon:** find the row for `entry_id` and rewrite its
     `.summary-badge*` span (class, `title`, inner SVG) to match `status` — JS
     mirror of the `_entry_row.html` match arms. `null` removes the badge.
  2. **Reading pane:** if `entry_id` is the currently open entry, fetch
     `GET /entries/{id}/summary/fragment` and swap it into
     `#rp-summary-container` via the existing `performSwap`/`swap()` helper.
- **Removed:** `installSidebarPolling()` and its `setInterval`. The
  `/sidebar/unread` fragment endpoint + `_sidebar_unread.html` +
  `rdrs:sidebar-unread` event are removed if no other consumer remains (verified
  during implementation).

### New fragment endpoint

`GET /entries/{id}/summary/fragment` — returns the reading-pane summary section
(`#rp-summary-container` inner markup) for the given entry, reusing
`resolve_summary()` and the `_reading_pane` summary partial. This is the SSR-first
equivalent of the form-swap response, used by SSE to refresh the open entry.

## Data Flow

```
worker/handler mutates state
        │  EventBus.emit_*(user_id, …)
        ▼
broadcast::Sender ──► every /events stream
        │ filter by user_id (+ shutdown select)
        ▼
SSE event (sidebar | summary)
        ▼
sse.js
  ├─ sidebar  → rdrs-sidebar.refresh() → GET /api/sidebar → _updateBadges
  └─ summary  → rewrite list badge (status in event)
               └─ if open entry → GET /entries/{id}/summary/fragment → swap #rp-summary-container
```

## Error Handling

- `broadcast::send` with no receivers → ignored.
- Lagged receiver → single `sidebar` resync signal; client refetches.
- SSE network drop → EventSource reconnects natively; `open` handler resyncs the
  sidebar.
- Fragment/`/api/sidebar` fetch failure in `sse.js` → swallowed silently (matches
  existing polling's `catch {}`); next event or reconnect recovers.

## Testing

**Rust**
- `EventBus` unit test: a subscriber only observes events for matching `user_id`;
  cross-user events are filtered.
- Integration: `POST /entries/{id}/summarize` causes a subscribed receiver to
  observe `Summary { Pending }`; worker completion observed as
  `Summary { Completed }` + `Sidebar`.
- Shutdown: cancelling the token ends the stream (assert the `select!`/loop
  terminates).

**E2E (Playwright BDD, `e2e/`)**
- Summarize an entry → reading pane updates to the completed summary **without a
  manual reload**, and the entry-row icon transitions to the completed state.
- Mark an entry read in one context → sidebar unread count updates live.

**Screenshots**
- No new visual components (icons already exist); no screenshot regeneration
  expected. Re-confirm after implementation.

## Dependencies

- `async-stream` (or equivalent) for the stream macro; `tokio-stream` if needed
  for broadcast adapters. Versions chosen per the 7-day cooldown rule at
  implementation time. `tokio::sync::broadcast` and `axum::response::sse` are
  already available (tokio + axum 0.8).

## Rollout / Compatibility

- Google Reader API (`handlers/greader/`) is unaffected — it does not use the
  sidebar/summary UI paths.
- SSE degrades safely: if a client never connects, the server simply has no
  receivers and behaves as today minus polling.
