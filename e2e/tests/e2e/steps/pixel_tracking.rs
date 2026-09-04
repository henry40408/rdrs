//! Open tracking: the opt-in, the pixel a real browser fetches, and the two
//! pages that read the counts back.

use anyhow::{Context, Result};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::eventually;
use rdrs_e2e::world::RdrsWorld;

const PREFERENCES_FORM: &str = r#"form[action="/user-settings/preferences"]"#;

/// Turn tracking on through the real form, which is what one scenario is
/// actually about.
#[when("I turn on open tracking")]
async fn turn_on_tracking(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/user-settings").await?;
    let driver = world.driver()?;
    driver.check("pixel-tracking").await?;
    driver
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

/// The same opt-in as a precondition. Seeded rather than driven, because
/// entries created before the opt-in carry no pixel — a scenario that needs a
/// rate has to enable tracking before it seeds its feed, and paying a page load
/// for that in every one of them buys nothing.
#[given("I have open tracking turned on")]
async fn tracking_turned_on(world: &mut RdrsWorld) -> Result<()> {
    let user_id = world.user_id().await?;
    world.seed().enable_pixel_tracking(user_id).await
}

/// Every entry of the feed just seeded, recorded as opened. Stands in for an
/// external client that fetched each pixel during a sync — the case a
/// browser-driven scenario cannot reach.
#[given(expr = "every entry in {string} has been opened")]
async fn every_entry_opened(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    let seed = world.seed().clone();
    let feed_id = seed.feed_id_by_title(user_id, &feed_title).await?;
    seed.record_opens_for_feed(user_id, feed_id).await
}

#[then("the feeds table has an open rate column")]
async fn has_open_rate_column(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_text_somewhere("Open Rate").await
}

#[then("the feeds table has no open rate column")]
async fn has_no_open_rate_column(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    let headers = driver.texts_of("table.feeds-table thead th").await?;
    anyhow::ensure!(
        !headers.iter().any(|h| h.contains("Open Rate")),
        "an opted-out reader should not be shown a column of dashes: {headers:?}"
    );
    Ok(())
}

/// The pixel `<img>` inside the rendered article, as the browser sees it after
/// the swap. Same-origin and un-proxied is the whole property under test — the
/// sanitiser would have stripped a 1x1 image and rewritten its `src` through
/// `/api/proxy/image`, so finding it here proves injection ran afterwards.
async fn pixel_srcs(driver: &thirtyfour::WebDriver) -> Result<Vec<String>> {
    let value = driver
        .eval(
            "return Array.from(\
               document.querySelectorAll('.reading-pane img, #reading-pane img')\
             ).map(i => i.getAttribute('src')).filter(s => s && s.includes('/p/'));",
        )
        .await?;
    Ok(value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

#[then("the reading pane carries a tracking pixel")]
async fn pane_carries_pixel(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    eventually("the reading pane to carry a tracking pixel", || async {
        let srcs = pixel_srcs(driver).await?;
        Ok(srcs
            .iter()
            .any(|s| s.starts_with("/p/") && s.to_ascii_lowercase().ends_with(".gif")))
    })
    .await
}

#[then("the reading pane carries no tracking pixel")]
async fn pane_carries_no_pixel(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    // The pane has to be there first, or this passes on an empty page.
    driver.expect_text_somewhere("Test Entry 1").await?;
    let srcs = pixel_srcs(driver).await?;
    anyhow::ensure!(
        srcs.is_empty(),
        "an opted-out reader's entry must carry no pixel: {srcs:?}"
    );
    Ok(())
}

/// The open-rate cell of the row naming `feed_title`.
async fn open_rate_cell(world: &mut RdrsWorld, feed_title: &str) -> Result<String> {
    let driver = world.driver()?;
    let row = driver.row_with_text(feed_title).await?;
    let cell = row
        .find(thirtyfour::By::Css("[data-testid=\"feed-open-rate\"]"))
        .await
        .with_context(|| format!("no open-rate cell in the row for `{feed_title}`"))?;
    Ok(cell.text().await?)
}

#[then(expr = "the open rate for {string} is {string}")]
async fn open_rate_is(world: &mut RdrsWorld, feed_title: String, expected: String) -> Result<()> {
    let actual = open_rate_cell(world, &feed_title).await?;
    anyhow::ensure!(
        actual.contains(&expected),
        "expected the open rate for `{feed_title}` to read `{expected}`, got `{actual}`"
    );
    Ok(())
}

#[then(expr = "the open rate for {string} is not reported yet")]
async fn open_rate_not_reported(world: &mut RdrsWorld, feed_title: String) -> Result<()> {
    let actual = open_rate_cell(world, &feed_title).await?;
    anyhow::ensure!(
        !actual.contains('%'),
        "a feed below the sample floor must not be given a percentage, got `{actual}`"
    );
    Ok(())
}

#[then(expr = "the statistics page ranks {string} above {string} by open rate")]
async fn statistics_ranks(world: &mut RdrsWorld, first: String, second: String) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_text_somewhere("Feeds by Open Rate").await?;
    let titles = driver
        .texts_of("[data-testid=\"stats-open-rate\"] .stats-bar-row-header span:first-child")
        .await?;
    let at = |name: &str| titles.iter().position(|t| t.trim() == name);
    let (Some(a), Some(b)) = (at(&first), at(&second)) else {
        anyhow::bail!("expected both `{first}` and `{second}` to be listed, got {titles:?}");
    };
    anyhow::ensure!(
        a < b,
        "the least-opened feed belongs at the top, got {titles:?}"
    );
    Ok(())
}
