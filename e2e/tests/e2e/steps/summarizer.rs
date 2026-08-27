//! The standalone Summarizer page — a port of `summarizer.steps.js`.

use anyhow::{Result, ensure};
use cucumber::gherkin::Step;
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, TextContent, Within, click_when_ready};
use rdrs_e2e::first_column;
use rdrs_e2e::wait::{despite_swaps, eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;

const CARD: &str = "[data-summarizer-card]";

#[when("I open the Summarizer")]
async fn open_summarizer(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/summarizer").await
}

#[when("I enter these URLs:")]
async fn enter_urls(world: &mut RdrsWorld, step: &Step) -> Result<()> {
    let urls = first_column(step)?.join("\n");
    world.driver()?.fill("summarizer-input", &urls).await
}

#[when("I submit the summarizer form")]
async fn submit_summarizer(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    despite_swaps("submitting the summarizer form", || async {
        let form = driver.test_id("summarizer-form").await?;
        let button = form
            .button_named("Summarize")
            .await?
            .ok_or_else(|| anyhow::anyhow!("the summarizer form has no Summarize button"))?;
        click_when_ready(&button).await
    })
    .await
}

#[then(expr = "I should see {int} summary cards")]
async fn see_cards(world: &mut RdrsWorld, count: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the number of summary cards", count, || async {
        Ok(driver.css_all(CARD).await?.len())
    })
    .await
}

/// Cards run one at a time (a client-side serial queue in `summarizer.js`) and
/// the mock Kagi upstream adds a delay per request, so each card is waited on
/// in turn rather than asserted immediately.
#[then(expr = "each card resolves to a completed state containing {string}")]
async fn cards_complete(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    let count = driver.css_all(CARD).await?.len();
    for index in 0..count {
        let selector = format!("{CARD}:nth-of-type({})", index + 1);
        driver
            .expect_attr(&selector, "data-state", Some("completed"))
            .await?;
        let body = format!("{selector} [data-sz-body]");
        eventually(&format!("card {index} contains {text:?}"), || async {
            let Some(element) = driver.css_opt(&body).await? else {
                return Ok(false);
            };
            Ok(element.content_text().await?.contains(&text))
        })
        .await?;
    }
    Ok(())
}

#[when("I copy the first summary card")]
async fn copy_first_card(world: &mut RdrsWorld) -> Result<()> {
    // The label swap only needs the clipboard write to resolve; granting the
    // permission keeps `navigator.clipboard.writeText` from rejecting in a
    // headless browser.
    world.browser()?.grant_clipboard().await?;
    let driver = world.driver()?;
    despite_swaps("copying the first summary card", || async {
        let card = driver.css(CARD).await?;
        let button = card
            .button_named("Copy summary")
            .await?
            .ok_or_else(|| anyhow::anyhow!("the card has no copy button"))?;
        click_when_ready(&button).await
    })
    .await
}

#[then(expr = "the first summary card's copy button reads {string}")]
async fn copy_button_reads(world: &mut RdrsWorld, text: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the copy button's label", text, || async {
        driver
            .css(&format!("{CARD} [data-sz-copy]"))
            .await?
            .content_text()
            .await
    })
    .await
}

/// Scoped to the page content: the sidebar always carries its own link to
/// `/user-settings`, so a bare `href` selector matches that too.
#[then("I should see a link to Settings")]
async fn see_settings_link(world: &mut RdrsWorld) -> Result<()> {
    let link = world
        .driver()?
        .css(r#".page-content a[href="/user-settings"]"#)
        .await?;
    ensure!(
        link.is_displayed().await?,
        "the Settings link is present but hidden"
    );
    Ok(())
}

#[then("I should not see the summarizer form")]
async fn no_summarizer_form(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("summarizer-form").await
}
