import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

// NOTE: "I am signed in" (auth.steps.js) already calls api.register and logs in.
// NOTE: "I open the statistics page" is defined in admin.steps.js.
// This file defines only the three steps unique to the chart interaction.

Given("I have read articles over several days", async ({ currentUser, seed }) => {
  // User is already registered by "I am signed in". Just seed data.
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, "News");
  const feedId = seed.createFeed(categoryId, "https://example.com/feed.xml", "Example");
  const ids = seed.insertEntries(
    Array.from({ length: 4 }, (_, i) => ({
      feedId,
      guid: `stats-${i}`,
      title: `Stats Entry ${i}`,
      link: `https://example.com/e/${i}`,
      content: "<p>x</p>",
    }))
  );
  // Three reads today → today's bar is the tallest (count = 3).
  seed.markRead(ids[0], "0 seconds");
  seed.markRead(ids[1], "-1 hours");
  seed.markRead(ids[2], "-2 hours");
  // One read yesterday → yesterday's bar has count = 1.
  seed.markRead(ids[3], "-1 days");
});

When("I tap the tallest read-activity bar", async ({ page }) => {
  // The chart renders oldest → newest; the last column is today, which has count 3.
  const bar = page.locator("rdrs-reading-chart .stats-bar-col").last();
  await bar.waitFor();
  await bar.click();
});

Then("the chart info card shows a read count", async ({ page }) => {
  const card = page.locator("rdrs-reading-chart .stats-chart-card");
  await expect(card).toBeVisible();
  // Format is "MM/DD · N"; assert it ends with the count 3.
  await expect(card).toContainText(/·\s*3$/);
});

When("I tap the single-read bar", async ({ page }) => {
  // The "yesterday" column has count 1 — a short bar, well below the chart top.
  const bar = page.locator('rdrs-reading-chart .stats-bar-col[data-count="1"]').first();
  await bar.waitFor();
  await bar.click();
});

Then("the info card sits just above that bar", async ({ page }) => {
  const card = page.locator("rdrs-reading-chart .stats-chart-card");
  // Measure the visible coloured fill (.stats-bar), not the full-height column.
  const barFill = page.locator('rdrs-reading-chart .stats-bar-col[data-count="1"] .stats-bar').first();
  await expect(card).toBeVisible();
  const cardBox = await card.boundingBox();
  const barBox = await barFill.boundingBox();
  // The card's bottom edge should hover just above the bar fill's top edge — a
  // small gap, NOT floating at the chart's top edge regardless of bar height.
  const gap = barBox.y - (cardBox.y + cardBox.height);
  expect(gap).toBeGreaterThanOrEqual(0);
  expect(gap).toBeLessThanOrEqual(12);
});
