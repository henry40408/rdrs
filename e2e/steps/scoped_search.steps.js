import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given(
  "a category {string} containing entries titled {string} and {string}",
  async ({ seed, currentUser }, categoryName, titleA, titleB) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.createCategory(userId, categoryName);
    const feedId = seed.createFeed(
      categoryId,
      `https://example.com/${currentUser.username}-scoped-search.xml`,
      `${categoryName} Feed`
    );
    seed.insertEntries([
      {
        feedId,
        guid: `${currentUser.username}-scoped-a`,
        title: titleA,
        link: `https://example.com/${currentUser.username}/scoped-a`,
        content: `<p>${titleA}</p>`,
        publishedOffset: "-1 hours",
      },
      {
        feedId,
        guid: `${currentUser.username}-scoped-b`,
        title: titleB,
        link: `https://example.com/${currentUser.username}/scoped-b`,
        content: `<p>${titleB}</p>`,
        publishedOffset: "-2 hours",
      },
    ]);
  }
);

When("I type {string} into the scoped search box", async ({ page }, term) => {
  // The scoped-search form auto-submits via a 250ms debounce on `input`
  // (installEntriesSearch in app.js) and swaps `[data-entries-list]` — no
  // Enter key or explicit wait needed; the Then assertions below auto-retry
  // long enough to cover the debounce + fetch round trip.
  await page.getByTestId("scoped-search-input").fill(term);
});

Then("the entry list shows {string}", async ({ page }, title) => {
  await expect(page.getByTestId("entry-item").filter({ hasText: title })).toBeVisible();
});

Then("the entry list does not show {string}", async ({ page }, title) => {
  await expect(page.getByTestId("entry-item").filter({ hasText: title })).toHaveCount(0);
});

When("I mark matching entries as read", async ({ page }) => {
  // The "Mark N matching as Read" form submits through an onsubmit
  // window.confirm. Arm the one-shot dialog handler and trigger the click in
  // the same step so the accept is in place before the prompt fires (a
  // separate pre-arming step races the native form submit here).
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByTestId("mark-matching-btn").click();
});

Then("{string} is no longer in the unread list", async ({ page }, title) => {
  // "Mark matching as Read" POSTs and redirects back to the same scoped
  // (q=…) unread-tab URL — the now-read entry drops out of the default
  // unread filter, so absence from the list is the correct signal here.
  await expect(page.getByTestId("entry-item").filter({ hasText: title })).toHaveCount(0);
});
