//! Sidebar ordering and the hide-fully-read toggle.

use anyhow::{Context, Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::{eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;

/// Shared with the theme; the page is re-read so submitting writes back the
/// other fields' current values.
const PREFERENCES_FORM: &str = r#"form[action="/user-settings/preferences"]"#;

async fn submit_preferences(world: &RdrsWorld) -> Result<()> {
    world
        .driver()?
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

#[when(expr = "I set the sidebar order to {string}")]
async fn set_sidebar_order(world: &mut RdrsWorld, order: String) -> Result<()> {
    world.goto("/user-settings").await?;
    world
        .driver()?
        .select_option("sidebar-sort", &order)
        .await?;
    submit_preferences(world).await
}

// Registered as `Given` and `When`: used both ways.
#[given("fully-read categories and feeds are hidden")]
#[when("fully-read categories and feeds are hidden")]
async fn hide_fully_read(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/user-settings").await?;
    world.driver()?.check("sidebar-hide-read").await?;
    submit_preferences(world).await
}

#[given(expr = "all entries in feed {string} are marked read")]
async fn feed_all_read(world: &mut RdrsWorld, title: String) -> Result<()> {
    let user_id = world.user_id().await?;
    world.seed().mark_feed_read(user_id, &title).await
}

/// Exhaustive, so a row that should be hidden fails here.
#[then(expr = "the sidebar categories read {string}")]
async fn sidebar_categories_read(world: &mut RdrsWorld, expected: String) -> Result<()> {
    expect_labels(
        world,
        "#sidebar-categories a[data-category-id] .sidebar-item-label",
        &expected,
    )
    .await
}

#[then(expr = "the sidebar feeds read {string}")]
async fn sidebar_feeds_read(world: &mut RdrsWorld, expected: String) -> Result<()> {
    expect_labels(
        world,
        ".sidebar-feed[data-feed-id] .sidebar-item-label",
        &expected,
    )
    .await
}

/// Hiding fully-read feeds can empty the open category's list, whose margins
/// then showed as a gap. Measured as distance to the next row, after the list
/// mounts so it cannot pass before the feeds arrive.
#[then(expr = "the sidebar leaves no gap below category {string}")]
async fn no_gap_below_category(world: &mut RdrsWorld, name: String) -> Result<()> {
    let driver = world.driver()?;
    eventually("the open category's feed list mounted", || async {
        Ok(!driver
            .css_all(".sidebar-feeds[data-category-id]")
            .await?
            .is_empty())
    })
    .await?;
    let script = format!(
        r##"
        const name = {};
        const rows = [...document.querySelectorAll("#sidebar-categories a[data-category-id]")];
        const at = rows.findIndex(
            (row) => row.querySelector(".sidebar-item-label")?.textContent.trim() === name,
        );
        if (at < 0 || at + 1 >= rows.length) return null;
        return Math.round(
            rows[at + 1].getBoundingClientRect().top - rows[at].getBoundingClientRect().bottom,
        );
        "##,
        serde_json::Value::from(name.as_str()),
    );
    let gap = driver
        .eval(&script)
        .await?
        .as_i64()
        .with_context(|| format!("`{name}` and the category below it were not both listed"))?;
    ensure!(gap <= 1, "the sidebar left a {gap}px gap below `{name}`");
    Ok(())
}

#[then(expr = "the sidebar order field shows {string}")]
async fn sidebar_order_shows(world: &mut RdrsWorld, value: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the sidebar order field", value, || {
        driver.value_of("sidebar-sort")
    })
    .await
}

#[then("the hide-fully-read checkbox is checked")]
async fn hide_read_checked(world: &mut RdrsWorld) -> Result<()> {
    ensure!(
        world.driver()?.is_checked("sidebar-hide-read").await?,
        "the hide-fully-read checkbox is not ticked"
    );
    Ok(())
}

async fn expect_labels(world: &RdrsWorld, selector: &str, expected: &str) -> Result<()> {
    let expected: Vec<String> = expected
        .split(',')
        .map(|name| name.trim().to_owned())
        .collect();
    let driver = world.driver()?;
    eventually_eq(
        &format!("the labels matching `{selector}`"),
        expected,
        || async {
            let texts = driver.texts_of(selector).await?;
            Ok(texts
                .into_iter()
                .map(|text| text.trim().to_owned())
                .collect::<Vec<_>>())
        },
    )
    .await
}
