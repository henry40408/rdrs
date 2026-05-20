import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

const VIEWPORTS = {
  mobile: { width: 375, height: 667 },
  tablet: { width: 768, height: 1024 },
  desktop: { width: 1280, height: 800 },
};

Given("I am viewing on a {word} screen", async ({ page }, kind) => {
  const v = VIEWPORTS[kind];
  if (!v) throw new Error(`Unknown viewport: ${kind}`);
  await page.setViewportSize(v);
});

Given("I have a feed with {int} test entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, `Cat-${currentUser.username}`);
  const feedId = seed.createFeed(categoryId, `https://example.com/${currentUser.username}.xml`, "Mobile Feed");
  seed.seedTestEntries(feedId, count);
});

When("I open the inbox", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/`);
});

When("I open the categories page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/categories`);
});

When("I tap the hamburger", async ({ page }) => {
  await page.locator(".sidebar-toggle").click();
});

When("I tap the sidebar close button", async ({ page }) => {
  await page.locator(".sidebar-close").click();
});

Then("the sidebar is visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toHaveClass(/open/);
});

Then("the sidebar is not visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).not.toHaveClass(/open/);
});

Then("the hamburger button is visible", async ({ page }) => {
  await expect(page.locator(".sidebar-toggle")).toBeVisible();
});

Then("the entry list pane is at least {int}px wide", async ({ page }, minWidth) => {
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeGreaterThanOrEqual(minWidth);
});

Then("the categories table is shown as cards", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).toHaveCSS("display", "none");
  await expect(page.locator("table.mobile-cards td[data-label]").first()).toBeVisible();
});

Then("the categories table is shown as a table", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).not.toHaveCSS("display", "none");
});

Then("the sidebar is always-visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator(".sidebar-toggle")).toBeHidden();
});

Then("the entry list pane is narrower than the viewport", async ({ page }) => {
  const viewport = page.viewportSize();
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeLessThan(viewport.width * 0.9);
});

Then("the reading pane is visible on mobile", async ({ page }) => {
  // At ≤1024px width the reading pane is `display: none` by default and only
  // surfaces when the `.reading-pane-active` overlay class is present. Assert
  // both the class is applied AND the element is actually visible — class
  // alone would pass even if a future CSS regression unset `display: block`.
  const pane = page.locator("#reading-pane");
  await expect(pane).toHaveClass(/reading-pane-active/);
  await expect(pane).toBeVisible();
});

When("I tap the reading-pane back button", async ({ page }) => {
  await page.getByTestId("reading-pane-back").click();
});
