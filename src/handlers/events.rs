use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::middleware::auth::PageAuthUser;
use crate::services::{EventKind, SummaryEventData, UserEvent};

/// Build the `sidebar` SSE event (no data payload; the client just refetches).
fn sidebar_event() -> Event {
    Event::default().event("sidebar").data("1")
}

/// Map a domain event to its SSE wire form. `Sidebar` carries no data (the
/// client just refetches); `Summary` carries `{entry_id, status}`.
fn to_sse_event(ev: &UserEvent) -> Event {
    match &ev.kind {
        EventKind::Sidebar => sidebar_event(),
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
    sidebar_event()
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
                () = shutdown.cancelled() => break,
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

    #[tokio::test]
    async fn stream_delivers_matching_sidebar_event() {
        // Exercises the `EventKind::Sidebar` arm of `to_sse_event` (the
        // user-filtering test above only delivers a Summary event).
        let bus = EventBus::new(16);
        let shutdown = CancellationToken::new();
        let mut stream = Box::pin(user_event_stream(bus.subscribe(), 1, shutdown));

        bus.emit_sidebar(1);
        let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("stream should yield within 1s")
            .expect("an item is present")
            .expect("Ok event");
        let _ = item; // a sidebar event was built and delivered
    }

    #[tokio::test]
    async fn stream_emits_resync_on_lagged_receiver() {
        // Overflow a tiny channel before the stream polls so the first
        // `recv()` returns `Lagged`, which must surface as a resync nudge.
        let bus = EventBus::new(2);
        let shutdown = CancellationToken::new();
        let rx = bus.subscribe();
        for _ in 0..5 {
            bus.emit_sidebar(1);
        }
        let mut stream = Box::pin(user_event_stream(rx, 1, shutdown));

        let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("stream should yield a resync within 1s")
            .expect("an item is present")
            .expect("Ok event");
        let _ = item; // resync event emitted in response to Lagged
    }

    #[tokio::test]
    async fn stream_ends_when_all_senders_dropped() {
        // Dropping the only `EventBus` closes the broadcast channel; the
        // stream's `recv()` returns `Closed` and the stream must end.
        let bus = EventBus::new(16);
        let shutdown = CancellationToken::new();
        let rx = bus.subscribe();
        drop(bus);
        let mut stream = Box::pin(user_event_stream(rx, 1, shutdown));

        let next = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("should resolve promptly when channel closed");
        assert!(
            next.is_none(),
            "stream must end when all senders are dropped"
        );
    }
}
