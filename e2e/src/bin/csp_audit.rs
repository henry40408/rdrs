//! CSP audit — walks the app in a real browser and fails on any Content
//! Security Policy violation.
//!
//! This exists because the Rust-side guard in
//! `src/middleware/security_headers.rs` is a *static* scan: it greps
//! `templates/` and `static/js/` for `style="`, `<style`, `on*=` handlers and
//! inline `<script>` bodies. That catches the authoring mistakes, but it is
//! blind to everything the policy actually governs at runtime — a stylesheet
//! `@import` to another origin, a webfont from a CDN, an `img-src` the markup
//! never mentions, markup assigned to `innerHTML` by a script, or a `<style>`
//! element built inside a shadow root. Only a browser enforcing the header can
//! see those.
//!
//!   cd e2e && cargo run --bin csp-audit

use std::time::Duration;

use anyhow::{Result, ensure};
use rdrs_e2e::api::{Api, PASSWORD};
use rdrs_e2e::browser::{Browser, Scripting};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::seed::Seed;
use rdrs_e2e::server::Harness;
use serde::Deserialize;
use thirtyfour::prelude::*;

/// Collected in the page and drained after every navigation.
///
/// `securitypolicyviolation` is preferred over scraping console text: it is
/// structured, stable across Chromium versions, and fires for attribute-level
/// violations (an inline `style=`) that carry no blocked URI to match on.
///
/// Injected through CDP's `addScriptToEvaluateOnNewDocument`, which is exempt
/// from the page's own CSP — so the collector cannot be silenced by the very
/// policy it is measuring.
const COLLECTOR: &str = r"
window.__cspViolations = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__cspViolations.push({
    directive: e.effectiveDirective || e.violatedDirective,
    blockedURI: e.blockedURI,
    sourceFile: e.sourceFile,
    line: e.lineNumber,
    sample: (e.sample || '').slice(0, 80),
  });
});
";

/// Drains the page-side buffer.
const DRAIN: &str = "const v = window.__cspViolations || []; \
                     window.__cspViolations = []; return v;";

/// Deferred module scripts and any swap fragment get this long to settle; a
/// violation raised by a script that has not executed yet would otherwise be
/// missed.
const SETTLE: Duration = Duration::from_millis(250);

/// One reported violation, plus where the walk was when it fired.
#[derive(Debug, Deserialize)]
struct Violation {
    #[serde(skip)]
    where_: String,
    directive: String,
    #[serde(rename = "blockedURI")]
    blocked_uri: Option<String>,
    #[serde(rename = "sourceFile")]
    source_file: Option<String>,
    line: Option<u32>,
    sample: Option<String>,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let blocked = self.blocked_uri.as_deref().unwrap_or("");
        let blocked = if blocked.is_empty() {
            "(inline)"
        } else {
            blocked
        };
        write!(
            f,
            "  [{}] {} blocked {blocked}",
            self.where_, self.directive
        )?;
        if let Some(source) = self.source_file.as_deref().filter(|s| !s.is_empty()) {
            write!(f, " from {source}:{}", self.line.unwrap_or(0))?;
        }
        if let Some(sample) = self.sample.as_deref().filter(|s| !s.is_empty()) {
            write!(f, " — sample: {sample}")?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let harness = Harness::start().await?;
    let endpoints = harness.endpoints().clone();
    let browser = Browser::open(Scripting::Enabled).await?;
    let driver = browser.driver();

    driver
        .cdp()
        .send_raw(
            "Page.addScriptToEvaluateOnNewDocument",
            serde_json::json!({ "source": COLLECTOR }),
        )
        .await?;

    let mut violations: Vec<Violation> = Vec::new();
    let result = audit(&browser, &endpoints, &mut violations).await;
    browser.quit().await?;
    result?;

    ensure!(
        violations.iter().all(|v| v.where_ == "positive control"),
        "CSP violations detected:\n{}",
        violations
            .iter()
            .filter(|v| v.where_ != "positive control")
            .map(Violation::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("csp-audit: no violations across the app");
    Ok(())
}

async fn audit(
    browser: &Browser,
    endpoints: &rdrs_e2e::server::Endpoints,
    violations: &mut Vec<Violation>,
) -> Result<()> {
    let driver = browser.driver();
    let base = &endpoints.base_url;

    // ---- seed an account with something to render ----
    let api = Api::new(base)?;
    let username = format!("csp-{}", rdrs_e2e::random_slug());
    api.setup_first_account(&username, PASSWORD).await?;

    let seed = Seed::open(&endpoints.db_path).await?;
    let user_id = seed.user_id(&username).await?;
    let category_id = seed.create_category(user_id, "Tech").await?;
    let feed_id = seed
        .create_feed(
            category_id,
            "https://example.com/feed.xml",
            Some("Example Feed"),
        )
        .await?;
    seed.seed_test_entries(feed_id, 12).await?;
    seed.configure_kagi(user_id, "e2e-test-token").await?;
    seed.make_admin(user_id).await?;

    // ---- logged-out surfaces ----
    for (label, path) in [("Login", "/login"), ("Setup", "/setup")] {
        visit(driver, base, label, path, violations).await?;
    }

    // ---- sign in ----
    driver.goto(format!("{base}/login")).await?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", PASSWORD).await?;
    driver.submit("login-submit").await?;
    drain(driver, "Login (submit)", violations).await?;

    // ---- logged-in surfaces ----
    let feed_edit = format!("/feeds/{feed_id}/edit");
    for (label, path) in [
        ("Unread", "/"),
        ("All entries", "/entries"),
        ("Starred", "/entries/starred"),
        ("Summarized", "/entries/summarized"),
        ("Feeds", "/feeds"),
        ("Feed edit", feed_edit.as_str()),
        ("Categories", "/categories"),
        ("Import OPML", "/feeds/import"),
        ("Search", "/search?q=test"),
        ("User settings", "/user-settings"),
        ("App settings", "/settings"),
        // The statistics bars carry their geometry as `pct-N` classes rather
        // than an inline `style` — this page is the reason that scale exists.
        ("Statistics", "/statistics"),
        ("Admin", "/admin"),
    ] {
        visit(driver, base, label, path, violations).await?;
    }

    // ---- runtime-injected markup, which the static scan cannot reach ----

    // The reading pane arrives as an HTML fragment swapped into the document by
    // script; its markup is parsed under the same policy as the page.
    driver.goto(format!("{base}/")).await?;
    let row = driver.test_id("entry-item").await?;
    row.find(By::Css("[data-testid=\"entry-title-link\"]"))
        .await?
        .click()
        .await?;
    rdrs_e2e::wait::eventually("the reading pane to fill", || async {
        Ok(driver
            .css_opt("#reading-pane:not(.reading-pane-empty)")
            .await?
            .is_some())
    })
    .await?;
    tokio::time::sleep(SETTLE).await;
    drain(driver, "Reading pane (swap fragment)", violations).await?;

    // The keyboard-help overlay builds a shadow root. A `<style>` element
    // inside a shadow tree is still markup and still policed, which is why
    // `rdrs-kb-help` uses a constructable stylesheet instead.
    driver.press("?").await?;
    driver.expect_visible("kb-help").await?;
    tokio::time::sleep(SETTLE).await;
    drain(driver, "Keyboard help overlay (shadow DOM)", violations).await?;
    driver.press_focused("Escape").await?;

    // The off-canvas sidebar is only reachable at a narrow viewport. The
    // resize happens on this session rather than in a second browser, which
    // would lose the sign-in.
    resize(driver, 375, 667).await?;
    driver.goto(format!("{base}/")).await?;
    driver.click_css(".sidebar-toggle").await?;
    tokio::time::sleep(SETTLE).await;
    drain(driver, "Sidebar drawer (mobile)", violations).await?;
    resize(driver, 1280, 800).await?;

    // ---- positive control ----
    // An audit that reports zero findings is worthless unless the collector is
    // known to have been live. Plant a violation the policy must reject and
    // require it to surface; if this fails, every clean result above is
    // meaningless rather than reassuring.
    driver
        .eval(
            r#"document.body.insertAdjacentHTML(
                 'beforeend',
                 '<div id="csp-control" style="color:red"></div>',
               ); return true;"#,
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = violations.len();
    drain(driver, "positive control", violations).await?;
    let control = &violations[before..];
    ensure!(
        !control.is_empty(),
        "positive control: a planted inline style must be blocked — the \
         collector was not live, so every clean result above is meaningless"
    );
    ensure!(
        control[0].directive.contains("style-src"),
        "positive control was blocked by {}, expected style-src",
        control[0].directive
    );
    Ok(())
}

/// Navigation waits on the document being ready, never on the network going
/// idle: every logged-in page holds an open SSE stream, so the network never
/// goes idle and waiting for it would time out on every single page.
async fn visit(
    driver: &WebDriver,
    base: &str,
    label: &str,
    path: &str,
    violations: &mut Vec<Violation>,
) -> Result<()> {
    driver.goto(format!("{base}{path}")).await?;
    tokio::time::sleep(SETTLE).await;
    drain(driver, label, violations).await
}

/// Moves what the page collected into the run-wide list.
///
/// Must be called after every navigation: the collector is re-injected per
/// document, so the buffer starts empty again.
async fn drain(driver: &WebDriver, where_: &str, violations: &mut Vec<Violation>) -> Result<()> {
    let found = driver.eval(DRAIN).await?;
    for value in found.as_array().cloned().unwrap_or_default() {
        let mut violation: Violation = serde_json::from_value(value)?;
        where_.clone_into(&mut violation.where_);
        violations.push(violation);
    }
    Ok(())
}

async fn resize(driver: &WebDriver, width: u32, height: u32) -> Result<()> {
    driver
        .cdp()
        .send_raw(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
        )
        .await?;
    Ok(())
}
