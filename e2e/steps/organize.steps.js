import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I have a category named {string}", async ({ seed, currentUser }, name) => {
  const userId = seed.getUserId(currentUser.username);
  seed.createCategory(userId, name);
});

Given("I am on the feeds page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds`);
  await expect(page.getByTestId("feed-category-select")).not.toContainText("Loading");
});

When("I add a feed from the mock RSS server under {string}", async ({ page, feedServerUrl }, _category) => {
  await page.getByTestId("feed-url-input").fill(`${feedServerUrl}/feed.xml`);
  await page.getByTestId("feed-category-select").selectOption({ index: 0 });
  await page.getByTestId("add-feed-btn").click();
});

Then("I see a success flash {string}", async ({ page }, message) => {
  await expect(page.getByTestId("flash-message")).toContainText(message);
});

Then("the feeds table contains {string}", async ({ page }, text) => {
  await expect(page.getByTestId("feeds-table")).toContainText(text);
});
