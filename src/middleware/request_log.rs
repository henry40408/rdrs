//! Per-request timing, the HTTP counterpart to sqlx's statement log.
//!
//! Every request logs DEBUG `http.request`; one at or past
//! [`SLOW_REQUEST_THRESHOLD`] logs WARN `http.slow_request` instead.
//!
//! **`route` is the matched route template, never the request path**: paths
//! can carry credentials (`/invite/{token}`, signed proxy URLs) that must not
//! reach logs. Unmatched requests are labelled [`UNMATCHED_ROUTE`].
//!
//! Duration ends when the response head is ready, excluding body streaming, so
//! SSE `/events` reads as fast — intended.
//!
//! Layered **outermost**, since inner layers short-circuit and `/events` sits
//! outside the `core` stack.

use std::time::{Duration, Instant};

use axum::{
    extract::{MatchedPath, Request},
    http::{Extensions, Method},
    middleware::Next,
    response::Response,
};

/// Requests at least this long log at WARN as `http.slow_request`. Matches
/// sqlx's `slow_statements_duration` default.
pub const SLOW_REQUEST_THRESHOLD: Duration = Duration::from_secs(1);

/// `route` for unmatched requests; never the (attacker-controlled) path.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Time the request and emit one event once the response head is ready.
pub async fn log_request_duration(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = route_label(req.extensions()).to_owned();

    let started = Instant::now();
    let response = next.run(req).await;
    let elapsed = started.elapsed();

    log_completed(&method, &route, response.status().as_u16(), elapsed);
    response
}

/// The matched route template for `extensions`, or [`UNMATCHED_ROUTE`].
fn route_label(extensions: &Extensions) -> &str {
    extensions
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE, MatchedPath::as_str)
}

/// Emit the event; split out so tests can pass an exact duration. `elapsed` is
/// logged both as `Duration` (console) and `elapsed_ms` (JSON aggregation).
fn log_completed(method: &Method, route: &str, status: u16, elapsed: Duration) {
    let elapsed_ms = as_millis_f64(elapsed);

    if elapsed >= SLOW_REQUEST_THRESHOLD {
        tracing::warn!(
            event = "http.slow_request",
            method = %method,
            route,
            status,
            ?elapsed,
            elapsed_ms,
            threshold_ms = as_millis_f64(SLOW_REQUEST_THRESHOLD),
            "request exceeded the slow-request threshold"
        );
    } else {
        tracing::debug!(
            event = "http.request",
            method = %method,
            route,
            status,
            ?elapsed,
            elapsed_ms,
            "request completed"
        );
    }
}

fn as_millis_f64(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::StatusCode, routing::get};
    use std::io;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn contents(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .expect("no test panics while holding the lock")
                    .clone(),
            )
            .expect("the fmt subscriber writes UTF-8")
        }
    }

    impl io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("no test panics while holding the lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Thread-local DEBUG subscriber; hold the guard while events are produced.
    fn capture_logs() -> (CapturedLogs, tracing::subscriber::DefaultGuard) {
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (logs, guard)
    }

    /// Mirrors `create_router`: `core` plus an outside-the-stack `/events`.
    fn app() -> Router {
        let core = Router::new()
            .route("/invite/{token}", get(async || "invite page"))
            .route("/", get(async || "unread page"));

        Router::new()
            .route("/events", get(async || "stream"))
            .merge(core)
            .layer(axum::middleware::from_fn(log_request_duration))
    }

    async fn get_path(path: &str) -> StatusCode {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request is well-formed");
        app()
            .oneshot(request)
            .await
            .expect("the router is infallible")
            .status()
    }

    #[tokio::test]
    async fn logs_the_route_template_not_the_invite_token() {
        // The invite token is a credential in the path; it must not be logged.
        let (logs, _guard) = capture_logs();

        let status = get_path("/invite/s3cret-invite-token").await;

        assert_eq!(status, StatusCode::OK);
        let logs = logs.contents();
        assert!(
            logs.contains(r#"route="/invite/{token}""#),
            "expected the matched template as the route label, got:\n{logs}"
        );
        assert!(
            !logs.contains("s3cret-invite-token"),
            "the invite token must never reach the log:\n{logs}"
        );
    }

    #[tokio::test]
    async fn logs_one_debug_event_per_request_with_the_duration() {
        let (logs, _guard) = capture_logs();

        let status = get_path("/").await;

        assert_eq!(status, StatusCode::OK);
        let logs = logs.contents();
        assert_eq!(
            logs.matches(r#"event="http.request""#).count(),
            1,
            "expected exactly one event per request, got:\n{logs}"
        );
        assert!(logs.contains("DEBUG"), "{logs}");
        assert!(logs.contains("method=GET"), "{logs}");
        assert!(logs.contains(r#"route="/""#), "{logs}");
        assert!(logs.contains("status=200"), "{logs}");
        assert!(
            logs.contains("elapsed_ms="),
            "the numeric duration is what JSON output aggregates on:\n{logs}"
        );
    }

    #[tokio::test]
    async fn covers_the_sse_route_that_sits_outside_the_core_stack() {
        // `/events` sits outside `core`; only an outermost layer sees it.
        let (logs, _guard) = capture_logs();

        let status = get_path("/events").await;

        assert_eq!(status, StatusCode::OK);
        assert!(
            logs.contents().contains(r#"route="/events""#),
            "{}",
            logs.contents()
        );
    }

    #[tokio::test]
    async fn an_unmatched_request_is_labelled_without_its_path() {
        // A 404 path is attacker-controlled text. It is labelled, not logged.
        let (logs, _guard) = capture_logs();

        let status = get_path("/no-such-route-4f3b").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        let logs = logs.contents();
        assert!(logs.contains(r#"route="<unmatched>""#), "{logs}");
        assert!(logs.contains("status=404"), "{logs}");
        assert!(
            !logs.contains("no-such-route-4f3b"),
            "the raw path must not be logged:\n{logs}"
        );
    }

    #[test]
    fn a_request_at_the_threshold_warns_instead() {
        // Exactly the threshold counts as slow.
        let (logs, _guard) = capture_logs();

        log_completed(&Method::GET, "/", 200, SLOW_REQUEST_THRESHOLD);

        let logs = logs.contents();
        assert!(logs.contains(r#"event="http.slow_request""#), "{logs}");
        assert!(logs.contains("WARN"), "{logs}");
        assert!(logs.contains("elapsed_ms=1000"), "{logs}");
        assert!(
            logs.contains("threshold_ms=1000"),
            "the threshold travels with the event so an alert can state it:\n{logs}"
        );
    }

    #[test]
    fn a_request_just_under_the_threshold_stays_at_debug() {
        // Keep the literal below in step with the threshold.
        assert_eq!(SLOW_REQUEST_THRESHOLD, Duration::from_secs(1));
        let (logs, _guard) = capture_logs();

        log_completed(&Method::GET, "/", 200, Duration::from_millis(999));

        let logs = logs.contents();
        assert!(logs.contains(r#"event="http.request""#), "{logs}");
        assert!(!logs.contains("slow_request"), "{logs}");
    }

    #[test]
    fn route_label_falls_back_when_no_route_matched() {
        assert_eq!(route_label(&Extensions::new()), UNMATCHED_ROUTE);
    }
}
