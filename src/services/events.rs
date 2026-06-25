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
