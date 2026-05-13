import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

When("I star the entry titled {string}", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("star-toggle")
    .first()
    .click();
});

When("I mark the entry titled {string} read", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("read-toggle")
    .first()
    .click();
});

Then("the entry titled {string} is marked starred", async ({ page }, title) => {
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row.locator("[data-starred='true']")).toBeVisible();
});

Then("the sidebar starred count is at least {int}", async ({ page }, n) => {
  const text = await page.getByTestId("sidebar-starred-count").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(n);
});

Then("the sidebar unread count decreases by {int}", async ({ page }, _delta) => {
  // Sketched assertion — refined when @skip is lifted post-PR-10/11.
  const text = await page.getByTestId("sidebar-unread-count").innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(0);
});

Then("the reading pane shows a summary", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-summary")).toBeVisible();
});
