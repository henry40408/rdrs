//! Registration, sign-in, invites and sign-out.

use anyhow::{Result, ensure};
use cucumber::{given, then, when};
use rdrs_e2e::api::PASSWORD;
use rdrs_e2e::dom::Dom;
use rdrs_e2e::world::RdrsWorld;

/// Stores the invite path on the world for a later step.
#[given("an admin has created an account for me")]
async fn admin_created_account(world: &mut RdrsWorld) -> Result<()> {
    let username = world.user.username.clone();
    world.invite_path = Some(world.api().invite_account(&username).await?);
    Ok(())
}

#[given("I am a registered user")]
async fn registered_user(world: &mut RdrsWorld) -> Result<()> {
    let (username, password) = world.credentials();
    world.api().register(&username, &password).await
}

/// Registers another account first, since the first user becomes admin.
#[given("the instance already has an owner account")]
async fn instance_has_owner(world: &mut RdrsWorld) -> Result<()> {
    world.api().register("e2e-owner", PASSWORD).await
}

#[given("I am signed in")]
async fn signed_in(world: &mut RdrsWorld) -> Result<()> {
    let (username, password) = world.credentials();
    world.api().register(&username, &password).await?;
    world.goto("/login").await?;
    let driver = world.driver()?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", &password).await?;
    driver.click("login-submit").await?;
    world.expect_path("/").await
}

#[when("I open my one-time link and choose a password")]
async fn redeem_invite(world: &mut RdrsWorld) -> Result<()> {
    let password = world.user.password.clone();
    submit_invite(world, &password).await
}

/// The confirmation clears `minlength`, so the server's mismatch check (not
/// constraint validation) rejects it.
#[when("I open my one-time link and mistype the confirmation")]
async fn mistype_confirmation(world: &mut RdrsWorld) -> Result<()> {
    submit_invite(world, "badger-kestrel-19-plume").await
}

async fn submit_invite(world: &mut RdrsWorld, confirmation: &str) -> Result<()> {
    let invite = world.invite_path()?;
    let password = world.user.password.clone();
    world.goto(&invite).await?;
    let driver = world.driver()?;
    driver.fill("invite-password", &password).await?;
    driver.fill("invite-confirm-password", confirmation).await?;
    driver.click("invite-submit").await?;
    Ok(())
}

#[when("I sign in with my credentials")]
async fn sign_in(world: &mut RdrsWorld) -> Result<()> {
    let (username, password) = world.credentials();
    let driver = world.driver()?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", &password).await?;
    driver.click("login-submit").await?;
    Ok(())
}

#[when("I sign in with the wrong password")]
async fn sign_in_wrong_password(world: &mut RdrsWorld) -> Result<()> {
    let username = world.user.username.clone();
    world.goto("/login").await?;
    let driver = world.driver()?;
    driver.fill("username-input", &username).await?;
    driver.fill("password-input", "wrongpassword").await?;
    driver.click("login-submit").await?;
    Ok(())
}

#[when("I log out")]
async fn log_out(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.click("logout-btn").await?;
    world.expect_path("/login").await
}

#[when("I visit the home page")]
async fn visit_home(world: &mut RdrsWorld) -> Result<()> {
    world.goto("/").await
}

#[when(expr = "I visit {string}")]
async fn visit_path(world: &mut RdrsWorld, path: String) -> Result<()> {
    world.goto(&path).await
}

#[then("I am redirected to the login page with a success message")]
async fn redirected_with_success(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/login").await?;
    world
        .driver()?
        .expect_text("flash-message", "Password set")
        .await
}

#[then("I land on the unread inbox")]
async fn land_on_inbox(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/").await?;
    world.driver()?.expect_visible("main-nav").await
}

#[then("I see a login error")]
async fn see_login_error(world: &mut RdrsWorld) -> Result<()> {
    world.driver()?.expect_visible("login-error").await
}

#[then(expr = "I see {string} on the invite page")]
async fn see_invite_error(world: &mut RdrsWorld, message: String) -> Result<()> {
    world.driver()?.expect_text("invite-error", &message).await
}

#[then("the sidebar does not offer the app settings link")]
async fn no_app_settings_link(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    driver.expect_visible("main-nav").await?;
    driver.expect_absent("nav-app-settings").await
}

/// The admin guard redirects to `/login`, which bounces a signed-in session
/// back to the inbox.
#[then("I am not shown the app settings page")]
async fn not_shown_app_settings(world: &mut RdrsWorld) -> Result<()> {
    let driver = world.driver()?;
    ensure!(
        driver.heading_opt("App").await?.is_none(),
        "the app settings heading rendered"
    );
    let url = driver.current_url().await?;
    ensure!(
        !url.as_str().contains("/settings"),
        "still on the settings page: {url}"
    );
    Ok(())
}

#[then("I see the logged-out flash message")]
async fn logged_out_flash(world: &mut RdrsWorld) -> Result<()> {
    world
        .driver()?
        .expect_text("flash-message", "You have been logged out.")
        .await
}

/// Logout sends `Clear-Site-Data` so the sidebar's `sessionStorage` mirror
/// does not leak to the next user.
#[then("the sidebar's cached data no longer survives in session storage")]
async fn sidebar_cache_cleared(world: &mut RdrsWorld) -> Result<()> {
    let cached = world
        .driver()?
        .execute("return sessionStorage.getItem('rdrs.sidebar.v1');", vec![])
        .await?;
    ensure!(
        cached.json().is_null(),
        "the sidebar cache survived logout: {:?}",
        cached.json()
    );
    Ok(())
}

#[then("I am redirected to the login page")]
async fn redirected_to_login(world: &mut RdrsWorld) -> Result<()> {
    world.expect_path("/login").await
}
