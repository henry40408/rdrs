//! Request interception — Playwright's `page.route`, on the CDP `Fetch`
//! domain.
//!
//! Two things in this suite need to change what the network does rather than
//! merely observe it:
//!
//! * The **no-JS walkthrough** aborts every `*.js` request, on top of switching
//!   scripting off. Disabling scripting alone leaves the requests in flight,
//!   and the walkthrough exists to prove the pages work when the scripts are
//!   never *delivered* — a stricter thing, and a CI gate since #490.
//! * The **stale-response scenarios** hold one fragment response back for
//!   600 ms so a second click can overtake it. What they assert is that the
//!   slow, stale response never overwrites the entry the reader picked
//!   afterwards.
//!
//! `WebDriver` has no equivalent, so this drives CDP directly. It needs
//! thirtyfour's `cdp-events` feature: commands go out over the ordinary
//! WebDriver connection, but `Fetch.requestPaused` has to be *received*, which
//! only the WebSocket transport can do.
//!
//! Every paused request must be answered — continued, failed or fulfilled — or
//! the page hangs waiting on it. The dispatcher below answers each one exactly
//! once, and requests matching no rule are continued unmodified.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use regex::Regex;
use thirtyfour::cdp::domains::fetch::{
    ContinueRequest, Enable, FailRequest, RequestPattern, RequestPaused,
};
use thirtyfour::cdp::domains::network::{ErrorReason, ResourceType, ResponseReceived};
use thirtyfour::prelude::*;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

/// What to do with a request whose URL matches a rule.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// Refuse it, as an ad blocker or an offline network would.
    Abort,
    /// Hold it for a while, then let it through.
    ///
    /// The delay is served without blocking other requests: the dispatcher
    /// hands each paused request to its own task.
    Delay(Duration),
}

/// One URL pattern and what to do with the requests it matches.
#[derive(Debug)]
struct Rule {
    pattern: Regex,
    action: Action,
    /// How many requests this rule has answered — what
    /// [`Interceptor::hits`] reports.
    hits: Arc<AtomicUsize>,
    /// Notified after each request this rule handled has been answered, so a
    /// step can wait for a held response to settle.
    settled: Arc<Notify>,
}

/// A live CDP attachment to one browser: request rules, plus a log of the
/// status code every top-level document answered with.
///
/// Dropping it stops both listeners; the browser is closed at the end of the
/// scenario anyway, so `Fetch` is never explicitly disabled.
#[derive(Debug)]
pub struct Network {
    rules: Arc<Mutex<Vec<Rule>>>,
    documents: Arc<Mutex<Vec<Document>>>,
    dispatcher: JoinHandle<()>,
    responses: JoinHandle<()>,
}

/// One navigation's outcome — what Playwright's `page.goto()` returned.
#[derive(Debug, Clone)]
pub struct Document {
    pub url: String,
    pub status: u32,
}

impl Network {
    /// Attaches to a browser and starts answering paused requests.
    ///
    /// # Errors
    ///
    /// Fails when the CDP WebSocket cannot be opened, or a domain refused to
    /// enable.
    pub async fn attach(driver: &WebDriver) -> Result<Self> {
        let session = Arc::new(
            driver
                .cdp()
                .connect()
                .await
                .context("opening a CDP WebSocket for request interception")?,
        );

        // Subscribed *before* enabling, so nothing paused between the two is
        // left unanswered — a request paused with no listener stalls the page
        // until it times out.
        let mut paused = session
            .subscribe::<RequestPaused>()
            .await
            .context("subscribing to Fetch.requestPaused")?;
        // `WebDriver` cannot report a navigation's status code at all, so the
        // walkthrough's "did this link answer 200?" check reads it from here.
        let mut received = session
            .subscribe::<ResponseReceived>()
            .await
            .context("subscribing to Network.responseReceived")?;
        session
            .send(Enable {
                // Every request, at the request stage: the rules decide what is
                // interesting, and a narrower pattern would have to be widened
                // every time a new rule is added.
                patterns: Some(vec![RequestPattern::default()]),
                handle_auth_requests: None,
            })
            .await
            .context("enabling the Fetch domain")?;

        let documents: Arc<Mutex<Vec<Document>>> = Arc::new(Mutex::new(Vec::new()));
        let responses = tokio::spawn({
            let documents = Arc::clone(&documents);
            async move {
                while let Some(event) = received.next().await {
                    if !matches!(event.r#type, ResourceType::Document) {
                        continue;
                    }
                    let url = event.response.get("url").and_then(serde_json::Value::as_str);
                    let status = event
                        .response
                        .get("status")
                        .and_then(serde_json::Value::as_u64);
                    if let (Some(url), Some(status)) = (url, status) {
                        documents.lock().await.push(Document {
                            url: url.to_owned(),
                            status: status as u32,
                        });
                    }
                }
            }
        });

        let rules: Arc<Mutex<Vec<Rule>>> = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = tokio::spawn({
            let rules = Arc::clone(&rules);
            async move {
                while let Some(event) = paused.next().await {
                    let Some(url) = event.request.get("url").and_then(serde_json::Value::as_str)
                    else {
                        // No URL to match on; let it through rather than
                        // leaving the page waiting.
                        let _ = session.send(continue_request(&event)).await;
                        continue;
                    };
                    let matched = {
                        let rules = rules.lock().await;
                        rules
                            .iter()
                            .find(|rule| rule.pattern.is_match(url))
                            .map(|rule| {
                                (
                                    rule.action,
                                    Arc::clone(&rule.hits),
                                    Arc::clone(&rule.settled),
                                )
                            })
                    };

                    let session = Arc::clone(&session);
                    // Each request is answered on its own task, so a held one
                    // does not stall the rest of the page.
                    tokio::spawn(async move {
                        let Some((action, hits, settled)) = matched else {
                            let _ = session.send(continue_request(&event)).await;
                            return;
                        };
                        match action {
                            Action::Abort => {
                                let _ = session
                                    .send(fail_request(&event, ErrorReason::BlockedByClient))
                                    .await;
                            }
                            Action::Delay(delay) => {
                                tokio::time::sleep(delay).await;
                                // The send can fail: while the request was
                                // held, the page's own stale-response guard may
                                // have aborted it. That is the post-fix
                                // behaviour the scenario is asserting, not an
                                // error.
                                let _ = session.send(continue_request(&event)).await;
                            }
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        settled.notify_waiters();
                    });
                }
            }
        });

        Ok(Self {
            rules,
            documents,
            dispatcher,
            responses,
        })
    }

    /// The status code the most recent navigation to a URL containing
    /// `needle` answered with.
    ///
    /// Stands in for the `Response` object `page.goto()` returns. Reads the
    /// *last* match, because a link followed twice logs twice.
    pub async fn document_status(&self, needle: &str) -> Option<u32> {
        self.documents
            .lock()
            .await
            .iter()
            .rev()
            .find(|document| document.url.contains(needle))
            .map(|document| document.status)
    }

    /// Adds a rule, returning a handle for waiting on and counting its hits.
    ///
    /// Rules are tried in the order they were added, first match wins.
    ///
    /// # Errors
    ///
    /// Fails when `pattern` is not a valid regular expression.
    pub async fn route(&self, pattern: &str, action: Action) -> Result<RouteHandle> {
        let regex = Regex::new(pattern)
            .with_context(|| format!("`{pattern}` is not a valid URL pattern"))?;
        let hits = Arc::new(AtomicUsize::new(0));
        let settled = Arc::new(Notify::new());
        self.rules.lock().await.push(Rule {
            pattern: regex,
            action,
            hits: Arc::clone(&hits),
            settled: Arc::clone(&settled),
        });
        Ok(RouteHandle { hits, settled })
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.dispatcher.abort();
        self.responses.abort();
    }
}

/// A rule's counter and its settle signal.
#[derive(Debug, Clone)]
pub struct RouteHandle {
    hits: Arc<AtomicUsize>,
    settled: Arc<Notify>,
}

impl RouteHandle {
    /// How many requests this rule has answered so far.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Waits until one more request handled by this rule has been answered.
    ///
    /// Returns immediately if one already has — `Notify` alone would miss a
    /// notification sent before the wait started, which is the common case when
    /// the delay is shorter than the step that follows it.
    ///
    /// # Errors
    ///
    /// Fails when nothing settles within `timeout`.
    pub async fn wait_for_settled(&self, timeout: Duration) -> Result<()> {
        if self.hits() > 0 {
            return Ok(());
        }
        tokio::time::timeout(timeout, self.settled.notified())
            .await
            .context("no held request settled in time")?;
        Ok(())
    }
}

fn continue_request(event: &RequestPaused) -> ContinueRequest {
    ContinueRequest {
        request_id: event.request_id.clone(),
        url: None,
        method: None,
        post_data: None,
        headers: None,
    }
}

fn fail_request(event: &RequestPaused, reason: ErrorReason) -> FailRequest {
    FailRequest {
        request_id: event.request_id.clone(),
        error_reason: reason,
    }
}
