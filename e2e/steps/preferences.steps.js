import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I am on the user settings page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/user-settings`);
  await expect(page.getByTestId("theme-select")).toBeVisible();
});

When("I switch the theme to {string}", async ({ page, serverUrl }, theme) => {
  await page.getByTestId("theme-select").selectOption(theme);
  await page.locator('form[action="/user-settings/preferences"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
});

Then("the html element has data-theme {string}", async ({ page }, value) => {
  await expect(page.locator("html")).toHaveAttribute("data-theme", value);
});

Then("the html element has no data-theme attribute", async ({ page }) => {
  await expect(page.locator("html")).not.toHaveAttribute("data-theme");
});

When("I change my password to {string}", async ({ page, currentUser, serverUrl }, newPassword) => {
  await page.locator('form[action="/user-settings/password"] [data-testid="current-password"]').fill(currentUser.password);
  await page.locator('form[action="/user-settings/password"] [data-testid="new-password"]').fill(newPassword);
  await page.locator('form[action="/user-settings/password"] [data-testid="confirm-new-password"]').fill(newPassword);
  await page.locator('form[action="/user-settings/password"] button[type=submit]').click();
  // The server deletes all sessions and redirects to /login on success
  await page.waitForURL(`${serverUrl}/login`);
  currentUser.password = newPassword;
});

Then("I can sign in with {string}", async ({ page, currentUser, serverUrl }, password) => {
  // After password change, session is already destroyed and page is at /login
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

When("I set the retention period to {string} days", async ({ page, serverUrl }, days) => {
  await page.getByTestId("retention-read-days").fill(days);
  await page.locator('form[action="/user-settings/preferences"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
});

Then("the retention period field shows {string}", async ({ page }, value) => {
  await expect(page.getByTestId("retention-read-days")).toHaveValue(value);
});
