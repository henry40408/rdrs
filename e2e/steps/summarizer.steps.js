import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

When("I open the Summarizer", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/summarizer`);
});

When("I enter these URLs:", async ({ page }, table) => {
  const urls = table.raw().map((row) => row[0]).join("\n");
  await page.getByTestId("summarizer-input").fill(urls);
});

When("I submit the summarizer form", async ({ page }) => {
  await page.getByTestId("summarizer-form").getByRole("button", { name: "Summarize" }).click();
});

Then("I should see {int} summary cards", async ({ page }, count) => {
  await expect(page.locator("[data-summarizer-card]")).toHaveCount(count);
});

// Cards run one at a time (client-side serial queue in summarizer.js), and the
// mock Kagi server adds a 300ms delay per request, so poll each card's
// data-state with a generous timeout rather than asserting immediately.
Then(
  "each card resolves to a completed state containing {string}",
  async ({ page }, text) => {
    const cards = page.locator("[data-summarizer-card]");
    const count = await cards.count();
    for (let i = 0; i < count; i++) {
      const card = cards.nth(i);
      await expect(card).toHaveAttribute("data-state", "completed", { timeout: 10_000 });
      await expect(card.locator("[data-sz-body]")).toContainText(text);
    }
  },
);

Then("I should see a link to Settings", async ({ page }) => {
  // Scope to the page content: the sidebar always carries its own
  // data-testid="nav-settings" link to /user-settings, so a bare href
  // selector also matches that and trips Playwright's strict mode.
  await expect(
    page.locator(".page-content").locator('a[href="/user-settings"]'),
  ).toBeVisible();
});

Then("I should not see the summarizer form", async ({ page }) => {
  await expect(page.getByTestId("summarizer-form")).toHaveCount(0);
});
