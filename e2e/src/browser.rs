//! The browser session and its CDP emulations.
//!
//! `WebDriver::managed` downloads only the driver, never the browser, so a local
//! Chrome/Chromium is required; [`Browser::open`] says so explicitly. Emulations
//! use CDP rather than `BiDi` to avoid an extra feature and WebSocket stack.

use std::time::Duration;

use anyhow::{Context, Result};
use thirtyfour::prelude::*;

/// How long a query waits before giving up. Generous because slow CI runners
/// took over 10 s to land a navigation; only a real failure pays it in full.
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

/// The default desktop viewport.
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
    /// Fails when no local browser is installed, when the driver cannot be
    /// downloaded, or when the session cannot be created.
    pub async fn open(scripting: Scripting) -> Result<Self> {
        let mut caps = DesiredCapabilities::chrome();
        caps.add_arg("--headless=new")?;
        // Headless Linux reports `hover: none`, which triggers the touch layout;
        // this can't be fixed post-launch (`setEmulatedMedia` ignores hover/
        // pointer). Values are Blink enums: kHoverHoverType=2, kPointerFine=4.
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
        // Linux classic scrollbars would take 15px off the viewport (macOS
        // overlays take none), breaking layout assertions on CI only.
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
        // `--window-size` sizes the window, not the viewport; landing even
        // slightly under 1024px would trip the touch layout.
        browser.set_viewport(DESKTOP).await?;
        if scripting == Scripting::Disabled {
            browser.disable_scripting().await?;
        }
        Ok(browser)
    }

    /// Downloads the driver once up front, so parallel sessions on a cold
    /// cache (every CI run) don't all download it and contend on its lock file.
    pub async fn prepare() -> Result<()> {
        Self::open(Scripting::Enabled).await?.quit().await
    }

    /// The underlying session.
    pub fn driver(&self) -> &WebDriver {
        &self.driver
    }

    /// The viewport the session is currently emulating.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Resizes the viewport exactly via CDP; `WebDriver` window sizes include
    /// chrome, which would miss exact breakpoints.
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

    /// Emulates a touch device, the only way to flip `(hover: none)` /
    /// `(pointer: coarse)`; `setEmulatedMedia` ignores both.
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

    /// Emulates `prefers-color-scheme` (the app's system-follow path).
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

    /// Takes the browser offline. Unlike [`crate::network::Action::Abort`]
    /// (page-target only), this also reaches the service worker.
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

    /// Grants clipboard access; headless `clipboard.writeText` rejects without it.
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

    /// Does the element overlap the viewport at all? (`WebElement::rect` is in
    /// document coordinates.) Works under `@nojs` too.
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
    pub async fn quit(self) -> Result<()> {
        self.driver.quit().await?;
        Ok(())
    }

    /// Stops the page's scripts from the *next* document on, hence it runs
    /// before the first navigation and sessions are per-scenario.
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
