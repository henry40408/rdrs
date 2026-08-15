//! The browser session, and the emulations the suite depends on.
//!
//! `WebDriver::managed` downloads and supervises a matching chromedriver
//! itself, so nothing has to be installed alongside the tests — but it does
//! *not* download the browser, unlike the Playwright setup this replaces. A
//! Chrome or Chromium in one of the well-known locations is a prerequisite now;
//! [`Browser::open`] says so in as many words when it is missing, because the
//! raw driver error does not.
//!
//! Every emulation goes through CDP rather than `BiDi`.
//! `Emulation.setEmulatedMedia` is the only way to reach `prefers-color-scheme`
//! at all — `BiDi` has no equivalent. `Emulation.setScriptExecutionDisabled` is
//! a choice: `BiDi`'s `emulation.setScriptingEnabled` would also work, but it
//! would pull in the non-default `bidi` feature and a WebSocket stack for
//! something CDP already does over the connection we have. It is also what
//! Playwright's `javaScriptEnabled: false` did underneath, so the `@nojs`
//! scenarios run against the same mechanism as before.

use std::time::Duration;

use anyhow::{Context, Result};
use thirtyfour::prelude::*;

/// How long a query waits for a condition before giving up.
///
/// Only ever paid in full by a genuine failure, so it is set for the slowest
/// machine that runs this rather than the fastest: locally every wait settles
/// in well under a second, while a two-core CI runner driving several browsers
/// took longer than 10 s to land a navigation.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a query re-checks while waiting.
pub const WAIT_INTERVAL: Duration = Duration::from_millis(100);

/// A viewport, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// The default viewport, matching the `Desktop Chrome` device the Playwright
/// projects used.
pub const DESKTOP: Viewport = Viewport::new(1280, 720);

/// Whether the page's own scripts run — the `e2e` / `e2e-nojs` split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scripting {
    /// The scripted path: the modules under `static/js/` run.
    Enabled,
    /// The `@nojs` path: the page's own scripts never execute.
    Disabled,
}

/// A browser session, scoped to one scenario.
#[derive(Debug)]
pub struct Browser {
    driver: WebDriver,
    viewport: Viewport,
}

impl Browser {
    /// Starts a headless session with the page's scripts on or off.
    ///
    /// # Errors
    ///
    /// Fails when no local browser is installed, when the driver cannot be
    /// downloaded, or when the session cannot be created.
    pub async fn open(scripting: Scripting) -> Result<Self> {
        let mut caps = DesiredCapabilities::chrome();
        caps.set_headless()?;
        caps.add_arg(&format!(
            "--window-size={},{}",
            DESKTOP.width, DESKTOP.height
        ))?;
        // Containers get a 64 MB /dev/shm by default, which Chrome outgrows.
        caps.add_arg("--disable-dev-shm-usage")?;

        let driver = WebDriver::managed(caps).await.context(
            "could not start a browser session — a local Chrome or Chromium is required \
             (`brew install --cask ungoogled-chromium`, or `google-chrome` on CI); \
             unlike Playwright, the driver manager downloads only the driver",
        )?;

        let browser = Self {
            driver,
            viewport: DESKTOP,
        };
        if scripting == Scripting::Disabled {
            browser.disable_scripting().await?;
        }
        Ok(browser)
    }

    /// Downloads and starts the driver once, before any scenario asks for it.
    ///
    /// `WebDriver::managed` builds a *new* manager per call, so each session
    /// prepares the driver for itself. That is harmless when it is already
    /// cached and pathological when it is not: several sessions opening at once
    /// on a cold cache all try to download the same driver and contend on its
    /// lock file, which is a stall, not a slowdown. CI has a cold cache every
    /// run, which is exactly where the scenarios run in parallel.
    ///
    /// One session opened and closed up front settles it — the download happens
    /// once, and every later session finds the driver in place.
    ///
    /// # Errors
    ///
    /// Fails for the same reasons [`Browser::open`] does.
    pub async fn prepare() -> Result<()> {
        Self::open(Scripting::Enabled).await?.quit().await
    }

    /// The underlying session, for the page objects.
    pub fn driver(&self) -> &WebDriver {
        &self.driver
    }

    /// The viewport the session is currently emulating.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Resizes the viewport, Playwright's `setViewportSize`.
    ///
    /// Goes through `Emulation.setDeviceMetricsOverride` rather than the
    /// WebDriver window commands: a headless window's outer size includes
    /// chrome the layout does not see, so setting 375×667 that way lands a
    /// viewport of some other width — and the responsive scenarios assert on
    /// exact breakpoints.
    ///
    /// # Errors
    ///
    /// Fails when the CDP command is refused.
    pub async fn set_viewport(&mut self, viewport: Viewport) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setDeviceMetricsOverride",
                serde_json::json!({
                    "width": viewport.width,
                    "height": viewport.height,
                    "deviceScaleFactor": 1,
                    "mobile": false,
                }),
            )
            .await?;
        self.viewport = viewport;
        Ok(())
    }

    /// Emulates a touch-capable device, Playwright's `hasTouch`.
    ///
    /// The pointer-coarse scenarios need this: the app branches on
    /// `(hover: none)` / `(pointer: coarse)`, which a viewport size alone does
    /// not change.
    ///
    /// # Errors
    ///
    /// Fails when either CDP command is refused.
    pub async fn set_touch(&self, enabled: bool) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setTouchEmulationEnabled",
                serde_json::json!({ "enabled": enabled, "maxTouchPoints": 5 }),
            )
            .await?;
        // The media features are a separate override; the touch emulation above
        // changes event dispatch but not what `@media (pointer: coarse)` sees.
        let features = if enabled {
            serde_json::json!([
                { "name": "pointer", "value": "coarse" },
                { "name": "any-pointer", "value": "coarse" },
                { "name": "hover", "value": "none" },
                { "name": "any-hover", "value": "none" },
            ])
        } else {
            serde_json::json!([])
        };
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setEmulatedMedia",
                serde_json::json!({ "media": "screen", "features": features }),
            )
            .await?;
        Ok(())
    }

    /// Emulates `prefers-color-scheme`, with no stored preference — the app's
    /// system-follow path, which is what the screenshots are meant to show.
    ///
    /// # Errors
    ///
    /// Fails when the CDP command is refused.
    pub async fn emulate_color_scheme(&self, scheme: &str) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setEmulatedMedia",
                serde_json::json!({
                    "media": "screen",
                    "features": [{ "name": "prefers-color-scheme", "value": scheme }],
                }),
            )
            .await?;
        Ok(())
    }

    /// Is the element intersecting the viewport?
    ///
    /// Rebuilds Playwright's `toBeInViewport`, whose default ratio is "any
    /// overlap at all". `WebElement::rect` reports document coordinates, so it
    /// cannot answer this on its own once the page has scrolled.
    ///
    /// The driver can still inject script into a page whose *own* scripts are
    /// disabled — `Emulation.setScriptExecutionDisabled` stops the document's
    /// scripts, not `Execute Script` — so this works in the `@nojs` scenarios
    /// too.
    ///
    /// # Errors
    ///
    /// Fails when the script cannot run.
    pub async fn is_in_viewport(&self, element: &WebElement) -> Result<bool> {
        let visible = self
            .driver
            .execute(
                r"
                const el = arguments[0];
                const r = el.getBoundingClientRect();
                return r.bottom > 0 && r.right > 0
                    && r.top < window.innerHeight && r.left < window.innerWidth;
                ",
                vec![element.to_json()?],
            )
            .await?
            .json()
            .as_bool()
            .context("viewport probe did not return a boolean")?;
        Ok(visible)
    }

    /// Ends the session.
    ///
    /// # Errors
    ///
    /// Fails when the driver refuses to close.
    pub async fn quit(self) -> Result<()> {
        self.driver.quit().await?;
        Ok(())
    }

    /// Stops the page's own scripts from running.
    ///
    /// Takes effect on the *next* document, so it is issued before the first
    /// navigation — which is why sessions are per-scenario rather than shared.
    async fn disable_scripting(&self) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setScriptExecutionDisabled",
                serde_json::json!({ "value": true }),
            )
            .await?;
        Ok(())
    }
}
