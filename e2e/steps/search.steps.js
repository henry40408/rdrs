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

Given("I have an entry titled {string}", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, "Highlight Category");
  const feedId = seed.createFeed(
    categoryId,
    `https://example.com/${currentUser.username}-highlight.xml`,
    "Highlight Feed"
  );
  seed.insertEntries([
    {
      feedId,
      guid: `${currentUser.username}-highlight`,
      title,
      link: `https://example.com/${currentUser.username}/highlight`,
      content: `<p>${title}</p>`,
      publishedOffset: "-1 hours",
    },
  ]);
});

Given("I am on the search page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/search`);
});

When("I use a narrow phone viewport", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 720 });
});

Then("the highlighted term {string} renders on a single line", async ({ page }, term) => {
  // When `.search-result-title` inherits `word-break: break-word`, a narrow
  // viewport breaks a Latin term like "Grok" mid-word across a line wrap; the
  // <mark> then spans two lines and its box grows to ~2× the single-line height
  // (measured ~43px vs ~22px here). box-decoration-break:clone coalesces the
  // fragments into one client rect, so rect *count* can't tell them apart —
  // assert on box height instead. The fix (word-break: normal) keeps the term
  // whole, so the mark stays a single line.
  const mark = page.locator(".search-result-title mark", { hasText: term });
  await expect(mark).toBeVisible();
  const height = await mark.evaluate((el) => el.getBoundingClientRect().height);
  expect(height).toBeLessThan(30);
});

Then("the highlighted title flows as one inline block", async ({ page }) => {
  // The mobile tap-target rules once set `.search-result-title { display: flex }`.
  // A flex/inline-flex title turns each text run and the highlight <mark> into
  // separate flex items that wrap into a broken multi-column layout around the
  // highlight (text appears to flow around the marked word). The title must stay
  // a block so the <mark> renders in normal inline flow — assert it is not a flex
  // container (a single-line mark height alone can't catch this: "Grok" as its
  // own flex item is still one line tall).
  const display = await page
    .locator(".search-result-title")
    .first()
    .evaluate((el) => getComputedStyle(el).display);
  expect(display).not.toMatch(/flex/);
});

When("I search for {string}", async ({ page }, term) => {
  await page.getByTestId("search-input").fill(term);
  await page.keyboard.press("Enter");
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
