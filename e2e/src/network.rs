//! Request interception over the CDP `Fetch` domain, which `WebDriver` lacks.
//! Needs thirtyfour's `cdp-events` feature: receiving `Fetch.requestPaused`
//! requires the WebSocket transport.
//!
//! Every paused request must be answered exactly once or the page hangs;
//! requests matching no rule are continued unmodified.

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
    Watch,
    /// Refuse it, as an ad blocker or an offline network would.
    Abort,
    /// Hold it for a while, then let it through.
    Delay(Duration),
    /// Hold it until released: [`Action::Delay`] without the clock.
    Hold(watch::Receiver<bool>),
    /// Answer it here, without going to the network.
    Fulfill { content_type: String, body: String },
}

/// One URL pattern and what to do with the requests it matches.
#[derive(Debug)]
struct Rule {
    pattern: Regex,
    /// Only match this HTTP method, when set.
    method: Option<String>,
    action: Action,
    /// Matching requests intercepted, counted before any hold.
    arrived: Arc<AtomicUsize>,
    /// Matching requests answered.
    hits: Arc<AtomicUsize>,
    /// Notified when a request is paused, and again once it is answered.
    signal: Arc<Notify>,
}

/// A CDP attachment: request rules plus a log of top-level document statuses.
/// Dropping it stops both listeners.
#[derive(Debug)]
pub struct Network {
    rules: Arc<Mutex<Vec<Rule>>>,
    documents: Arc<Mutex<Vec<Document>>>,
    dispatcher: JoinHandle<()>,
    responses: JoinHandle<()>,
}

/// One navigation's URL and status code.
#[derive(Debug, Clone)]
pub struct Document {
    pub url: String,
    pub status: u32,
}

impl Network {
    /// Attaches to a browser and starts answering paused requests.
    pub async fn attach(driver: &WebDriver) -> Result<Self> {
        let session = Arc::new(
            driver
                .cdp()
                .connect()
                .await
                .context("opening a CDP WebSocket for request interception")?,
        );

        // Subscribe before enabling: a request paused with no listener stalls
        // the page.
        let mut paused = session
            .subscribe::<RequestPaused>()
            .await
            .context("subscribing to Fetch.requestPaused")?;
        // `WebDriver` cannot report a navigation's status code.
        let mut received = session
            .subscribe::<ResponseReceived>()
            .await
            .context("subscribing to Network.responseReceived")?;
        session
            .send(Enable {
                // Every request; the rules decide what is interesting.
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
                        // No URL to match on; let it through.
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
                    // Own task per request, so a held one does not stall others.
                    tokio::spawn(async move {
                        let Some((action, arrived, hits, signal)) = matched else {
                            let _ = session.send(continue_request(&event)).await;
                            return;
                        };
                        // Announced before any hold, so "request made" and
                        // "response landed" can be awaited separately.
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
                                // May fail if the page's stale-response guard
                                // aborted it meanwhile; that is expected.
                                let _ = session.send(continue_request(&event)).await;
                            }
                            Action::Hold(mut release) => {
                                // A release that beats the request is not
                                // lost: `changed()` returns immediately.
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

    /// Status of the most recent navigation to a URL containing `needle`.
    pub async fn document_status(&self, needle: &str) -> Option<u32> {
        self.documents
            .lock()
            .await
            .iter()
            .rev()
            .find(|document| document.url.contains(needle))
            .map(|document| document.status)
    }

    /// Adds a rule for every method. Rules are tried in order; first match wins.
    pub async fn route(&self, pattern: &str, action: Action) -> Result<RouteHandle> {
        self.route_method(pattern, None, action).await
    }

    /// Adds a rule scoped to one HTTP method.
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

/// A rule's counters and progress signal, plus the release switch for a hold.
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
    pub async fn wait_for_arrival(&self, timeout: Duration) -> Result<()> {
        self.wait_until(timeout, || self.arrived() > 0)
            .await
            .context("no matching request was made in time")
    }

    /// Waits until a matching request has been answered.
    pub async fn wait_for_settled(&self, timeout: Duration) -> Result<()> {
        self.wait_until(timeout, || self.hits() > 0)
            .await
            .context("no held request settled in time")
    }

    /// Lets a held request through.
    pub fn release(&self) -> Result<()> {
        let release = self
            .release
            .as_ref()
            .context("this route was not created with `hold`")?;
        // A closed channel just means the browser is closing.
        let _ = release.send(true);
        Ok(())
    }

    /// Polls `done` between notifications, checking first: `Notify` drops
    /// notifications sent before anyone waits.
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
        // CDP takes the body base64-encoded.
        body: Some(BASE64.encode(body)),
        response_phrase: None,
    }
}
