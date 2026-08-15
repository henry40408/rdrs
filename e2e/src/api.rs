//! Account creation over HTTP, the way an operator would do it.
//!
//! rdrs has no public sign-up: `/api/setup` creates the very first account and
//! then closes for good, and every later account is created by an admin who
//! hands out a one-time link. Scenarios still want a throwaway user each, so
//! the first call claims the setup endpoint and everything after it goes
//! through the real admin + invite flow.
//!
//! A port of `support/api.js`, including its map of bootstrap admins keyed by
//! base URL: the suite runs a pool of servers, each with its own database, so
//! each needs its own first account.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use percent_encoding::percent_decode_str;
use reqwest::header::{HeaderValue, SET_COOKIE};
use reqwest::{Client, StatusCode, redirect};
use tokio::sync::Mutex;

/// The password every account this suite creates is given.
pub const PASSWORD: &str = "vulture-mango-77-quilt";

/// The account that claimed `/api/setup` on each server, created on first use.
///
/// `/api/setup` closes for good once claimed, so this must be remembered per
/// server rather than per scenario — every later account on that server is
/// created by its admin.
static BOOTSTRAP_ADMINS: Mutex<Option<HashMap<String, Credentials>>> = Mutex::const_new(None);

/// A username and password pair.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// An authenticated session: the cookie jar as a header value, and the CSRF
/// token to echo back on state-changing requests.
#[derive(Debug, Clone)]
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

/// Talks to one rdrs server's account endpoints.
#[derive(Debug, Clone)]
pub struct Api {
    base_url: String,
    client: Client,
}

impl Api {
    /// Builds a client that reports redirects instead of following them — the
    /// invite link only exists in the flash cookie on a 303 that a following
    /// client would consume.
    ///
    /// # Errors
    ///
    /// Fails when the HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .build()
            .context("building the API client")?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    /// Creates an account with a password, whatever it takes.
    ///
    /// # Errors
    ///
    /// Fails when the admin cannot be created, the invite cannot be issued, or
    /// the invite cannot be redeemed.
    pub async fn register(&self, username: &str, password: &str) -> Result<()> {
        let admin = self.ensure_admin().await?;
        if admin.username == username {
            return Ok(());
        }
        let invite = self.invite_account(username).await?;
        self.redeem_invite(&invite, password).await
    }

    /// Claims the one-time setup endpoint for `username`.
    ///
    /// The account it creates is the instance's administrator, which is what
    /// the README screenshots depict — a single-user install, sidebar and all.
    /// Going through [`Api::register`] instead would create an ordinary member
    /// account and quietly drop the admin entries from every captured sidebar.
    ///
    /// # Errors
    ///
    /// Fails when setup has already been claimed, or is refused.
    pub async fn setup_first_account(&self, username: &str, password: &str) -> Result<()> {
        self.claim_setup(username, password).await
    }

    /// Creates an account and hands back its one-time link, unredeemed.
    ///
    /// The half of [`Api::register`] that stops before choosing a password, for
    /// scenarios that drive the invite page in the browser.
    ///
    /// # Errors
    ///
    /// Fails when the admin cannot sign in, or the account cannot be created.
    pub async fn invite_account(&self, username: &str) -> Result<String> {
        let admin = self.ensure_admin().await?;
        let session = self.login(&admin.username, &admin.password).await?;
        self.create_account(&session, username).await
    }

    /// Signs in and returns the session cookie and CSRF token.
    ///
    /// # Errors
    ///
    /// Fails when the credentials are refused.
    pub async fn login(&self, username: &str, password: &str) -> Result<Session> {
        let response = self
            .client
            .post(format!("{}/api/session", self.base_url))
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .context("posting /api/session")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("login failed ({status}): {body}");
        }

        let cookies = set_cookies(&response);
        // The synchronizer-token guard wants this echoed back as a header on
        // every state-changing request, which is what csrf.js does in the
        // browser.
        let csrf = cookie_value(&cookies, "csrf_token").unwrap_or_default();
        Ok(Session {
            cookie: cookie_header(&cookies),
            csrf,
        })
    }

    /// This server's bootstrap admin, claiming `/api/setup` the first time.
    ///
    /// The lock is held across the claim so two scenarios starting at once on
    /// the same server cannot both try it — the second would be refused, since
    /// setup closes for good.
    async fn ensure_admin(&self) -> Result<Credentials> {
        let mut admins = BOOTSTRAP_ADMINS.lock().await;
        let admins = admins.get_or_insert_with(HashMap::new);
        if let Some(admin) = admins.get(&self.base_url) {
            return Ok(admin.clone());
        }
        let admin = Credentials {
            username: format!("e2e-bootstrap-{}", crate::random_slug()),
            password: PASSWORD.to_owned(),
        };
        self.claim_setup(&admin.username, &admin.password).await?;
        admins.insert(self.base_url.clone(), admin.clone());
        Ok(admin)
    }

    async fn claim_setup(&self, username: &str, password: &str) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/api/setup", self.base_url))
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .context("posting /api/setup")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("setup failed ({status}): {body}");
        }
        Ok(())
    }

    async fn create_account(&self, session: &Session, username: &str) -> Result<String> {
        let response = self
            .client
            .post(format!("{}/admin/users", self.base_url))
            .header("Cookie", &session.cookie)
            .header("X-CSRF-Token", &session.csrf)
            .form(&[("username", username), ("role", "user")])
            .send()
            .await
            .context("posting /admin/users")?;
        if response.status() != StatusCode::SEE_OTHER {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("creating the account failed ({status}): {body}");
        }

        // The link is shown once, in the flash cookie, and stored only as an
        // HMAC — reading it here is exactly what an admin does on the page.
        let flash = set_cookies(&response)
            .into_iter()
            .find(|cookie| cookie.starts_with("flash="))
            .context("no flash cookie on the create-account response")?;
        let decoded = percent_decode_str(&flash).decode_utf8_lossy().into_owned();
        let path = crate::invite_path_re()
            .find(&decoded)
            .with_context(|| format!("no invite link in flash: {decoded}"))?;
        Ok(path.as_str().to_owned())
    }

    async fn redeem_invite(&self, invite_path: &str, password: &str) -> Result<()> {
        // Load the page first: the anonymous-session middleware mints the
        // session and readable CSRF cookie on that GET, and the
        // synchronizer-token guard wants the token echoed back on the POST. In
        // a browser csrf.js does this; here it is done by hand.
        let url = format!("{}{invite_path}", self.base_url);
        let page = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("loading {url}"))?;
        let cookies = set_cookies(&page);
        let csrf = cookie_value(&cookies, "csrf_token")
            .map(|value| percent_decode_str(&value).decode_utf8_lossy().into_owned())
            .unwrap_or_default();

        let response = self
            .client
            .post(&url)
            .header("Cookie", cookie_header(&cookies))
            .header("X-CSRF-Token", csrf)
            .form(&[("password", password), ("confirm_password", password)])
            .send()
            .await
            .with_context(|| format!("posting {url}"))?;
        if response.status() != StatusCode::SEE_OTHER {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("invite redemption failed ({status}): {body}");
        }
        Ok(())
    }
}

/// Every `Set-Cookie` on a response, as raw strings.
fn set_cookies(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| HeaderValue::to_str(value).ok())
        .map(str::to_owned)
        .collect()
}

/// Folds `Set-Cookie` values into the `Cookie` header to send back.
fn cookie_header(cookies: &[String]) -> String {
    cookies
        .iter()
        .filter_map(|cookie| cookie.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The value of one named cookie, still percent-encoded.
fn cookie_value(cookies: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    cookies
        .iter()
        .find(|cookie| cookie.starts_with(&prefix))
        .and_then(|cookie| cookie.split(';').next())
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_owned())
}
