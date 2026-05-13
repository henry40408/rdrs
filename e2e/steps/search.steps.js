import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I have a feed with entries titled:", async ({ seed, currentUser }, table) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, "Search Category");
  const feedId = seed.createFeed(categoryId, `https://example.com/${currentUser.username}-feed.xml`, "Search Feed");
  const rows = table.raw().map((r, i) => ({
    feedId,
    guid: `${currentUser.username}-${i}`,
    title: r[0],
    link: `https://example.com/${currentUser.username}/${i}`,
    content: `<p>${r[0]}</p>`,
    publishedOffset: `-${i + 1} hours`,
  }));
  seed.insertEntries(rows);
});

Given("I am on the search page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/search`);
});

When("I search for {string}", async ({ page }, term) => {
  await page.getByTestId("search-input").fill(term);
  await page.keyboard.press("Enter");
});

When("I press {string}", async ({ page }, key) => {
  await page.click("body");
  await page.keyboard.press(key);
});

Then("I see search results:", async ({ page }, table) => {
  await expect(page.getByTestId("search-results")).toBeVisible();
  for (const [title] of table.raw()) {
    await expect(page.locator(".search-result-title", { hasText: title })).toBeVisible();
  }
});

Then("the result count is {int}", async ({ page }, count) => {
  await expect(page.locator(".search-result")).toHaveCount(count);
});

Then("the search input is focused", async ({ page }) => {
  await expect(page.getByTestId("search-input")).toBeFocused();
});

Then("I see the empty-results message", async ({ page }) => {
  await expect(page.getByTestId("search-empty")).toBeVisible();
});
