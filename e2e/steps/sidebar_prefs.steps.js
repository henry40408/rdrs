import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

// The two sidebar settings ship in the same "Display Preferences" form as the
// theme, so each of these submits the whole form — which is why they re-read
// the page first rather than posting a hand-built body: whatever the other
// fields currently hold is what gets written back.
const submitPreferences = async (page, serverUrl) => {
  await page.locator('form[action="/user-settings/preferences"] button[type=submit]').click();
  await page.waitForURL(`${serverUrl}/user-settings`);
};

When("I set the sidebar order to {string}", async ({ page, serverUrl }, order) => {
  await page.goto(`${serverUrl}/user-settings`);
  await page.getByTestId("sidebar-sort").selectOption(order);
  await submitPreferences(page, serverUrl);
});

Given("fully-read categories and feeds are hidden", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/user-settings`);
  await page.getByTestId("sidebar-hide-read").check();
  await submitPreferences(page, serverUrl);
});

Given("all entries in feed {string} are marked read", async ({ seed, currentUser }, title) => {
  seed.markFeedRead(seed.getUserId(currentUser.username), title);
});

// Ordered, exhaustive assertions: `toHaveText` with an array also pins the
// count, so a row that should have been hidden fails here rather than passing
// unnoticed at the end of the list.
const namesOf = (expected) => expected.split(",").map((s) => s.trim());

Then("the sidebar categories read {string}", async ({ page }, expected) => {
  await expect(
    page.locator("#sidebar-categories a[data-category-id] .sidebar-item-label")
  ).toHaveText(namesOf(expected));
});

Then("the sidebar feeds read {string}", async ({ page }, expected) => {
  await expect(
    page.locator(".sidebar-feed[data-feed-id] .sidebar-item-label")
  ).toHaveText(namesOf(expected));
});

Then("the sidebar order field shows {string}", async ({ page }, value) => {
  await expect(page.getByTestId("sidebar-sort")).toHaveValue(value);
});

Then("the hide-fully-read checkbox is checked", async ({ page }) => {
  await expect(page.getByTestId("sidebar-hide-read")).toBeChecked();
});
