//! The Cucumber world: one browser session, one throwaway account and one
//! borrowed server per scenario.
//!
//! The session opens in a `before` hook, not `new`: only the hook sees the
//! `@nojs` tag, and `Emulation.setScriptExecutionDisabled` applies to the next
//! document.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use cucumber::World;
use thirtyfour::prelude::*;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::api::{Api, Credentials, PASSWORD};
use crate::browser::{Browser, Scripting, Viewport};
use crate::network::{Action, Network, RouteHandle};
use crate::seed::Seed;
use crate::server::Endpoints;

/// The pool of servers scenarios borrow from, published once by the runner.
static POOL: OnceLock<Pool> = OnceLock::new();

/// Interchangeable servers, one borrowed per running scenario. Not one shared
/// server: its summary worker runs one job at a time, so concurrent summarizing
/// scenarios would queue past any assertion timeout.
struct Pool {
    /// Servers not currently lent out.
    free: std::sync::Mutex<Vec<Endpoints>>,
    /// One permit per server, so `new` waits rather than finding `free` empty.
    permits: Arc<Semaphore>,
}

/// Publishes the pool to the worlds.
///
/// # Panics
///
/// Panics if called twice.
pub fn set_pool(servers: Vec<Endpoints>) {
    let permits = Arc::new(Semaphore::new(servers.len()));
    POOL.set(Pool {
        free: std::sync::Mutex::new(servers),
        permits,
    })
    .ok()
    .expect("the pool is published once, before any scenario runs");
}

fn pool() -> &'static Pool {
    POOL.get()
        .expect("the runner publishes the pool before the first scenario")
}

/// State shared by the steps of one scenario.
#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct RdrsWorld {
    /// Registered lazily by the sign-in step, so sign-up scenarios can claim
    /// the name themselves.
    pub user: Credentials,
    /// The account's row id, once it exists.
    user_id: Option<i64>,
    api: Api,
    seed: Seed,
    browser: Option<Browser>,
    /// Seeded entry ids in insertion order, for "the second entry" steps.
    pub seeded_entries: Vec<i64>,
    /// The one-time invite link issued for this account.
    pub invite_path: Option<String>,
    /// A second account, for admin-table rows that are not the scenario's own.
    pub other_username: Option<String>,
    /// Sidebar unread count before an out-of-band mutation, for SSE asserts.
    pub unread_before: Option<u32>,
    /// CDP interception, attached lazily since it costs a WebSocket.
    network: Option<Network>,
    /// Held fragment and full-content responses.
    pub delayed_fragment: Option<RouteHandle>,
    pub delayed_full_content: Option<RouteHandle>,
    /// Last-seen list pane `data-snapshot-at`, to assert the stamp advanced.
    pub pane_stamp: Option<String>,
    /// The SSE-driven summary fragment, held open until a step releases it.
    pub held_summary_fragment: Option<RouteHandle>,
    /// Counts re-queue POSTs, to prove an in-flight toggle is inert.
    pub summarize_posts: Option<RouteHandle>,
    /// Held summarize POST, so the busy label stays up long enough to measure.
    pub delayed_summarize: Option<RouteHandle>,
    /// Mobile action bar `(label, x, width)` per button, captured before a
    /// label changes.
    pub action_bar: Option<Vec<(String, f64, f64)>>,
    /// The server this scenario borrowed, returned to the pool on drop.
    endpoints: Option<Endpoints>,
    /// Keeps the pool from lending this server twice.
    _lease: OwnedSemaphorePermit,
}

impl Drop for RdrsWorld {
    /// Returns the server before the lease drops, so the next permit holder
    /// always finds one.
    fn drop(&mut self) {
        if let Some(endpoints) = self.endpoints.take()
            && let Ok(mut free) = pool().free.lock()
        {
            free.push(endpoints);
        }
    }
}

impl RdrsWorld {
    async fn new() -> Result<Self> {
        let pool = pool();
        // The permit makes popping a free server below infallible.
        let lease = Arc::clone(&pool.permits)
            .acquire_owned()
            .await
            .context("the server pool was closed")?;
        let endpoints = pool
            .free
            .lock()
            .expect("the pool lock is never held across a panic")
            .pop()
            .expect("a permit guarantees a free server");

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
            delayed_summarize: None,
            action_bar: None,
            endpoints: Some(endpoints),
            _lease: lease,
        })
    }

    /// This scenario's username and password, owned.
    pub fn credentials(&self) -> (String, String) {
        (self.user.username.clone(), self.user.password.clone())
    }

    /// The one-time link issued for this account, if a step created one.
    pub fn invite_path(&self) -> Result<String> {
        self.invite_path
            .clone()
            .context("no invite link: no step created an account for this scenario")
    }

    /// The second account, if a step registered one.
    pub fn other_username(&self) -> Result<String> {
        self.other_username
            .clone()
            .context("no second account: no step registered another user")
    }

    /// Opens the session for a scenario.
    pub async fn open(&mut self, scripting: Scripting) -> Result<()> {
        self.browser = Some(Browser::open(scripting).await?);
        Ok(())
    }

    /// Ends the session, if one was opened.
    pub async fn close(&mut self) -> Result<()> {
        if let Some(browser) = self.browser.take() {
            browser.quit().await?;
        }
        Ok(())
    }

    /// The scenario's browser, opened by the `before` hook.
    pub fn browser(&self) -> Result<&Browser> {
        self.browser
            .as_ref()
            .context("no browser session: the `before` hook did not open one")
    }

    /// The scenario's browser, mutably, for the emulations that latch.
    pub fn browser_mut(&mut self) -> Result<&mut Browser> {
        self.browser
            .as_mut()
            .context("no browser session: the `before` hook did not open one")
    }

    /// The scenario's driver.
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

    /// The addresses of the server this scenario borrowed.
    fn endpoints(&self) -> &Endpoints {
        self.endpoints
            .as_ref()
            .expect("the world holds its server until it is dropped")
    }

    /// The base URL of the server under test.
    pub fn base_url(&self) -> &str {
        &self.endpoints().base_url
    }

    /// A URL that answers with a valid RSS document.
    pub fn feed_url(&self) -> &str {
        &self.endpoints().feed_url
    }

    /// The row id of this scenario's account, resolving it on first use.
    pub async fn user_id(&mut self) -> Result<i64> {
        if let Some(id) = self.user_id {
            return Ok(id);
        }
        let id = self.seed.user_id(&self.user.username).await?;
        self.user_id = Some(id);
        Ok(id)
    }

    /// Navigates to a path on the server under test.
    pub async fn goto(&self, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base_url());
        self.driver()?.goto(&url).await?;
        Ok(())
    }

    /// The current URL's path and query, the shape the steps assert against.
    pub async fn path(&self) -> Result<String> {
        let url = self.driver()?.current_url().await?;
        Ok(match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        })
    }

    /// Waits for the browser's path and query to equal `expected` (the port is
    /// ephemeral, so full URLs are unstable).
    pub async fn expect_path(&self, expected: &str) -> Result<()> {
        crate::wait::eventually_eq(&format!("URL is {expected}"), expected.to_owned(), || {
            self.path()
        })
        .await
    }

    /// Resizes the viewport.
    pub async fn resize(&mut self, viewport: Viewport) -> Result<()> {
        self.browser_mut()?.set_viewport(viewport).await
    }

    /// Holds every request matching `pattern` for `delay`, then lets it through.
    pub async fn delay_requests(&mut self, pattern: &str, delay: Duration) -> Result<RouteHandle> {
        self.route(pattern, Action::Delay(delay)).await
    }

    /// Stubs the seeded entries' origin: offline, `https://example.com` fails
    /// DNS and the popup URL collapses to `chrome-error://chromewebdata/`.
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

    /// GETs as the signed-in browser and returns the body (e.g. OPML export).
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
        // Secure deployments use `__Host-csrf_token`; E2E is plain HTTP today.
        let csrf = cookies
            .iter()
            .find(|cookie| cookie.name == "__Host-csrf_token")
            .or_else(|| cookies.iter().find(|cookie| cookie.name == "csrf_token"))
            .map(|cookie| cookie.value.clone())
            .unwrap_or_default();
        Ok((jar, csrf))
    }

    /// Holds matching requests open until the handle is released.
    pub async fn hold_requests(&mut self, pattern: &str) -> Result<RouteHandle> {
        self.attach_network().await?;
        self.network
            .as_ref()
            .expect("just attached the interceptor above")
            .hold(pattern)
            .await
    }

    /// Counts matching requests of one method without changing them.
    pub async fn watch_requests(&mut self, pattern: &str, method: &str) -> Result<RouteHandle> {
        self.attach_network().await?;
        self.network
            .as_ref()
            .expect("just attached the interceptor above")
            .route_method(pattern, Some(method), Action::Watch)
            .await
    }

    /// Adds an interception rule, attaching CDP on first use.
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

    /// POSTs as the signed-in browser, out of band, so only the resulting SSE
    /// event can update the open page. Attaches the CSRF token as `csrf.js` would.
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
        // Any non-error status (200 or 303); only the side effect matters.
        anyhow::ensure!(
            !response.status().is_client_error() && !response.status().is_server_error(),
            "POST {path} answered {}",
            response.status()
        );
        Ok(())
    }
}
