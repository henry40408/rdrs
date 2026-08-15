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
//! `WebDriver` connection, but `Fetch.requestPaused` has to be *received*, which
//! only the WebSocket transport can do.
//!
//! Every paused request must be answered — continued, failed or fulfilled — or
//! the page hangs waiting on it. The dispatcher below answers each one exactly
//! once, and requests matching no rule are continued unmodified.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::StreamExt;
use regex::Regex;
use thirtyfour::cdp::domains::fetch::{
    ContinueRequest, Enable, FailRequest, FulfillRequest, HeaderEntry, RequestPattern,
    RequestPaused,
};
use thirtyfour::cdp::domains::network::{ErrorReason, ResourceType, ResponseReceived};
use thirtyfour::prelude::*;
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;

/// What to do with a request whose URL matches a rule.
#[derive(Debug, Clone)]
pub enum Action {
    /// Let it through untouched, and only count it.
    ///
    /// The shape of Playwright's `page.on("request", …)`: the scenario wants to
    /// know whether a request fired at all, not to change it.
    Watch,
    /// Refuse it, as an ad blocker or an offline network would.
    Abort,
    /// Hold it for a while, then let it through.
    ///
    /// The delay is served without blocking other requests: the dispatcher
    /// hands each paused request to its own task.
    Delay(Duration),
    /// Hold it until the scenario says otherwise, then let it through.
    ///
    /// The open-ended form of [`Action::Delay`], for the races whose second
    /// half is triggered by something other than the clock — the summary
    /// fragment held until the pane has moved on.
    Hold(watch::Receiver<bool>),
    /// Answer it here, without going to the network at all — Playwright's
    /// `route.fulfill`.
    Fulfill { content_type: String, body: String },
}

/// One URL pattern and what to do with the requests it matches.
#[derive(Debug)]
struct Rule {
    pattern: Regex,
    /// Only match this HTTP method, when set.
    method: Option<String>,
    action: Action,
    /// How many matching requests have been intercepted, counted the moment
    /// they are paused — before any hold.
    arrived: Arc<AtomicUsize>,
    /// How many have been answered.
    hits: Arc<AtomicUsize>,
    /// Notified when a request is paused, and again once it is answered.
    signal: Arc<Notify>,
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
                    let url = event
                        .response
                        .get("url")
                        .and_then(serde_json::Value::as_str);
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
                    let method = event
                        .request
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let matched = {
                        let rules = rules.lock().await;
                        rules
                            .iter()
                            .find(|rule| {
                                rule.pattern.is_match(url)
                                    && rule.method.as_ref().is_none_or(|wanted| wanted == method)
                            })
                            .map(|rule| {
                                (
                                    rule.action.clone(),
                                    Arc::clone(&rule.arrived),
                                    Arc::clone(&rule.hits),
                                    Arc::clone(&rule.signal),
                                )
                            })
                    };

                    let session = Arc::clone(&session);
                    // Each request is answered on its own task, so a held one
                    // does not stall the rest of the page.
                    tokio::spawn(async move {
                        let Some((action, arrived, hits, signal)) = matched else {
                            let _ = session.send(continue_request(&event)).await;
                            return;
                        };
                        // Counted and announced before any hold, so a step can
                        // wait for "the request has been made" separately from
                        // "the response has landed".
                        arrived.fetch_add(1, Ordering::SeqCst);
                        signal.notify_waiters();

                        match action {
                            Action::Watch => {
                                let _ = session.send(continue_request(&event)).await;
                            }
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
                            Action::Hold(mut release) => {
                                // `changed()` returns immediately when the
                                // sender already flipped it, so a release that
                                // beats the request here is not lost.
                                while !*release.borrow_and_update() {
                                    if release.changed().await.is_err() {
                                        break;
                                    }
                                }
                                let _ = session.send(continue_request(&event)).await;
                            }
                            Action::Fulfill { content_type, body } => {
                                let _ = session
                                    .send(fulfill_request(&event, &content_type, &body))
                                    .await;
                            }
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        signal.notify_waiters();
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

    /// Adds a rule for every method, returning a handle for waiting on and
    /// counting its requests.
    ///
    /// Rules are tried in the order they were added, first match wins.
    ///
    /// # Errors
    ///
    /// Fails when `pattern` is not a valid regular expression.
    pub async fn route(&self, pattern: &str, action: Action) -> Result<RouteHandle> {
        self.route_method(pattern, None, action).await
    }

    /// Adds a rule scoped to one HTTP method.
    ///
    /// # Errors
    ///
    /// Fails when `pattern` is not a valid regular expression.
    pub async fn route_method(
        &self,
        pattern: &str,
        method: Option<&str>,
        action: Action,
    ) -> Result<RouteHandle> {
        let regex = Regex::new(pattern)
            .with_context(|| format!("`{pattern}` is not a valid URL pattern"))?;
        let arrived = Arc::new(AtomicUsize::new(0));
        let hits = Arc::new(AtomicUsize::new(0));
        let signal = Arc::new(Notify::new());
        self.rules.lock().await.push(Rule {
            pattern: regex,
            method: method.map(str::to_owned),
            action,
            arrived: Arc::clone(&arrived),
            hits: Arc::clone(&hits),
            signal: Arc::clone(&signal),
        });
        Ok(RouteHandle {
            arrived,
            hits,
            signal,
            release: None,
        })
    }

    /// Holds every matching request open until the handle is released.
    ///
    /// # Errors
    ///
    /// Fails when `pattern` is not a valid regular expression.
    pub async fn hold(&self, pattern: &str) -> Result<RouteHandle> {
        let (release, gate) = watch::channel(false);
        let mut handle = self.route(pattern, Action::Hold(gate)).await?;
        handle.release = Some(Arc::new(release));
        Ok(handle)
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.dispatcher.abort();
        self.responses.abort();
    }
}

/// A rule's counters, its progress signal, and — for a held route — the switch
/// that lets the request go.
#[derive(Debug, Clone)]
pub struct RouteHandle {
    arrived: Arc<AtomicUsize>,
    hits: Arc<AtomicUsize>,
    signal: Arc<Notify>,
    release: Option<Arc<watch::Sender<bool>>>,
}

impl RouteHandle {
    /// How many matching requests have been intercepted, held or not.
    pub fn arrived(&self) -> usize {
        self.arrived.load(Ordering::SeqCst)
    }

    /// How many matching requests have been answered.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Waits until a matching request has been intercepted.
    ///
    /// # Errors
    ///
    /// Fails when none arrives within `timeout`.
    pub async fn wait_for_arrival(&self, timeout: Duration) -> Result<()> {
        self.wait_until(timeout, || self.arrived() > 0)
            .await
            .context("no matching request was made in time")
    }

    /// Waits until a matching request has been answered.
    ///
    /// # Errors
    ///
    /// Fails when nothing settles within `timeout`.
    pub async fn wait_for_settled(&self, timeout: Duration) -> Result<()> {
        self.wait_until(timeout, || self.hits() > 0)
            .await
            .context("no held request settled in time")
    }

    /// Lets a held request through.
    ///
    /// # Errors
    ///
    /// Fails when this handle did not come from [`Network::hold`].
    pub fn release(&self) -> Result<()> {
        let release = self
            .release
            .as_ref()
            .context("this route was not created with `hold`")?;
        // Ignores a closed channel: the dispatcher having gone away means the
        // browser is closing, not that the release failed.
        let _ = release.send(true);
        Ok(())
    }

    /// Polls `done` between notifications.
    ///
    /// The check comes first each time round, because `Notify` drops a
    /// notification sent before anyone was waiting — the common case when the
    /// request settles faster than the step that follows it.
    async fn wait_until(&self, timeout: Duration, done: impl Fn() -> bool) -> Result<()> {
        tokio::time::timeout(timeout, async {
            loop {
                if done() {
                    return;
                }
                self.signal.notified().await;
            }
        })
        .await?;
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

fn fulfill_request(event: &RequestPaused, content_type: &str, body: &str) -> FulfillRequest {
    FulfillRequest {
        request_id: event.request_id.clone(),
        response_code: 200,
        response_headers: Some(vec![HeaderEntry {
            name: "Content-Type".to_owned(),
            value: content_type.to_owned(),
        }]),
        // CDP takes the body base64-encoded, which is also how it carries
        // binary responses.
        body: Some(BASE64.encode(body)),
        response_phrase: None,
    }
}
