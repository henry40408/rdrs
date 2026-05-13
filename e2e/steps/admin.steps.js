import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";
import Database from "better-sqlite3";

const { Given, When, Then } = createBdd(test);

// Promote the currentUser to admin via direct SQL, then sign in.
Given("I am signed in as an admin", async ({ page, api, currentUser, seed, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  const db = new Database(seed.dbPath);
  try {
    db.prepare(`UPDATE user SET role = 'admin' WHERE id = ?`).run(userId);
  } finally {
    db.close();
  }
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

// Register a second user so there is a non-self row in the admin table.
Given("there is another registered user", async ({ api, currentUser }) => {
  const otherUsername = `other-${currentUser.username}`;
  await api.register(otherUsername, "password123");
});

When("I open the admin page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/admin`);
});

When("I open the statistics page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/statistics`);
});

// Click the disable button on the first row that has one (non-self rows only).
When("I disable the first non-self user", async ({ page }) => {
  const disableBtn = page.getByTestId("admin-disable-btn").first();
  await disableBtn.click();
  // Wait for the page to reload after the form POST redirect.
  await page.waitForURL(/\/admin/);
});

Then("I see my username in the users table", async ({ page, currentUser }) => {
  await expect(page.getByTestId("admin-users-table")).toContainText(currentUser.username);
});

Then("that user is shown as disabled", async ({ page }) => {
  await expect(page.getByTestId("admin-user-disabled").first()).toBeVisible();
});

Then("the statistics show at least {int} feed", async ({ page }, n) => {
  const text = await page.getByTestId("stat-feeds-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});

Then("the statistics show at least {int} entries", async ({ page }, n) => {
  const text = await page.getByTestId("stat-entries-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});
