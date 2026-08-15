//! The Cucumber world: one browser session and one throwaway account per
//! scenario.
//!
//! The session cannot be opened in `new`, because whether the page's scripts
//! run is decided by the scenario's `@nojs` tag and `World::new` never sees it.
//! A `before` hook opens it instead, which is also the only order that works:
//! `Emulation.setScriptExecutionDisabled` applies to the next document, so it
//! has to be issued before the first navigation.
//!
//! `support/fixtures.js` built the same state out of worker-scoped Playwright
//! fixtures. The server, its database and the mock upstreams are now
//! process-wide (see `server.rs`); what stays per-scenario is the account and
//! the browser.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use cucumber::World;
use thirtyfour::prelude::*;

use crate::api::{Api, Credentials, PASSWORD};
use crate::browser::{Browser, Scripting, Viewport};
use crate::network::{Action, Network, RouteHandle};
use crate::seed::Seed;
use crate::server::Endpoints;

/// Set once by the runner, read by every `World::new`.
static ENDPOINTS: OnceLock<Endpoints> = OnceLock::new();

/// Publishes the running server's addresses to the worlds.
///
/// # Panics
///
/// Panics if called twice.
pub fn set_endpoints(endpoints: Endpoints) {
    ENDPOINTS
        .set(endpoints)
        .expect("the endpoints are published once, before any scenario runs");
}

/// The running server's addresses.
///
/// # Panics
///
/// Panics when the runner has not published them yet.
pub fn endpoints() -> &'static Endpoints {
    ENDPOINTS
        .get()
        .expect("the runner publishes the endpoints before the first scenario")
}

/// State shared by the steps of one scenario.
#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct RdrsWorld {
    /// The account this scenario runs as, registered lazily by the sign-in
    /// step so that scenarios about signing up can claim the name themselves.
    pub user: Credentials,
    /// The account's row id, once it exists.
    user_id: Option<i64>,
    api: Api,
    seed: Seed,
    browser: Option<Browser>,
    /// Ids of entries seeded during the scenario, in insertion order — what
    /// the "the second entry" style steps index into.
    pub seeded_entries: Vec<i64>,
    /// The one-time link an admin issued for this account, when a scenario is
    /// about redeeming it. `currentUser.invitePath` in the JavaScript suite.
    pub invite_path: Option<String>,
    /// A second account the scenario created, when it needs a row in the admin
    /// table that is not its own. `currentUser.otherUsername` before.
    pub other_username: Option<String>,
    /// The sidebar's unread count, read just before a mutation fired out of
    /// band — what the SSE assertion compares against.
    pub unread_before: Option<u32>,
    /// CDP request interception, attached the first time a scenario asks to
    /// hold a response back. Most scenarios never do, and the attachment costs
    /// a WebSocket, so it is not part of opening the browser.
    network: Option<Network>,
    /// The held fragment and full-content responses, by the step that armed
    /// them — `delayedFragments` / `delayedFullContentFetches` before.
    pub delayed_fragment: Option<RouteHandle>,
    pub delayed_full_content: Option<RouteHandle>,
    /// The list pane's `data-snapshot-at` when it was last tagged, for the
    /// "has the render stamp advanced?" assertion.
    pub pane_stamp: Option<String>,
    /// The SSE-driven summary fragment, held open until a step releases it.
    pub held_summary_fragment: Option<RouteHandle>,
    /// Counts re-queue POSTs, to prove an in-flight toggle is inert.
    pub summarize_posts: Option<RouteHandle>,
}

impl RdrsWorld {
    async fn new() -> Result<Self> {
        let endpoints = endpoints();
        Ok(Self {
            user: Credentials {
                username: format!("e2e-{}", crate::random_slug()),
                password: PASSWORD.to_owned(),
            },
            user_id: None,
            api: Api::new(&endpoints.base_url)?,
            seed: Seed::open(&endpoints.db_path).await?,
            browser: None,
            seeded_entries: Vec::new(),
            invite_path: None,
            other_username: None,
            unread_before: None,
            network: None,
            delayed_fragment: None,
            delayed_full_content: None,
            pane_stamp: None,
            held_summary_fragment: None,
            summarize_posts: None,
        })
    }

    /// This scenario's username and password, owned so the caller can keep
    /// using the world while it holds them.
    pub fn credentials(&self) -> (String, String) {
        (self.user.username.clone(), self.user.password.clone())
    }

    /// The one-time link issued for this account.
    ///
    /// # Errors
    ///
    /// Fails when no step has asked an admin to create the account.
    pub fn invite_path(&self) -> Result<String> {
        self.invite_path
            .clone()
            .context("no invite link: no step created an account for this scenario")
    }

    /// The second account this scenario created.
    ///
    /// # Errors
    ///
    /// Fails when no step registered one.
    pub fn other_username(&self) -> Result<String> {
        self.other_username
            .clone()
            .context("no second account: no step registered another user")
    }

    /// Opens the session for a scenario.
    ///
    /// # Errors
    ///
    /// Fails when no browser session can be started.
    pub async fn open(&mut self, scripting: Scripting) -> Result<()> {
        self.browser = Some(Browser::open(scripting).await?);
        Ok(())
    }

    /// Ends the session, if one was opened.
    ///
    /// # Errors
    ///
    /// Fails when the driver refuses to close.
    pub async fn close(&mut self) -> Result<()> {
        if let Some(browser) = self.browser.take() {
            browser.quit().await?;
        }
        Ok(())
    }

    /// The scenario's browser.
    ///
    /// # Errors
    ///
    /// Fails when no session was opened — a `before` hook that did not run.
    pub fn browser(&self) -> Result<&Browser> {
        self.browser
            .as_ref()
            .context("no browser session: the `before` hook did not open one")
    }

    /// The scenario's browser, mutably, for the emulations that latch.
    ///
    /// # Errors
    ///
    /// Fails when no session was opened.
    pub fn browser_mut(&mut self) -> Result<&mut Browser> {
        self.browser
            .as_mut()
            .context("no browser session: the `before` hook did not open one")
    }

    /// The scenario's driver.
    ///
    /// # Errors
    ///
    /// Fails when no session was opened.
    pub fn driver(&self) -> Result<&WebDriver> {
        Ok(self.browser()?.driver())
    }

    /// The account API for this server.
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// The seed helper for this server's database.
    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    /// The base URL of the server under test.
    pub fn base_url(&self) -> &str {
        &endpoints().base_url
    }

    /// A URL that answers with a valid RSS document.
    pub fn feed_url(&self) -> &str {
        &endpoints().feed_url
    }

    /// The row id of this scenario's account, resolving it on first use.
    ///
    /// # Errors
    ///
    /// Fails when the account has not been created yet.
    pub async fn user_id(&mut self) -> Result<i64> {
        if let Some(id) = self.user_id {
            return Ok(id);
        }
        let id = self.seed.user_id(&self.user.username).await?;
        self.user_id = Some(id);
        Ok(id)
    }

    /// Navigates to a path on the server under test.
    ///
    /// # Errors
    ///
    /// Fails when the navigation is refused.
    pub async fn goto(&self, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base_url());
        self.driver()?.goto(&url).await?;
        Ok(())
    }

    /// The current URL's path and query, the shape the steps assert against.
    ///
    /// # Errors
    ///
    /// Fails when the driver cannot report a URL.
    pub async fn path(&self) -> Result<String> {
        let url = self.driver()?.current_url().await?;
        Ok(match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        })
    }

    /// Waits for the browser to land on `expected`, Playwright's
    /// `waitForURL`.
    ///
    /// Compares path and query rather than the whole URL: the server's port is
    /// ephemeral, so the old assertions' `${serverUrl}/…` has no stable form
    /// here.
    ///
    /// # Errors
    ///
    /// Fails when the URL has not settled on `expected` in time.
    pub async fn expect_path(&self, expected: &str) -> Result<()> {
        crate::wait::eventually_eq(&format!("URL is {expected}"), expected.to_owned(), || {
            self.path()
        })
        .await
    }

    /// Resizes the viewport.
    ///
    /// # Errors
    ///
    /// Fails when the CDP command is refused.
    pub async fn resize(&mut self, viewport: Viewport) -> Result<()> {
        self.browser_mut()?.set_viewport(viewport).await
    }

    /// Holds every request matching `pattern` for `delay`, then lets it
    /// through — Playwright's `page.route` with a sleep.
    ///
    /// # Errors
    ///
    /// Fails when CDP cannot be attached, or the pattern is invalid.
    pub async fn delay_requests(&mut self, pattern: &str, delay: Duration) -> Result<RouteHandle> {
        self.route(pattern, Action::Delay(delay)).await
    }

    /// Answers every request to the seeded entries' origin with a stub page.
    ///
    /// The shortcut that opens an entry's link in a new tab asserts on *which*
    /// URL it targets, not that the page loads — and `https://example.com`
    /// fails DNS resolution on a machine without internet, which collapses the
    /// popup's URL to `chrome-error://chromewebdata/`.
    ///
    /// # Errors
    ///
    /// Fails when CDP cannot be attached.
    pub async fn stub_external_pages(&mut self) -> Result<()> {
        self.route(
            r"^https://example\.com/",
            Action::Fulfill {
                content_type: "text/html".to_owned(),
                body: "<!doctype html><title>stubbed external page</title>".to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    /// GETs from the server as the signed-in browser, returning the body.
    ///
    /// `page.request.get` in the JavaScript suite — used for the OPML export,
    /// which is a download rather than a page.
    ///
    /// # Errors
    ///
    /// Fails when the request cannot be sent, or answers 4xx/5xx.
    pub async fn get_as_user(&self, path: &str) -> Result<String> {
        let (jar, csrf) = self.browser_credentials().await?;
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?
            .get(format!("{}{path}", self.base_url()))
            .header("Cookie", jar)
            .header("X-CSRF-Token", csrf)
            .send()
            .await
            .with_context(|| format!("getting {path} as the signed-in user"))?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "GET {path} answered {status}");
        Ok(response.text().await?)
    }

    /// The browser's cookie jar as a `Cookie` header, plus its CSRF token.
    async fn browser_credentials(&self) -> Result<(String, String)> {
        let cookies = self.driver()?.get_all_cookies().await?;
        let jar = cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        // The server writes `__Host-csrf_token` instead of `csrf_token`
        // whenever the deployment is Secure. E2E runs over plain HTTP, so this
        // is not load-bearing yet — but it will be the day E2E moves to HTTPS.
        let csrf = cookies
            .iter()
            .find(|cookie| cookie.name == "__Host-csrf_token")
            .or_else(|| cookies.iter().find(|cookie| cookie.name == "csrf_token"))
            .map(|cookie| cookie.value.clone())
            .unwrap_or_default();
        Ok((jar, csrf))
    }

    /// Holds every request matching `pattern` open until the handle is
    /// released.
    ///
    /// # Errors
    ///
    /// Fails when CDP cannot be attached, or the pattern is invalid.
    pub async fn hold_requests(&mut self, pattern: &str) -> Result<RouteHandle> {
        self.attach_network().await?;
        self.network
            .as_ref()
            .expect("just attached the interceptor above")
            .hold(pattern)
            .await
    }

    /// Counts matching requests of one method without changing them —
    /// Playwright's `page.on("request", …)`.
    ///
    /// # Errors
    ///
    /// Fails when CDP cannot be attached, or the pattern is invalid.
    pub async fn watch_requests(&mut self, pattern: &str, method: &str) -> Result<RouteHandle> {
        self.attach_network().await?;
        self.network
            .as_ref()
            .expect("just attached the interceptor above")
            .route_method(pattern, Some(method), Action::Watch)
            .await
    }

    /// Adds an interception rule, attaching CDP on first use.
    ///
    /// Most scenarios never intercept anything and the attachment costs a
    /// WebSocket, so it is not part of opening the browser.
    async fn route(&mut self, pattern: &str, action: Action) -> Result<RouteHandle> {
        self.attach_network().await?;
        self.network
            .as_ref()
            .expect("just attached the interceptor above")
            .route(pattern, action)
            .await
    }

    async fn attach_network(&mut self) -> Result<()> {
        if self.network.is_none() {
            let driver = self.browser()?.driver();
            self.network = Some(Network::attach(driver).await?);
        }
        Ok(())
    }

    /// POSTs to the server as the signed-in browser, out of band.
    ///
    /// `page.request.post` in the JavaScript suite: it shares the browser's
    /// session cookie but bypasses the page's own patched `fetch`, so it must
    /// attach the CSRF token itself — exactly what `csrf.js` does in the real
    /// UI, echoing the readable `csrf_token` cookie back as `X-CSRF-Token`.
    ///
    /// Used to fire a mutation the page did not initiate, so the SSE event it
    /// emits is the only thing that can update the open page.
    ///
    /// # Errors
    ///
    /// Fails when the request cannot be sent, or answers 4xx/5xx.
    pub async fn post_as_user(&self, path: &str) -> Result<()> {
        let (jar, csrf) = self.browser_credentials().await?;
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?
            .post(format!("{}{path}", self.base_url()))
            .header("Cookie", jar)
            .header("X-CSRF-Token", csrf)
            .send()
            .await
            .with_context(|| format!("posting {path} as the signed-in user"))?;
        // Any non-error status will do — 200, or a 303 back to the list. What
        // matters is the side effect, not the body.
        anyhow::ensure!(
            !response.status().is_client_error() && !response.status().is_server_error(),
            "POST {path} answered {}",
            response.status()
        );
        Ok(())
    }
}
