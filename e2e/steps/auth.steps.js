import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I am a registered user", async ({ api, currentUser }) => {
  await api.register(currentUser.username, currentUser.password);
});

// Register an unrelated account first so the account under test is NOT the
// instance's first user (the first registration is promoted to admin).
Given("the instance already has an owner account", async ({ api }) => {
  await api.register("e2e-owner", "password123");
});

Given("I am signed in", async ({ page, api, currentUser, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

When("I register with matching passwords", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/register`);
  await page.getByTestId("register-username").fill(currentUser.username);
  await page.getByTestId("register-password").fill(currentUser.password);
  await page.getByTestId("register-confirm-password").fill(currentUser.password);
  await page.getByTestId("register-submit").click();
});

When("I register with mismatched passwords", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/register`);
  await page.getByTestId("register-username").fill(currentUser.username);
  await page.getByTestId("register-password").fill(currentUser.password);
  await page.getByTestId("register-confirm-password").fill("different456");
  await page.getByTestId("register-submit").click();
});

When("I sign in with my credentials", async ({ page, currentUser }) => {
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
});

When("I sign in with the wrong password", async ({ page, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill("wrongpassword");
  await page.getByTestId("login-submit").click();
});

Then("I am redirected to the login page with a success message", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/login`);
  await expect(page.getByTestId("flash-message")).toBeVisible();
  await expect(page.getByTestId("flash-message")).toContainText("Registration successful");
});

Then("I land on the unread inbox", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/`);
  await expect(page.getByTestId("main-nav")).toBeVisible();
});

Then("I see a login error", async ({ page }) => {
  await expect(page.getByTestId("login-error")).toBeVisible();
});

Then("I see {string} on the register page", async ({ page }, message) => {
  await expect(page.getByTestId("register-error")).toContainText(message);
});

Then("I am still on the register page", async ({ page }) => {
  expect(page.url()).toContain("/register");
});

Then("the sidebar does not offer the app settings link", async ({ page }) => {
  await expect(page.getByTestId("main-nav")).toBeVisible();
  await expect(page.getByTestId("nav-app-settings")).toHaveCount(0);
});

Then("I am not shown the app settings page", async ({ page }) => {
  // The admin guard redirects to /login, which bounces an already-signed-in
  // session back to the inbox — either way the config table never renders.
  await expect(page.getByRole("heading", { name: "App", exact: true })).toHaveCount(0);
  expect(page.url()).not.toContain("/settings");
});
