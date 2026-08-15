//! Full-text search and the highlight layout — a port of `search.steps.js`.

use anyhow::{Result, ensure};
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use rdrs_e2e::browser::{Viewport, WAIT_INTERVAL, WAIT_TIMEOUT};
use rdrs_e2e::dom::{Dom, TextContent};
use rdrs_e2e::first_column;
use rdrs_e2e::seed::NewEntry;
use rdrs_e2e::wait::eventually_eq;
use rdrs_e2e::world::RdrsWorld;
use thirtyfour::prelude::*;

#[given("I have a feed with entries titled:")]
async fn feed_with_titles(world: &mut RdrsWorld, step: &Step) -> Result<()> {
    let titles = first_column(step)?;
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, "Search Category").await?;
    let feed_id = seed
        .create_feed(
            category_id,
            &format!("https://example.com/{username}-feed.xml"),
            Some("Search Feed"),
        )
        .await?;

    let entries: Vec<_> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| {
            NewEntry::new(feed_id, &format!("{username}-{i}"), title)
                .link(format!("https://example.com/{username}/{i}"))
                .content(format!("<p>{title}</p>"))
                .published_offset(format!("-{} hours", i + 1))
        })
        .collect();
    world.seeded_entries = seed.insert_entries(&entries).await?;
    Ok(())
}

#[given(expr = "I have an entry titled {string}")]
async fn entry_titled(world: &mut RdrsWorld, title: String) -> Result<()> {
    let username = world.user.username.clone();
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let category_id = seed.create_category(user_id, "Highlight Category").await?;
    let feed_id = seed
        .create_feed(
            category_id,
            &format!("https://example.com/{username}-highlight.xml"),
            Some("Highlight Feed"),
        )
        .await?;
    let entry = NewEntry::new(feed_id, &format!("{username}-highlight"), &title)
        .link(format!("https://example.com/{username}/highlight"))
        .content(format!("<p>{title}</p>"))
        .published_offset("-1 hours");
    world.seeded_entries = seed.insert_entries(&[entry]).await?;
    Ok(())
}

#[given("I am on the search page")]
async fn on_search_page(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/search").await
}

// `Given`, despite reading like an action: the scenario reaches it as an `And`
// following a `Given`, and cucumber resolves `And` to whatever keyword came
// before it.
#[given("I use a narrow phone viewport")]
async fn narrow_viewport(world: &mut RdrsWorld) -> Result<()> {
    world.resize(Viewport::new(360, 720)).await
}

/// Enter submits the search form, which is a full navigation — so this waits
/// for the current document to go away rather than letting the assertions race
/// the results page. Without the wait they run against the *old* page, where
/// the previous query's results (or none at all) are still on screen.
#[when(expr = "I search for {string}")]
async fn search_for(world: &mut RdrsWorld, term: String) -> Result<()> {
    let driver = world.driver()?;
    let field = driver.test_id("search-input").await?;
    field.clear().await?;
    field.send_keys(&term).await?;

    let document = driver.find(By::Tag("html")).await?;
    // Sent to the field itself rather than to whatever holds focus: a stray
    // click or a re-render between the fill and the keystroke would otherwise
    // send Enter somewhere that does not submit, and the wait below would then
    // time out with nothing to explain it.
    field.send_keys(Key::Enter).await?;
    document
        .wait_until()
        .wait(WAIT_TIMEOUT, WAIT_INTERVAL)
        .stale()
        .await?;
    Ok(())
}

#[then("I see search results:")]
async fn see_results(world: &mut RdrsWorld, step: &Step) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_visible("search-results").await?;
    for title in first_column(step)? {
        driver.expect_text_somewhere(&title).await?;
    }
    Ok(())
}

#[then(expr = "the result count is {int}")]
async fn result_count(world: &mut RdrsWorld, count: usize) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the number of search results", count, || async {
        Ok(driver.css_all(".search-result").await?.len())
    })
    .await
}

#[then("the search input is focused")]
async fn search_focused(world: &mut RdrsWorld) -> Result<()> {
    ensure!(
        world.driver()?.is_focused("search-input").await?,
        "the search input does not have focus"
    );
    Ok(())
}

#[then("I see the empty-results message")]
async fn empty_results(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("search-empty").await
}

/// When `.search-result-title` inherits `word-break: break-word`, a narrow
/// viewport breaks a Latin term like "Grok" mid-word across a line wrap; the
/// `<mark>` then spans two lines and its box grows to roughly twice the
/// single-line height. `box-decoration-break: clone` coalesces the fragments
/// into one client rect, so counting rects cannot tell them apart — the height
/// can. The fix (`word-break: normal`) keeps the term whole.
#[then(expr = "the highlighted term {string} renders on a single line")]
async fn highlight_single_line(world: &mut RdrsWorld, term: String) -> Result<()> {
    let driver = world.driver()?;
    let selector = ".search-result-title mark";
    let mark = driver.css(selector).await?;
    ensure!(
        mark.content_text().await?.contains(&term),
        "the first highlight is not {term:?}"
    );
    let (_, _, _, height) = driver.bounding_box(selector).await?;
    ensure!(
        height < 30.0,
        "the highlight is {height}px tall, so it wrapped onto a second line"
    );
    Ok(())
}

/// The mobile tap-target rules once set `.search-result-title { display: flex }`.
/// A flex title turns each text run and the `<mark>` into separate flex items
/// that wrap into a broken multi-column layout around the highlight. The title
/// must stay a block so the `<mark>` renders in normal inline flow — a
/// single-line height alone cannot catch this, since a one-word flex item is
/// still one line tall.
#[then("the highlighted title flows as one inline block")]
async fn highlight_inline_block(world: &mut RdrsWorld) -> Result<()> {
    let display = world
        .driver()?
        .computed_style(".search-result-title", "display")
        .await?;
    ensure!(
        !display.contains("flex"),
        "the title is a flex container ({display})"
    );
    Ok(())
}
