//! Theme, password and retention settings — a port of `preferences.steps.js`.

use anyhow::Result;
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::eventually_eq;
use rdrs_e2e::world::RdrsWorld;

/// The preferences form, which shares its test ids with the password form on
/// the same page — hence the `form[action=…]` prefixes below.
const PREFERENCES_FORM: &str = r#"form[action="/user-settings/preferences"]"#;
const PASSWORD_FORM: &str = r#"form[action="/user-settings/password"]"#;

#[given("I am on the user settings page")]
async fn on_user_settings(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/user-settings").await?;
    world.driver()?.expect_visible("theme-select").await
}

#[when(expr = "I switch the theme to {string}")]
async fn switch_theme(world: &mut RdrsWorld, theme: String) -> Result<()> {
    let driver = world.driver()?;
    driver.select_option("theme-select", &theme).await?;
    driver
        .click_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

#[then(expr = "the html element has data-theme {string}")]
async fn html_has_theme(world: &mut RdrsWorld, value: String) -> Result<()> {
    world
        .driver()?
        .expect_attr("html", "data-theme", Some(&value))
        .await
}

#[then("the html element has no data-theme attribute")]
async fn html_has_no_theme(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_attr("html", "data-theme", None).await
}

/// Light mode drops the global `-webkit-font-smoothing: antialiased` (which
/// renders dark ink on light paper thin on macOS) back to the heavier `auto`;
/// dark mode keeps `antialiased`. The computed value is deterministic across
/// platforms even though the visual effect is macOS-only, so this guards the
/// per-theme rule without depending on screenshot rendering.
#[then(expr = "the body uses {string} font smoothing")]
async fn body_font_smoothing(world: &mut RdrsWorld, value: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the body's font smoothing", value, || async {
        let smoothing = driver
            .eval(
                "return getComputedStyle(document.body)\
                 .getPropertyValue('-webkit-font-smoothing');",
            )
            .await?;
        Ok(smoothing.as_str().unwrap_or_default().to_owned())
    })
    .await
}

#[when(expr = "I change my password to {string}")]
async fn change_password(world: &mut RdrsWorld, new_password: String) -> Result<()> {
    let current = world.user.password.clone();
    let driver = world.driver()?;
    driver
        .fill_css(
            &format!("{PASSWORD_FORM} [data-testid=\"current-password\"]"),
            &current,
        )
        .await?;
    driver
        .fill_css(
            &format!("{PASSWORD_FORM} [data-testid=\"new-password\"]"),
            &new_password,
        )
        .await?;
    driver
        .fill_css(
            &format!("{PASSWORD_FORM} [data-testid=\"confirm-new-password\"]"),
            &new_password,
        )
        .await?;
    driver
        .click_css(&format!("{PASSWORD_FORM} button[type=submit]"))
        .await?;
    // The server deletes every session and redirects to /login on success.
    world.expect_path("/login").await?;
    world.user.password = new_password;
    Ok(())
}

/// After a password change the session is already destroyed and the browser is
/// sitting on `/login`.
#[then(expr = "I can sign in with {string}")]
async fn can_sign_in_with(world: &mut RdrsWorld, password: String) -> Result<()> {
    let username = world.user.username.clone();
    world.goto("/login").await?;
    let driver = world.driver()?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", &password).await?;
    driver.click("login-submit").await?;
    world.expect_path("/").await
}

#[when(expr = "I set the retention period to {string} days")]
async fn set_retention(world: &mut RdrsWorld, days: String) -> Result<()> {
    let driver = world.driver()?;
    driver.fill("retention-read-days", &days).await?;
    driver
        .click_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

#[then(expr = "the retention period field shows {string}")]
async fn retention_shows(world: &mut RdrsWorld, value: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the retention field", value, || {
        driver.value_of("retention-read-days")
    })
    .await
}
