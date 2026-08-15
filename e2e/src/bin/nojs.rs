//! Walks the whole app with scripting genuinely off, and fails the run on
//! anything a scriptless reader cannot do.
//!
//! The BDD suite asserts the paths we thought of. This drives a real browser
//! with script execution disabled *and* every `.js` request aborted, which is
//! what found that sign-out did not exist without JavaScript (#489) —
//! reachable only as a `fetch` DELETE, which no form can send.
//!
//! A gate rather than a report, matching how `csp-audit` is wired and
//! `touch-audit` is not: a job nobody has to read is a job nobody reads. Every
//! check below is a deliberate guarantee from the no-JS series (#480–#491), so
//! a failure here means one of them regressed, not that the page moved. The
//! findings are printed before the exit code is decided, so the log says which.
//!
//!   cd e2e && cargo run --bin nojs
//!
//! Two mechanisms replace Playwright's `javaScriptEnabled: false` plus
//! `context.route("**/*.js", abort)`:
//!
//! * `Emulation.setScriptExecutionDisabled` — what Playwright used underneath.
//! * The CDP `Fetch` domain, via [`Network`], for the aborts. Belt and braces:
//!   the walkthrough proves the pages work when the scripts are never
//!   *delivered*, which is stricter than merely not running them.

use std::collections::BTreeSet;

use anyhow::Result;
use rdrs_e2e::browser::{Browser, Scripting};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::network::{Action, Network};
use rdrs_e2e::seed::{NewEntry, Seed};
use rdrs_e2e::server::Harness;
use thirtyfour::components::SelectElement;

/// The account this walkthrough creates by claiming `/setup`.
const USER: &str = "walker";
const PASS: &str = "vulture-mango-77-quilt";

/// Aborts anything whose URL ends in `.js`, with or without a query string.
const SCRIPT_URLS: &str = r"\.js($|\?)";

/// Collected in order and printed as a block at the end, so one run says
/// everything that is wrong rather than stopping at the first thing.
#[derive(Default)]
struct Findings(Vec<String>);

impl Findings {
    fn note(&mut self, where_: &str, what: impl AsRef<str>) {
        self.0.push(format!("{where_}: {}", what.as_ref()));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let harness = Harness::start().await?;
    let endpoints = harness.endpoints().clone();
    let browser = Browser::open(Scripting::Disabled).await?;
    let network = Network::attach(browser.driver()).await?;
    network.route(SCRIPT_URLS, Action::Abort).await?;

    let mut findings = Findings::default();
    let result = walk(&browser, &network, &endpoints, &mut findings).await;
    browser.quit().await?;
    // A crash mid-walk is itself a finding, and the ones gathered before it
    // are still worth printing.
    if let Err(error) = result {
        findings.note("walkthrough", format!("aborted: {error:#}"));
    }

    println!("\n=== no-JS walkthrough findings ===");
    if findings.0.is_empty() {
        println!("(none)");
    }
    for finding in &findings.0 {
        println!("- {finding}");
    }
    println!("=== end ===\n");

    anyhow::ensure!(
        findings.0.is_empty(),
        "the app must stay usable with JavaScript disabled ({} finding(s))",
        findings.0.len()
    );
    Ok(())
}

async fn walk(
    browser: &Browser,
    network: &Network,
    endpoints: &rdrs_e2e::server::Endpoints,
    findings: &mut Findings,
) -> Result<()> {
    let driver = browser.driver();
    let base = &endpoints.base_url;
    let goto = async |path: &str| -> Result<()> {
        driver.goto(format!("{base}{path}")).await?;
        Ok(())
    };

    // ── 1. First-run setup ────────────────────────────────────────────────
    goto("/setup").await?;
    driver.fill_css("#username", USER).await?;
    driver.fill_css("#password", PASS).await?;
    if driver.css_opt("#confirm-password").await?.is_some() {
        driver.fill_css("#confirm-password", PASS).await?;
    }
    driver.submit_css(r#"form button[type="submit"]"#).await?;
    let url = driver.current_url().await?;
    if !url.as_str().contains("/login") {
        findings.note("setup", format!("expected to land on /login, got {url}"));
    }

    // ── 2. Sign in ────────────────────────────────────────────────────────
    driver.fill_css("#username", USER).await?;
    driver.fill_css("#password", PASS).await?;
    driver
        .submit_css(r#"form[action="/login"] button[type="submit"]"#)
        .await?;
    let url = driver.current_url().await?;
    if url.as_str().contains("/login") {
        findings.note(
            "login",
            format!(
                "still on /login after submitting: {}",
                driver.title().await?
            ),
        );
    }

    // ── 3. Navigation exists at all ───────────────────────────────────────
    let nav_links = driver.css_all("nav.nav-fallback a").await?;
    if driver.css_opt("nav.nav-fallback").await?.is_none() {
        findings.note("nav", "no scriptless navigation on the landing page");
    } else {
        let mut hrefs = BTreeSet::new();
        for link in nav_links {
            if let Some(href) = link.attr("href").await? {
                hrefs.insert(href);
            }
        }
        // Walk every destination and record anything that is not a 200.
        for href in hrefs {
            goto(&href).await?;
            match network.document_status(&href).await {
                Some(200) => {}
                other => findings.note("nav", format!("{href} returned {other:?}")),
            }
            if driver.css_opt("nav.nav-fallback").await?.is_none() {
                findings.note("nav", format!("{href} renders without the navigation"));
            }
        }
    }

    // ── 4. Subscribe to a feed ────────────────────────────────────────────
    goto("/feeds").await?;
    const ADD_FEED: &str = r#"form[action="/feeds"][method="post"]"#;
    if driver.css_opt(ADD_FEED).await?.is_none() {
        findings.note("feeds", "no add-feed form");
    } else {
        driver
            .fill_css(
                &format!(r#"{ADD_FEED} input[name="url"]"#),
                &format!("{}/feed.xml", endpoints.feed_url),
            )
            .await?;
        driver
            .submit_css(&format!(r#"{ADD_FEED} button[type="submit"]"#))
            .await?;
        if !driver.is_visible("flash-message").await? {
            findings.note("feeds", "adding a feed produced no visible confirmation");
        }
    }

    // ── 5. The filter bar ─────────────────────────────────────────────────
    goto("/feeds").await?;
    if driver.test_id_opt("feed-filter-apply").await?.is_none() {
        findings.note("feeds", "filter bar has no way to submit without scripting");
    } else {
        let sort = SelectElement::new(&driver.css("#sort-by").await?).await?;
        sort.select_by_value("unread").await?;
        driver.submit("feed-filter-apply").await?;
        let url = driver.current_url().await?;
        if !url.as_str().contains("sort=unread") {
            findings.note("feeds", format!("Apply did not carry the sort: {url}"));
        }
    }

    // ── 6. Categories: create, and see it ─────────────────────────────────
    goto("/categories").await?;
    const ADD_CATEGORY: &str = r#"form[action="/categories"][method="post"]"#;
    if driver.css_opt(ADD_CATEGORY).await?.is_none() {
        findings.note("categories", "no create form");
    } else {
        driver
            .fill_css(
                &format!(r#"{ADD_CATEGORY} input[name="name"]"#),
                "Walkthrough",
            )
            .await?;
        driver
            .submit_css(&format!(r#"{ADD_CATEGORY} button[type="submit"]"#))
            .await?;
        // The name comes back inside the rename form's input, not as text.
        let mut names = Vec::new();
        for input in driver
            .css_all(r#"form.cat-rename input[name="name"]"#)
            .await?
        {
            names.push(input.prop("value").await?.unwrap_or_default());
        }
        if !names.iter().any(|name| name == "Walkthrough") {
            findings.note(
                "categories",
                format!("the created category is not listed afterwards ({names:?})"),
            );
        }
        if !driver.is_visible("flash-message").await? {
            findings.note("categories", "creating a category produced no confirmation");
        }
    }

    // ── 7. Read an entry, star it, mark it unread ─────────────────────────
    // The mock feed carries no items, so seed real ones the way the BDD suite
    // does — otherwise the most important flow in a reader goes unwalked.
    {
        let seed = Seed::open(&endpoints.db_path).await?;
        let user_id = seed.user_id(USER).await?;
        let category_id = seed.create_category(user_id, "Walk").await?;
        let feed_id = seed
            .create_feed(
                category_id,
                "https://example.invalid/walk",
                Some("Walk Feed"),
            )
            .await?;
        let entries: Vec<_> = (0..3)
            .map(|i| {
                NewEntry::new(feed_id, &format!("walk-{i}"), &format!("Walk entry {i}"))
                    .link(format!("https://example.invalid/walk/{i}"))
                    .content(format!("<p>Body {i}</p>"))
            })
            .collect();
        seed.insert_entries(&entries).await?;
    }
    goto("/entries").await?;
    if driver.test_id_opt("entry-title-link").await?.is_none() {
        findings.note(
            "entries",
            "no entries to read (the mock feed may be empty) — skipped",
        );
    } else {
        driver.submit("entry-title-link").await?;
        let url = driver.current_url().await?;
        if !url.as_str().contains("entry=") {
            findings.note(
                "entries",
                format!("opening an entry did not land on a pane URL: {url}"),
            );
        }
        if driver.css_opt("#reading-pane").await?.is_none() {
            findings.note(
                "entries",
                "the reading pane did not render after opening an entry",
            );
        }

        if driver.test_id_opt("entry-star-action").await?.is_none() {
            findings.note("entries", "no star control");
        } else {
            driver.submit("entry-star-action").await?;
            if driver.css_opt(".entry-star.starred").await?.is_none() {
                findings.note("entries", "starring did not visibly take effect");
            }
        }

        if driver.test_id_opt("entry-read-toggle").await?.is_none() {
            findings.note("entries", "no read/unread toggle");
        } else {
            driver.submit("entry-read-toggle").await?;
        }
    }

    // ── 8. Search ─────────────────────────────────────────────────────────
    goto("/search").await?;
    // Scoped through `:has()` for the same reason the old spec used
    // `locator("form").filter({ has: input[name=q] })`: the signed-in shell
    // also renders the sign-out form, and a bare `form button[type=submit]`
    // picks *that* up — which signs the walkthrough out three steps early and
    // reports the rest of the app as missing.
    const SEARCH_FORM: &str = r#"form:has(input[name="q"])"#;
    const SEARCH_FIELD: &str = r#"form input[name="q"]"#;
    const SEARCH_SUBMIT: &str = r#"form:has(input[name="q"]) button[type="submit"]"#;
    if driver.css_opt(SEARCH_FORM).await?.is_none() {
        findings.note("search", "no search form");
    } else {
        driver.fill_css(SEARCH_FIELD, "test").await?;
        if driver.css_opt(SEARCH_SUBMIT).await?.is_none() {
            findings.note("search", "search form has no submit control");
        } else {
            driver.submit_css(SEARCH_SUBMIT).await?;
            let url = driver.current_url().await?;
            if !url.as_str().contains("q=test") {
                findings.note("search", format!("search did not navigate: {url}"));
            }
        }
    }

    // ── 9. Preferences ────────────────────────────────────────────────────
    goto("/user-settings").await?;
    const PREFS: &str = r#"form[action="/user-settings/preferences"]"#;
    if driver.css_opt(PREFS).await?.is_none() {
        findings.note("preferences", "no preferences form");
    } else if driver
        .css_opt(&format!(r#"{PREFS} button[type="submit"]"#))
        .await?
        .is_none()
    {
        findings.note("preferences", "preferences form has no submit control");
    }

    // ── 10. Sign out, and confirm the session is actually gone ────────────
    goto("/").await?;
    const LOGOUT: &str = r#"form[action="/logout"] button[type="submit"]"#;
    if driver.css_opt(LOGOUT).await?.is_none() {
        findings.note("logout", "no way to sign out");
    } else {
        driver.submit_css(LOGOUT).await?;
        let url = driver.current_url().await?;
        if !url.as_str().contains("/login") {
            findings.note("logout", format!("did not land on /login: {url}"));
        }
        // The session must really be over, not just redirected away from.
        goto("/").await?;
        let url = driver.current_url().await?;
        if !url.as_str().contains("/login") {
            findings.note("logout", "still signed in after signing out");
        }
    }

    Ok(())
}
