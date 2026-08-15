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
    /// The emulated media, tracked because `Emulation.setEmulatedMedia`
    /// replaces the *whole* feature list on every call — setting a colour
    /// scheme would otherwise silently drop the pointer emulation, and vice
    /// versa.
    media: Media,
}

/// The media features this suite emulates.
#[derive(Debug, Default, Clone)]
struct Media {
    /// `None` follows the browser's own setting; the screenshots pin it.
    color_scheme: Option<String>,
    /// Coarse pointer and no hover — the touch-device branch.
    touch: bool,
}

impl Media {
    /// The `features` array for `Emulation.setEmulatedMedia`.
    ///
    /// The pointer features are always stated, never left to the browser:
    /// headless Chrome has no real pointing device and reports
    /// `pointer: coarse` / `hover: none` unless told otherwise, which silently
    /// puts every desktop scenario on the touch-target branch of the
    /// stylesheet — a 44px minimum where the design says 35.
    fn features(&self) -> serde_json::Value {
        let (pointer, hover) = if self.touch {
            ("coarse", "none")
        } else {
            ("fine", "hover")
        };
        let mut features = vec![
            serde_json::json!({ "name": "pointer", "value": pointer }),
            serde_json::json!({ "name": "any-pointer", "value": pointer }),
            serde_json::json!({ "name": "hover", "value": hover }),
            serde_json::json!({ "name": "any-hover", "value": hover }),
        ];
        if let Some(scheme) = &self.color_scheme {
            features.push(serde_json::json!({
                "name": "prefers-color-scheme",
                "value": scheme,
            }));
        }
        serde_json::Value::Array(features)
    }
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
        // Playwright launches chromium with this, and the layout assertions
        // were written against it. Without it the classic scrollbars on Linux
        // take 15px out of the viewport, so a pane asked to be 375px wide
        // measures 360 — while macOS's overlay scrollbars take none, which
        // makes the difference invisible locally and a CI-only failure.
        caps.add_arg("--hide-scrollbars")?;

        let driver = WebDriver::managed(caps).await.context(
            "could not start a browser session — a local Chrome or Chromium is required \
             (`brew install --cask ungoogled-chromium`, or `google-chrome` on CI); \
             unlike Playwright, the driver manager downloads only the driver",
        )?;

        let browser = Self {
            driver,
            viewport: DESKTOP,
            media: Media::default(),
        };
        // Stated up front rather than left to the browser — see
        // `Media::features` for what headless Chrome reports otherwise.
        browser.apply_media().await?;
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
    /// `WebDriver` window commands: a headless window's outer size includes
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
    pub async fn set_touch(&mut self, enabled: bool) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setTouchEmulationEnabled",
                serde_json::json!({ "enabled": enabled, "maxTouchPoints": 5 }),
            )
            .await?;
        // The media features are a separate override: the touch emulation
        // above changes event dispatch but not what `@media (pointer: coarse)`
        // sees.
        self.media.touch = enabled;
        self.apply_media().await
    }

    /// Emulates `prefers-color-scheme`, with no stored preference — the app's
    /// system-follow path, which is what the screenshots are meant to show.
    ///
    /// # Errors
    ///
    /// Fails when the CDP command is refused.
    pub async fn emulate_color_scheme(&mut self, scheme: &str) -> Result<()> {
        self.media.color_scheme = Some(scheme.to_owned());
        self.apply_media().await
    }

    /// Sends the whole emulated-media set, since CDP replaces rather than
    /// merges it.
    async fn apply_media(&self) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Emulation.setEmulatedMedia",
                serde_json::json!({ "media": "screen", "features": self.media.features() }),
            )
            .await?;
        Ok(())
    }

    /// Grants clipboard access, Playwright's
    /// `grantPermissions(["clipboard-read", "clipboard-write"])`.
    ///
    /// Without it `navigator.clipboard.writeText` rejects in a headless
    /// browser, and the copy button never reaches its "Copied" state.
    ///
    /// # Errors
    ///
    /// Fails when the CDP command is refused.
    pub async fn grant_clipboard(&self) -> Result<()> {
        self.driver
            .cdp()
            .send_raw(
                "Browser.grantPermissions",
                serde_json::json!({
                    "permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"],
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
