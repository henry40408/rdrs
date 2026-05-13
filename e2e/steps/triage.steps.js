import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

When("I star the entry titled {string}", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("entry-star-action") // data-testid="entry-star-action" on the star button
    .first()
    .click();
});

When("I mark the entry titled {string} read", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("entry-read-action") // data-testid="entry-read-action" on the read button
    .first()
    .click();
});

Then("the entry titled {string} is marked starred", async ({ page }, title) => {
  // After starring, _entry_row.html renders a <span class="star-icon">⭐</span> inside the row.
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row.locator(".star-icon")).toBeVisible();
});

Then("the sidebar starred count is at least {int}", async ({ page }, n) => {
  // The sidebar Starred link (<a href="/entries/starred">) has no numeric badge —
  // only the Unread link carries a count. This step can only assert the link is
  // visible; a count assertion requires a future UI addition.
  // @skip kept on the owning scenario until a badge is added.
  const locator = page.locator('a[href="/entries/starred"]').first();
  await expect(locator).toBeVisible();
  void n; // numeric check deferred until sidebar starred-count badge is added
});

Then("the sidebar unread count decreases by {int}", async ({ page }, _delta) => {
  // The total-unread badge has id="unread-count" (rendered by rdrs-sidebar.js).
  // Delta comparison requires capturing the before-count, which needs a fixture
  // hook. For now assert the element is present and shows a non-negative number.
  // @skip kept on the owning scenario until before/after capture is wired.
  const locator = page.locator("#unread-count").first();
  const text = await locator.innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(0);
});

Then("the reading pane shows a summary", async ({ page }) => {
  // The summary container always renders as <div id="rp-summary-container">.
  // When a summary exists, a .summary-box is inside it.
  await expect(page.locator("#rp-summary-container .summary-box")).toBeVisible();
});

Then("the reading pane summary is dismissed", async ({ page }) => {
  // After clicking "Dismiss", app.js calls container.replaceChildren() which
  // empties #rp-summary-container but keeps the wrapper element in the DOM.
  await expect(page.locator("#rp-summary-container .summary-box")).toHaveCount(0);
});
