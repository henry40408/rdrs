//! Per-request timing, the HTTP-side counterpart to sqlx's `sqlx::query`
//! statement log.
//!
//! Until this existed, nothing in the process recorded how long a request
//! took: `sqlx::query` (DEBUG, with `elapsed`) covered the database and
//! stopped there, so a page that felt slow could not be attributed to query
//! time, template rendering, an upstream fetch, or a lock held somewhere in
//! between. This middleware closes that gap with one event per request.
//!
//! **Levels mirror the sqlx convention deliberately.** Every request is a
//! DEBUG `http.request`, and one at or beyond [`SLOW_REQUEST_THRESHOLD`] is a
//! WARN `http.slow_request` instead. That split is what makes the default
//! filter (`error,rdrs=info`, see `main::init_tracing`) useful without being
//! noisy: a healthy deployment logs nothing per request, while a request that
//! blew past a second surfaces on its own. Turn the full stream on with
//! `RUST_LOG=rdrs=debug` or, narrowed to this module,
//! `RUST_LOG=rdrs=info,rdrs::middleware::request_log=debug`.
//!
//! **The `route` field is the matched route template, never the request
//! path.** `/invite/{token}` carries a single-use invite credential *in the
//! path*, and `handlers::proxy` takes a signed URL the same way; logging
//! `uri().path()` would write both into a file that outlives the credential
//! and is routinely shipped to a log aggregator. [`MatchedPath`] gives the
//! template the router matched (`/invite/{token}`), which is also the label
//! worth aggregating on — a per-token path would be a distinct series in
//! every metrics backend. A request that matched no route has no template,
//! and its path is attacker-controlled text (log injection, plus whatever a
//! scanner put in the URL), so it is labelled [`UNMATCHED_ROUTE`] rather than
//! logged verbatim.
//!
//! **What the duration covers.** Timing runs from entry into this layer to
//! the moment the inner stack yields the response *head*, which is where
//! every other layer's work (auth, CSRF, `ETag` hashing, compression) and the
//! whole handler live. It does not include streaming the body to the client,
//! so it is a server-side service time, not a client-observed one — the same
//! thing `tower_http`'s `TraceLayer` reports as `latency`. The visible
//! consequence is `/events`: an SSE handler returns its head immediately and
//! then streams for minutes, so its entry reads as a fast request rather than
//! a multi-minute one. That is the intended reading; a connection-lifetime
//! number would trip the slow-request threshold on every SSE client.
//!
//! Layered **outermost** in `create_router`, for the same reason the security
//! headers are: several inner layers (`forward_auth`'s redirects, both CSRF
//! guards' 403s) short-circuit without calling `next`, and `/events` sits
//! outside the `core` stack entirely. Only the outermost position sees all of
//! them. It still runs after routing — axum populates [`MatchedPath`] when it
//! matches, before handing the request to the layered service — which is what
//! makes the template available here at all.

use std::time::{Duration, Instant};

use axum::{
    extract::{MatchedPath, Request},
    http::{Extensions, Method},
    middleware::Next,
    response::Response,
};

/// A request taking at least this long is logged at WARN as
/// `http.slow_request` instead of DEBUG.
///
/// One second, matching sqlx's own `slow_statements_duration` default so the
/// HTTP and SQL slow logs agree on what "slow" means. It sits well under
/// `services::http::SERVER_REQUEST_TIMEOUT`, so a request that the timeout
/// layer eventually kills is warned about first.
pub const SLOW_REQUEST_THRESHOLD: Duration = Duration::from_secs(1);

/// The `route` value for a request that matched no route (a 404 from the
/// fallback). Deliberately not the requested path — see the module docs.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Time the request and emit one structured event once the response head is
/// ready. Transparent: the response is returned untouched.
pub async fn log_request_duration(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    // Both are taken before `next` consumes the request, and both are cheap:
    // `Method` is an inline enum for the standard verbs, and the route
    // template is a short string in every routed case.
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

/// Emit the event for a finished request.
///
/// Split out of [`log_request_duration`] so both branches can be exercised
/// against an exact duration: reaching the WARN branch through the middleware
/// itself would need a test that genuinely blocks for
/// [`SLOW_REQUEST_THRESHOLD`].
///
/// `elapsed` is attached twice on purpose, following the pattern
/// `sqlx::query` uses: `elapsed` is the human-readable `Duration` debug
/// (`1.96ms`) that reads well in the console formats, `elapsed_ms` the plain
/// number to filter and aggregate on under `RDRS_LOG_FORMAT=json`.
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

/// `elapsed` in milliseconds, as a number rather than a formatted string.
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

    /// Everything the subscriber wrote during one test, as a string.
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

    /// Install a DEBUG-level subscriber for the current thread and return the
    /// buffer it writes to. The guard must be held for as long as the events
    /// are being produced — `#[tokio::test]` runs on a current-thread runtime,
    /// so the awaited work stays on the thread the default is set for.
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

    /// A router shaped like `create_router`'s: a `core` of ordinary routes
    /// merged under an outside-the-stack `/events`, with this middleware
    /// applied outermost over both.
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
        // The reason this middleware logs `MatchedPath` rather than
        // `uri().path()`: an invite token is a single-use credential that
        // travels in the path, and a log line outlives it. Logging the raw
        // path would hand every reader of the log a working invite.
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
        // `/events` is merged in outside `core` precisely to escape the
        // ETag/compression/timeout layers, so a middleware nested inside
        // those would never see it. This one is layered outermost and does.
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
        // At the boundary, not past it: `>=` is what makes a request that
        // takes exactly the threshold count as slow.
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
        // The literal below is one millisecond under the threshold; assert the
        // relationship rather than trusting the two to stay in step.
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
