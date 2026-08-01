import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

// Promote the currentUser to admin via the seed helper, then sign in.
Given("I am signed in as an admin", async ({ page, api, currentUser, seed, serverUrl }) => {
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  seed.makeAdmin(userId);
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});

// Create a second account so there is a non-self row in the admin table. The
// name is remembered because the disable step has to act on *this* row: the
// table also lists the worker's bootstrap admin (the account that claimed
// /setup, which every later account is created by), and disabling that would
// break every scenario that runs after this one on the same server.
Given("there is another registered user", async ({ api, currentUser }) => {
  currentUser.otherUsername = `other-${currentUser.username}`;
  await api.register(currentUser.otherUsername, "vulture-mango-77-quilt");
});

When("I open the admin page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/admin`);
});

When("I open the statistics page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/statistics`);
});

When("I disable the other user", async ({ page, currentUser }) => {
  const row = page.locator("tr", { hasText: currentUser.otherUsername });
  await row.getByTestId("admin-disable-btn").click();
  // Wait for the page to reload after the form POST redirect.
  await page.waitForURL(/\/admin/);
});

Then("I see my username in the users table", async ({ page, currentUser }) => {
  await expect(page.getByTestId("admin-users-table")).toContainText(currentUser.username);
});

Then("the other user is shown as disabled in the table", async ({ page, currentUser }) => {
  const row = page.locator("tr", { hasText: currentUser.otherUsername });
  await expect(row.getByTestId("admin-user-disabled")).toBeVisible();
});

Then("the statistics show at least {int} feed", async ({ page }, n) => {
  const text = await page.getByTestId("stat-site-feeds-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});

Then("the statistics show at least {int} entries", async ({ page }, n) => {
  const text = await page.getByTestId("stat-site-entries-total").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});
