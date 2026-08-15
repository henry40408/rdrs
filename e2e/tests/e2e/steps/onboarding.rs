//! The landing page's getting-started guide and the app settings page — a port
//! of `onboarding.steps.js`.

use anyhow::{Result, ensure};
use cucumber::{then, when};
use rdrs_e2e::dom::{Dom, Within};
use rdrs_e2e::world::RdrsWorld;

#[when("I open the landing page")]
async fn open_landing(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/").await
}

#[when("I am on the settings page")]
async fn on_settings(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/settings").await
}

#[then(expr = "the category dropdown offers {string}")]
async fn category_dropdown_offers(world: &mut RdrsWorld, label: String) -> Result<()> {
    world
        .driver()?
        .expect_text("feed-category-select", &label)
        .await
}

#[then("the landing page shows the getting-started guide")]
async fn shows_guide(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("onboarding-guide").await
}

#[then("the landing page does not show the getting-started guide")]
async fn no_guide(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("onboarding-guide").await
}

#[then(expr = "I see an {string} call to action")]
async fn call_to_action(world: &mut RdrsWorld, label: String) -> Result<()> {
    let guide = world.driver()?.test_id("onboarding-guide").await?;
    let link = guide.link_named(&label).await?;
    let link = link.ok_or_else(|| anyhow::anyhow!("the guide offers no `{label}` link"))?;
    ensure!(link.is_displayed().await?, "the `{label}` link is hidden");
    Ok(())
}

#[then(expr = "I see {string} on the landing page")]
async fn see_text_on_landing(world: &mut RdrsWorld, text: String) -> Result<()> {
    world.driver()?.expect_text_somewhere(&text).await
}

#[then("I see the active WebAuthn RP origin")]
async fn see_rp_origin(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_text("webauthn-rp-origin", "http")
        .await
}
