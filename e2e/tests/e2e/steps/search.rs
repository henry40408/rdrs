//! Full-text search and the highlight layout.

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

// `Given`: reached as an `And` after a `Given`.
#[given("I use a narrow phone viewport")]
async fn narrow_viewport(world: &mut RdrsWorld) -> Result<()> {
    world.resize(Viewport::new(360, 720)).await
}

/// Enter is a full navigation; wait for the old document to go so assertions
/// do not run against the previous page.
#[when(expr = "I search for {string}")]
async fn search_for(world: &mut RdrsWorld, term: String) -> Result<()> {
    let driver = world.driver()?;
    let field = driver.test_id("search-input").await?;
    field.clear().await?;
    field.send_keys(&term).await?;

    let document = driver.find(By::Tag("html")).await?;
    // Sent to the field, not the focused element, so a stray re-render cannot
    // swallow the Enter.
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

/// With `word-break: break-word` a narrow viewport splits "Grok" across lines;
/// `box-decoration-break: clone` merges the rects, so check height instead.
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

/// A flex title (from old tap-target rules) splits text and `<mark>` into flex
/// items; the title must stay a block. Height alone cannot catch this.
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
