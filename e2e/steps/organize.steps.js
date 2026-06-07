import path from "node:path";
import { fileURLToPath } from "node:url";
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

const FIXTURES_DIR = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../fixtures"
);

Given("I have a category named {string}", async ({ seed, currentUser }, name) => {
  const userId = seed.getUserId(currentUser.username);
  seed.createCategory(userId, name);
});

Given(
  "I have a feed {string} in category {string}",
  async ({ seed, currentUser }, feedTitle, categoryName) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.createCategory(userId, categoryName);
    seed.createFeed(
      categoryId,
      `https://example.com/${currentUser.username}-${feedTitle}.xml`,
      feedTitle
    );
  }
);

Given(
  "I have a feed from the mock RSS server in category {string}",
  async ({ seed, currentUser, feedServerUrl }, categoryName) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.createCategory(userId, categoryName);
    seed.createFeed(categoryId, `${feedServerUrl}/feed.xml`, "Test Feed");
  }
);

Given("I am on the feeds page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds`);
  await expect(page.getByTestId("feed-category-select")).not.toContainText("Loading");
});

Given("I am on the categories page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/categories`);
  await expect(page.getByTestId("category-name-input")).toBeVisible();
});

When("I add a feed from the mock RSS server under {string}", async ({ page, feedServerUrl }, category) => {
  await page.getByTestId("feed-url-input").fill(`${feedServerUrl}/feed.xml`);
  await page.getByTestId("feed-category-select").selectOption({ label: category });
  await page.getByTestId("add-feed-btn").click();
});

When("I create a category named {string}", async ({ page }, name) => {
  await page.getByTestId("category-name-input").fill(name);
  await page.getByTestId("add-category-btn").click();
});

When("I rename category {string} to {string}", async ({ page }, oldName, newName) => {
  // The <input> `value` attribute reflects the server-rendered initial state, not
  // the live `.value` property — so scope to the row by the original name BEFORE
  // filling, then click save within that same row.
  const row = page
    .getByTestId("categories-table")
    .locator(`tr:has(input[value="${oldName}"])`);
  await row.locator("input[type='text']").fill(newName);
  await row.locator("button.cat-rename-save").click();
});

When("I delete category {string}", async ({ page }, name) => {
  await page
    .getByTestId("categories-table")
    .locator(`tr:has(input[value="${name}"]) button.action-link-danger`)
    .click();
});

When("I filter feeds by category {string}", async ({ page }, name) => {
  // Option labels carry a count suffix like "Other Category (1)", so we can't
  // pass an exact label. Look up the matching option's value via DOM, then
  // selectOption(value). onchange auto-submits — waitForURL settles on the
  // filtered URL before subsequent assertions.
  const value = await page.locator("#filter-category").evaluate((sel, prefix) => {
    const opt = Array.from(sel.options).find((o) => o.text.startsWith(prefix + " ("));
    return opt ? opt.value : "";
  }, name);
  await page.locator("#filter-category").selectOption(value);
  await page.waitForURL(/\?.*category=\d+/);
});

When("I refresh the feed {string}", async ({ page }, title) => {
  await page
    .getByTestId("feeds-table")
    .locator("tr")
    .filter({ hasText: title })
    .locator("form[action$='/refresh'] button")
    .click();
});

When(
  "I edit the feed {string} and set its title to {string}",
  async ({ page }, oldTitle, newTitle) => {
    await page
      .getByTestId("feeds-table")
      .locator("tr")
      .filter({ hasText: oldTitle })
      .getByRole("link", { name: "edit" })
      .click();
    await page.getByTestId("feed-edit-title-input").fill(newTitle);
    await page.getByTestId("feed-edit-save-btn").click();
  }
);

When("I delete the feed {string}", async ({ page }, title) => {
  // Same row-scoped danger-button pattern as "I delete category"; the caller
  // must register "I confirm the next dialog" first (delete form uses confirm()).
  await page
    .getByTestId("feeds-table")
    .locator("tr")
    .filter({ hasText: title })
    .locator("button.action-link-danger")
    .click();
});

Given("I am on the import OPML page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds/import`);
  await expect(page.getByTestId("opml-file-input")).toBeVisible();
});

When("I import the OPML fixture {string}", async ({ page }, filename) => {
  await page
    .getByTestId("opml-file-input")
    .setInputFiles(path.join(FIXTURES_DIR, filename));
  await page.getByTestId("opml-import-btn").click();
});

Then("the exported OPML contains {string}", async ({ page, serverUrl }, text) => {
  // The Export OPML link is a GET download; page.request shares the browser
  // context's auth cookie, so this exports the signed-in user's subscriptions.
  const res = await page.request.get(
    `${serverUrl}/reader/api/0/subscription/export`
  );
  expect(res.ok()).toBeTruthy();
  expect(await res.text()).toContain(text);
});

Then("I see a success flash {string}", async ({ page }, message) => {
  await expect(page.getByTestId("flash-message")).toContainText(message);
});

Then("the flash banner shows a timestamp", async ({ page }) => {
  // HH:MM:SS — server-rendered for SSR cookie/inline-template paths,
  // client-rendered for window.flash.show() emits. Both paths must
  // produce a same-shape `<time>` element so the visual is consistent.
  await expect(page.getByTestId("flash-time").first()).toHaveText(
    /^\d{2}:\d{2}:\d{2}$/
  );
  await expect(page.getByTestId("flash-time").first()).toHaveAttribute("datetime", /.+/);
});

Then("the feeds table contains {string}", async ({ page }, text) => {
  await expect(page.getByTestId("feeds-table")).toContainText(text);
});

Then("the feeds table does not contain {string}", async ({ page }, text) => {
  await expect(page.getByTestId("feeds-table")).not.toContainText(text);
});

Then("the categories table contains {string}", async ({ page }, text) => {
  // Category names live in <input value="..."> within the rename form; their
  // text doesn't appear as DOM text content. Match the attribute directly.
  await expect(
    page.getByTestId("categories-table").locator(`input[value="${text}"]`)
  ).toBeVisible();
});

Then("the categories table does not contain {string}", async ({ page }, text) => {
  await expect(
    page.getByTestId("categories-table").locator(`input[value="${text}"]`)
  ).toHaveCount(0);
});
