//! The browser session, and the emulations the suite depends on.
//!
//! `WebDriver::managed` downloads and supervises a matching chromedriver itself,
//! but — unlike the Playwright setup this replaces — it does *not* download the
//! browser. A local Chrome or Chromium is a prerequisite now, and
//! [`Browser::open`] says so in as many words, because the raw driver error does
//! not.
//!
//! Every emulation goes through CDP rather than `BiDi`.
//! `Emulation.setEmulatedMedia` is the only way to reach `prefers-color-scheme`
//! at all. `Emulation.setScriptExecutionDisabled` is a choice: `BiDi`'s
//! equivalent would pull in a non-default feature and a WebSocket stack for
//! something CDP already does over the connection we have, and CDP is what
//! Playwright's `javaScriptEnabled: false` used underneath.

use std::time::Duration;

use anyhow::{Context, Result};
use thirtyfour::prelude::*;

/// How long a query waits for a condition before giving up.
///
/// Only ever paid in full by a genuine failure, so it is set for the slowest
/// machine that runs this: locally every wait settles in well under a second,
/// while a two-core CI runner driving several browsers took longer than 10 s to
/// land a navigation.
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
        caps.add_arg("--headless=new")?;
        // Pins the hover and pointer *types* the way Playwright does, because
        // nothing else can. A headless browser has no pointing device, so on a
        // Linux runner it reports `hover: none` — and the stylesheet's touch
        // baseline hangs off that, laying every desktop scenario out on the
        // 44px-tap-target branch. macOS reports `hover: hover` regardless, which
        // is why this was invisible locally.
        //
        // It cannot be corrected after the session starts:
        // `Emulation.setEmulatedMedia` silently ignores `hover` and `pointer`
        // (measured), and the only command that moves them,
        // `setTouchEmulationEnabled`, goes the other way.
        //
        // The values are Blink's own enums: `kHoverHoverType = 2`,
        // `kPointerFine = 4`.
        caps.add_arg(
            "--blink-settings=primaryHoverType=2,availableHoverTypes=2,\
             primaryPointerType=4,availablePointerTypes=4",
        )?;
        caps.add_arg(&format!(
            "--window-size={},{}",
            DESKTOP.width, DESKTOP.height
        ))?;
        // Containers get a 64 MB /dev/shm by default, which Chrome outgrows.
        caps.add_arg("--disable-dev-shm-usage")?;
        // Playwright launches chromium with this and the layout assertions were
        // written against it. Without it the classic scrollbars on Linux take
        // 15px out of the viewport, while macOS's overlay scrollbars take none —
        // invisible locally, a CI-only failure.
        caps.add_arg("--hide-scrollbars")?;

        let driver = WebDriver::managed(caps).await.context(
            "could not start a browser session — a local Chrome or Chromium is required \
             (`brew install --cask ungoogled-chromium`, or `google-chrome` on CI); \
             unlike Playwright, the driver manager downloads only the driver",
        )?;

        let mut browser = Self {
            driver,
            viewport: DESKTOP,
        };
        // `--window-size` above sizes the *window*; the stylesheet reads the
        // viewport, and the two differ by whatever chrome the platform's headless
        // build keeps. The touch baseline also lives under
        // `@media (max-width: 1024px)`, so a desktop scenario landing even
        // slightly under 1024 would be laid out as a phone.
        browser.set_viewport(DESKTOP).await?;
        if scripting == Scripting::Disabled {
            browser.disable_scripting().await?;
        }
        Ok(browser)
    }

    /// Downloads and starts the driver once, before any scenario asks for it.
    ///
    /// `WebDriver::managed` builds a *new* manager per call, which is harmless
    /// when the driver is cached and pathological when it is not: several
    /// sessions opening at once on a cold cache all download the same driver and
    /// contend on its lock file. CI has a cold cache every run, which is exactly
    /// where the scenarios run in parallel.
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
    /// chrome the layout does not see, and the responsive scenarios assert on
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
    /// not change. This is the *only* command that moves them —
    /// `Emulation.setEmulatedMedia` silently ignores both (measured) — which also
    /// means a desktop session cannot opt *into* `hover: hover` and has to start
    /// there, as `--headless=new` arranges.
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

    /// Cuts the browser off from the network — the "Offline" checkbox in the
    /// browser's own developer tools.
    ///
    /// Not [`crate::network::Action::Abort`], which is what the no-JS
    /// walkthrough uses: CDP request interception is attached to the *page*
    /// target, and the requests that have to fail here are issued by the service
    /// worker, which is a target of its own. Network conditions apply to the
    /// whole browser context and so reach both.
    ///
    /// # Errors
    ///
    /// Fails when either CDP command is refused.
    pub async fn set_offline(&self, offline: bool) -> Result<()> {
        // `emulateNetworkConditions` is a no-op until the domain is enabled.
        self.driver
            .cdp()
            .send_raw("Network.enable", serde_json::json!({}))
            .await?;
        self.driver
            .cdp()
            .send_raw(
                "Network.emulateNetworkConditions",
                serde_json::json!({
                    "offline": offline,
                    "latency": 0,
                    // -1 disables throttling; only the offline flag matters here.
                    "downloadThroughput": -1,
                    "uploadThroughput": -1,
                }),
            )
            .await?;
        Ok(())
    }

    /// Grants clipboard access. Without it `navigator.clipboard.writeText`
    /// rejects in a headless browser and the copy button never reaches its
    /// "Copied" state.
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
    /// cannot answer this once the page has scrolled.
    ///
    /// The driver can still inject script into a page whose *own* scripts are
    /// disabled, so this works in the `@nojs` scenarios too.
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
