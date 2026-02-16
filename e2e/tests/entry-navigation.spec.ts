import { test, expect } from "../fixtures/rdrs.js";

test.describe("Entry Keyboard Navigation", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("navuser", "password123");

    const userId = seed.getUserId("navuser");
    const categoryId = seed.createCategory(userId, "Nav Category");
    const feedId = seed.createFeed(categoryId, "https://example.com/nav-feed.xml", "Nav Feed");

    seed.seedTestEntries(feedId, 10);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    // Login via UI
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("navuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    // Wait for entries to load
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
  });

  test("j selects first entry, j/k navigates up and down", async ({
    page,
  }) => {
    // Press j to select first entry
    await page.keyboard.press("j");
    const firstEntry = page.getByTestId("entry-item").nth(0);
    await expect(firstEntry).toHaveClass(/selected/);

    // Press j again to select second entry
    await page.keyboard.press("j");
    const secondEntry = page.getByTestId("entry-item").nth(1);
    await expect(secondEntry).toHaveClass(/selected/);
    await expect(firstEntry).not.toHaveClass(/selected/);

    // Press k to go back up
    await page.keyboard.press("k");
    await expect(firstEntry).toHaveClass(/selected/);
    await expect(secondEntry).not.toHaveClass(/selected/);
  });

  test("Enter opens entry, q returns to list", async ({ page, serverUrl }) => {
    // Select and open first entry
    await page.keyboard.press("j");
    await page.keyboard.press("Enter");

    // Should navigate to entry detail page
    await page.waitForURL(/\/entries\/\d+/);
    await expect(page.getByTestId("entry-title")).toBeVisible();

    // Press q to go back
    await page.keyboard.press("q");
    await page.waitForURL(`${serverUrl}/`);
  });

  test("gg jumps to first, G jumps to last", async ({ page }) => {
    // Move down a few entries
    await page.keyboard.press("j");
    await page.keyboard.press("j");
    await page.keyboard.press("j");

    // gg should jump to first entry
    await page.keyboard.press("g");
    await page.keyboard.press("g");
    const firstEntry = page.getByTestId("entry-item").nth(0);
    await expect(firstEntry).toHaveClass(/selected/);

    // G should jump to last entry
    await page.keyboard.press("G");
    const lastEntry = page.getByTestId("entry-item").last();
    await expect(lastEntry).toHaveClass(/selected/);
  });

  test("n/N navigates to next/previous unread", async ({ page }) => {
    // On unread page, all entries are unread, so n just goes to next
    await page.keyboard.press("n");
    const firstEntry = page.getByTestId("entry-item").nth(0);
    await expect(firstEntry).toHaveClass(/selected/);

    await page.keyboard.press("n");
    const secondEntry = page.getByTestId("entry-item").nth(1);
    await expect(secondEntry).toHaveClass(/selected/);

    // N goes back to previous unread
    await page.keyboard.press("N");
    await expect(firstEntry).toHaveClass(/selected/);
  });
});
