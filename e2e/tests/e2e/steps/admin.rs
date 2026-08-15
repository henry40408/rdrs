//! The admin and statistics pages — a port of `admin.steps.js`.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::api::PASSWORD;
use rdrs_e2e::dom::{Dom, Within, submit_element};
use rdrs_e2e::world::RdrsWorld;

/// Promotes this scenario's account to admin through the seed helper, then
/// signs in.
#[given("I am signed in as an admin")]
async fn signed_in_as_admin(world: &mut RdrsWorld) -> Result<()> {
    let (username, password) = world.credentials();
    world.api().register(&username, &password).await?;
    let user_id = world.user_id().await?;
    world.seed().make_admin(user_id).await?;

    world.goto("/login").await?;
    let driver = world.driver()?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", &password).await?;
    driver.click("login-submit").await?;
    world.expect_path("/").await
}

/// Creates a second account so there is a non-self row in the admin table.
///
/// The name is remembered because the disable step has to act on *this* row:
/// the table also lists the bootstrap admin (the account that claimed
/// `/api/setup`, which every later account is created by), and disabling that
/// would break every scenario that runs after this one.
#[given("there is another registered user")]
async fn another_registered_user(world: &mut RdrsWorld) -> Result<()> {
    let other = format!("other-{}", world.user.username);
    world.api().register(&other, PASSWORD).await?;
    world.other_username = Some(other);
    Ok(())
}

#[when("I open the admin page")]
async fn open_admin(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/admin").await
}

#[when("I open the statistics page")]
async fn open_statistics(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/statistics").await
}

#[when("I disable the other user")]
async fn disable_other_user(world: &mut RdrsWorld) -> Result<()> {
    let other = world.other_username()?;
    let driver = world.driver()?;
    let button = driver
        .row_with_text(&other)
        .await?
        .test_id("admin-disable-btn")
        .await?;
    // The form POST redirects back to /admin, so the wait is for the document
    // to be replaced — what the old `waitForURL(/\/admin/)` stood in for, and
    // which watching the URL cannot detect when it redirects to the same page.
    submit_element(driver, &button).await
}

#[then("I see my username in the users table")]
async fn see_username_in_table(world: &mut RdrsWorld) -> Result<()> {
    let username = world.user.username.clone();
    world
        .driver()?
        .expect_text("admin-users-table", &username)
        .await
}

#[then("the other user is shown as disabled in the table")]
async fn other_user_disabled(world: &mut RdrsWorld) -> Result<()> {
    let other = world.other_username()?;
    let row = world.driver()?.row_with_text(&other).await?;
    let badge = row.test_id("admin-user-disabled").await?;
    ensure!(
        badge.is_displayed().await?,
        "`{other}` is not marked disabled"
    );
    Ok(())
}

#[then(expr = "the statistics show at least {int} feed")]
async fn stats_feeds(world: &mut RdrsWorld, minimum: u32) -> Result<()> {
    expect_at_least(world, "stat-site-feeds-total", minimum).await
}

#[then(expr = "the statistics show at least {int} entries")]
async fn stats_entries(world: &mut RdrsWorld, minimum: u32) -> Result<()> {
    expect_at_least(world, "stat-site-entries-total", minimum).await
}

async fn expect_at_least(world: &RdrsWorld, id: &str, minimum: u32) -> Result<()> {
    let text = world.driver()?.text_of(id).await?;
    let count: u32 = text
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("`{id}` reads {text:?}, which is not a count"))?;
    ensure!(
        count >= minimum,
        "`{id}` is {count}, expected at least {minimum}"
    );
    Ok(())
}
