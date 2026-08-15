//! Touch-target audit at iPhone-SE width (375px).
//!
//! Walks every interactive element on the main pages, records rendered
//! bounding boxes, and reports anything under 44px in either axis.
//!
//!   cd e2e && cargo run --bin touch-audit
//!
//! A **report, not a gate** — unlike `csp-audit` and `nojs`. Inline text links
//! are a legitimate exemption and the remaining findings need judgement, so
//! this prints and exits 0; the JSON report next to it is what a follow-up
//! reads.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rdrs_e2e::api::{Api, PASSWORD};
use rdrs_e2e::browser::{Browser, Scripting, Viewport};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::seed::Seed;
use rdrs_e2e::server::{Endpoints, Harness};
use serde::{Deserialize, Serialize};
use thirtyfour::prelude::*;

/// The smallest comfortable tap target, in CSS pixels.
const MIN: u32 = 44;

/// iPhone SE, the narrowest layout the app supports.
const VIEWPORT: Viewport = Viewport::new(375, 667);

/// Measures every interactive element and reports the ones below `MIN`.
///
/// Kept as page script rather than driven element-by-element from here: the
/// audit reads a computed style and a rect for every control on the page, and
/// a round trip each would take minutes.
const MEASURE: &str = r#"
const MIN = arguments[0];
const openSidebar = arguments[1];
const done = arguments[arguments.length - 1];
(async () => {
  if (openSidebar) {
    document.querySelector('.sidebar-toggle')?.click();
    await new Promise((r) => setTimeout(r, 250));
  }
  const sel =
    'button, a[href], select, input:not([type=hidden]), textarea, ' +
    'label:has(input,select,textarea), [role="button"], summary';
  const out = [];
  for (const el of document.querySelectorAll(sel)) {
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') continue;
    if (r.width === 0 || r.height === 0) continue;
    if (r.width >= MIN && r.height >= MIN) continue;
    const label =
      el.getAttribute('data-testid') ||
      el.getAttribute('aria-label') ||
      (el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 32);
    // A checkbox or radio inside a label: the label IS the tap target.
    const type = el.getAttribute('type') || '';
    const labelWrapped =
      (type === 'checkbox' || type === 'radio') && !!el.closest('label');
    out.push({
      tag: el.tagName.toLowerCase(),
      type,
      cls: (el.className?.toString() || '').slice(0, 44),
      label,
      w: Math.round(r.width),
      h: Math.round(r.height),
      // Inline text links are an accepted exemption; `.entry-item-title` is the
      // title link, whose >= 44px target is the whole row (the row is
      // click-delegated to it via installRowClickToOpen).
      inlineText:
        !!el.closest('p, .entry-item-title, .entry-item-meta, .reading-pane-article, .breadcrumb') ||
        labelWrapped,
    });
  }
  done(out);
})();
"#;

#[derive(Debug, Serialize, Deserialize)]
struct Finding {
    tag: String,
    #[serde(rename = "type")]
    kind: String,
    cls: String,
    label: String,
    w: u32,
    h: u32,
    #[serde(rename = "inlineText")]
    inline_text: bool,
}

#[derive(Debug, Serialize)]
struct PageReport {
    page: String,
    url: String,
    findings: Vec<Finding>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let harness = Harness::start().await?;
    let endpoints = harness.endpoints().clone();
    let mut browser = Browser::open(Scripting::Enabled).await?;

    let report = audit(&mut browser, &endpoints).await;
    browser.quit().await?;
    let report = report?;

    let out = report_path();
    std::fs::write(&out, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", out.display()))?;

    let mut total = 0;
    let mut real_gaps = 0;
    for page in &report {
        if page.findings.is_empty() {
            println!("\n## {} ({}) — OK (all >= {MIN}px)", page.page, page.url);
            continue;
        }
        println!(
            "\n## {} ({}) — {} sub-{MIN}px",
            page.page,
            page.url,
            page.findings.len()
        );
        for finding in &page.findings {
            total += 1;
            if !finding.inline_text {
                real_gaps += 1;
            }
            let kind = if finding.kind.is_empty() {
                String::new()
            } else {
                format!(" type={}", finding.kind)
            };
            let exempt = if finding.inline_text {
                "  (inline-text exempt)"
            } else {
                ""
            };
            println!(
                "  {:>3}x{:>3}  <{}{kind}> {}  [.{}]{exempt}",
                finding.w, finding.h, finding.tag, finding.label, finding.cls
            );
        }
    }
    println!(
        "\n=== {total} sub-{MIN}px hits; {real_gaps} non-inline (candidate gaps). \
         Report: {} ===",
        out.display()
    );
    Ok(())
}

fn report_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("touch-audit-report.json")
}

async fn audit(browser: &mut Browser, endpoints: &Endpoints) -> Result<Vec<PageReport>> {
    let base = endpoints.base_url.clone();

    // ---- seed a realistic account ----
    let api = Api::new(&base)?;
    let username = format!("touch-{}", rdrs_e2e::random_slug());
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

    // ---- sign in, then narrow the viewport ----
    {
        let driver = browser.driver();
        driver.goto(format!("{base}/login")).await?;
        driver.fill("username-input", &username).await?;
        driver.fill("password-input", PASSWORD).await?;
        driver.submit("login-submit").await?;
    }
    browser.set_viewport(VIEWPORT).await?;

    let feed_edit = format!("/feeds/{feed_id}/edit");
    let pages = [
        ("Unread", "/"),
        ("All Entries", "/entries"),
        ("Starred", "/entries/starred"),
        ("Summarized", "/entries/summarized"),
        ("Feeds", "/feeds"),
        ("Feed edit", feed_edit.as_str()),
        ("Categories", "/categories"),
        ("Import OPML", "/feeds/import"),
        ("Search", "/search?q=test"),
        ("User settings", "/user-settings"),
        ("App settings", "/settings"),
        ("Statistics", "/statistics"),
        ("Admin", "/admin"),
    ];

    let driver = browser.driver();
    let mut report = Vec::new();
    for (index, (name, url)) in pages.iter().enumerate() {
        driver.goto(format!("{base}{url}")).await?;
        report.push(PageReport {
            page: (*name).to_owned(),
            url: (*url).to_owned(),
            findings: measure(driver, false).await?,
        });
        // The off-canvas drawer is measured once, on the first page.
        if index == 0 {
            report.push(PageReport {
                page: "Sidebar drawer".to_owned(),
                url: (*url).to_owned(),
                findings: measure(driver, true).await?,
            });
        }
    }
    Ok(report)
}

async fn measure(driver: &WebDriver, open_sidebar: bool) -> Result<Vec<Finding>> {
    // `execute_async`, because the sidebar branch has to await its animation
    // before measuring — a synchronous script would read the drawer mid-slide.
    let value = driver
        .execute_async(
            MEASURE,
            vec![serde_json::json!(MIN), serde_json::json!(open_sidebar)],
        )
        .await?;
    Ok(serde_json::from_value(value.json().clone())?)
}
