//! Theme, password, retention and timezone settings.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::dom::Dom;
use rdrs_e2e::wait::{eventually, eventually_eq};
use rdrs_e2e::world::RdrsWorld;

/// Shares test ids with the password form, hence the `form[action=…]` prefix.
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
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

// Also used as an `And` barrier after a `When`.
#[then(expr = "the html element has data-theme {string}")]
#[when(expr = "the html element has data-theme {string}")]
async fn html_has_theme(world: &mut RdrsWorld, value: String) -> Result<()> {
    world
        .driver()?
        .expect_attr("html", "data-theme", Some(&value))
        .await
}

#[then("the html element has no data-theme attribute")]
async fn html_has_no_theme(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_attr("html", "data-theme", None)
        .await
}

/// Light mode uses `auto` smoothing, dark keeps `antialiased`; the computed
/// value is checkable on any platform though the effect is macOS-only.
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
        .submit_css(&format!("{PASSWORD_FORM} button[type=submit]"))
        .await?;
    // The server deletes every session and redirects to /login on success.
    world.expect_path("/login").await?;
    world.user.password = new_password;
    Ok(())
}

/// The password change already destroyed the session and landed on `/login`.
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

#[when(expr = "I set entries per page to {string}")]
async fn set_entries_per_page(world: &mut RdrsWorld, count: String) -> Result<()> {
    let driver = world.driver()?;
    driver.fill("entries-per-page", &count).await?;
    driver
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}

#[when(expr = "I set the retention period to {string} days")]
async fn set_retention(world: &mut RdrsWorld, days: String) -> Result<()> {
    let driver = world.driver()?;
    driver.fill("retention-read-days", &days).await?;
    driver
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
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

/// The page reads the zone when it loads, so this applies to the pages opened
/// after it.
#[when(expr = "my browser is in the timezone {string}")]
async fn browser_in_timezone(world: &mut RdrsWorld, zone: String) -> Result<()> {
    world
        .driver()?
        .cdp()
        .send_raw(
            "Emulation.setTimezoneOverride",
            serde_json::json!({ "timezoneId": zone }),
        )
        .await?;
    Ok(())
}

/// The report is sent after the page loads, and the notice appears on that
/// same page when the reply arrives.
#[then(expr = "I am offered to review the timezone {string}")]
async fn offered_timezone(world: &mut RdrsWorld, zone: String) -> Result<()> {
    eventually("the browser timezone offer", || async {
        Ok(world
            .driver()?
            .css_opt(&timezone_action(&zone))
            .await?
            .is_some())
    })
    .await
}

/// The link that suggests `zone`.
fn timezone_action(zone: &str) -> String {
    let encoded = zone.replace('/', "%2F");
    format!(r#"[data-testid="flash-action"][href$="timezone={encoded}"]"#)
}

#[then("I am not offered to review the timezone")]
async fn not_offered_timezone(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("flash-action").await
}

#[when("I watch for browser reports")]
async fn watch_browser_reports(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .watch_requests(r"/api/user/client-info(\?|$)", "PUT")
        .await?;
    world.client_info_puts = Some(handle);
    Ok(())
}

#[then("no browser report is sent")]
async fn no_browser_report(world: &mut RdrsWorld) -> Result<()> {
    let handle = world
        .client_info_puts
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no report watch was armed"))?;
    // The page sends its report right after loading.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let count = handle.arrived();
    ensure!(count == 0, "{count} browser report(s) were sent");
    Ok(())
}

#[when(expr = "I follow the timezone suggestion for {string}")]
async fn follow_timezone_suggestion(world: &mut RdrsWorld, zone: String) -> Result<()> {
    let driver = world.driver()?;
    driver.click_css(&timezone_action(&zone)).await?;
    driver.expect_visible("timezone-suggestion").await
}

#[then(expr = "the timezone field shows {string}")]
async fn timezone_shows(world: &mut RdrsWorld, zone: String) -> Result<()> {
    let driver = world.driver()?;
    eventually_eq("the timezone field", zone, || driver.value_of("timezone")).await
}

#[then("the timezone is marked as not saved")]
async fn timezone_marked_unsaved(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("timezone-suggestion").await
}

#[then("the timezone is not marked as not saved")]
async fn timezone_not_marked_unsaved(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_absent("timezone-suggestion").await
}

#[when("I save the preferences")]
async fn save_preferences(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .submit_css(&format!("{PREFERENCES_FORM} button[type=submit]"))
        .await?;
    world.expect_path("/user-settings").await
}
