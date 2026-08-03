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

When("I open the scoped search box", async ({ page }) => {
  await page.getByTestId("scoped-search-toggle").click();
  await expect(page.getByTestId("scoped-search-input")).toBeFocused();
});

When("I close the scoped search box", async ({ page }) => {
  await page.getByTestId("scoped-search-close").click();
});

Then("the scoped search box is open", async ({ page }) => {
  await expect(page.getByTestId("scoped-search-input")).toBeVisible();
  await expect(page.getByTestId("scoped-search-toggle")).toHaveAttribute("aria-expanded", "true");
});

Then("the scoped search box is closed", async ({ page }) => {
  // The drawer collapses to a zero-height grid row, so the input is present
  // but not visible — exactly what "hidden behind the toggle" means here.
  await expect(page.getByTestId("scoped-search-input")).toBeHidden();
  await expect(page.getByTestId("scoped-search-toggle")).toHaveAttribute("aria-expanded", "false");
});

// Height parity is what makes the filter bar read as one control strip and the
// drawer as one field. Both chips take their height from a sibling
// (`align-self: stretch`), which is exactly the kind of rule a later layout
// change breaks silently — so measure it.
const heightOf = async (locator) => (await locator.boundingBox()).height;

Then("the search toggle is as tall as the status filter", async ({ page }) => {
  const toggle = await heightOf(page.getByTestId("scoped-search-toggle"));
  const select = await heightOf(page.getByTestId("status-filter-select"));
  expect(Math.abs(toggle - select)).toBeLessThan(1);
});

Then("the search close button is as tall as the search box", async ({ page }) => {
  const close = await heightOf(page.getByTestId("scoped-search-close"));
  const input = await heightOf(page.getByTestId("scoped-search-input"));
  expect(Math.abs(close - input)).toBeLessThan(1);
});

// On mobile the drawer opens under the fixed hamburger and its row is indented
// past the button, putting the two side by side — so their midlines have to
// agree. The drawer row's block padding is the only thing holding that, and it
// is invisible to any per-element size assertion.
Then("the scoped search box shares its midline with the hamburger", async ({ page }) => {
  // Poll rather than measure once: the drawer expands over a 0.16s
  // grid-template-rows transition, and while its clip is still short the
  // centered input reports a box straddling the pane's top edge.
  const midlineOf = async (locator) => {
    const box = await locator.boundingBox();
    return box.y + box.height / 2;
  };
  await expect
    .poll(async () =>
      Math.abs(
        (await midlineOf(page.getByTestId("scoped-search-input"))) -
          (await midlineOf(page.locator(".sidebar-toggle"))),
      ),
    )
    .toBeLessThan(1);
});

Then("the mark-above button is hidden", async ({ page }) => {
  await expect(page.getByTestId("mark-above-btn")).toHaveCount(0);
});

Then("the mark-above button is shown", async ({ page }) => {
  await expect(page.getByTestId("mark-above-btn")).toBeVisible();
});

When("I type {string} into the scoped search box", async ({ page }, term) => {
  // The scoped-search form auto-submits via a 250ms debounce on `input`
  // (installEntriesSearch in app.js) and swaps `[data-entries-list]` — no
  // Enter key or explicit wait needed; the Then assertions below auto-retry
  // long enough to cover the debounce + fetch round trip.
  await page.getByTestId("scoped-search-input").fill(term);
});

When("I clear the scoped search box", async ({ page }) => {
  // Same debounced auto-submit path as typing — clearing fires an `input`
  // event, swaps the (now-unfiltered) list, and syncScopedSearchParam removes
  // `?q=` from the address bar.
  await page.getByTestId("scoped-search-input").fill("");
});

Then("the URL has the {string} query parameter set to {string}", async ({ page }, key, value) => {
  // Poll: the URL is replaceState'd only after the debounced swap resolves.
  await expect.poll(() => new URL(page.url()).searchParams.get(key)).toBe(value);
});

Then("the URL has no {string} query parameter", async ({ page }, key) => {
  await expect.poll(() => new URL(page.url()).searchParams.get(key)).toBeNull();
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
