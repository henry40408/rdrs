import { test, expect } from "../fixtures/rdrs.js";

test.describe("Entry Detail Page", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("detailuser", "password123");

    const userId = seed.getUserId("detailuser");
    const categoryId = seed.createCategory(userId, "Detail Category");
    const feedId = seed.createFeed(categoryId, "https://example.com/detail-feed.xml", "Detail Feed");

    seed.seedTestEntries(feedId, 5);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("detailuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    // Click first entry to load in reading pane
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();
  });

  // PR-12 cleanup: SSR fragment endpoint doesn't auto-mark as read; UX intentionally changed. SSR-first migration changed behavior; spec § Testing schedules this for deletion in PR-12.
  test.fixme("opening entry auto-marks as read", async ({ page, serverUrl }) => {
    // Entry was opened in reading pane, which auto-marks as read.
    // Go back to unread list to verify.
    await page.goto(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    // The entry we just viewed should no longer be in the unread list
    // (count should be 4 instead of 5)
    await expect(page.getByTestId("entry-item")).toHaveCount(4);
  });

  // PR-12 cleanup: SSR uses ★/☆ icon glyphs in button, not "Star"/"Unstar" text. SSR-first migration changed behavior; spec § Testing schedules this for deletion in PR-12.
  test.fixme("s toggles star button text", async ({ page }) => {
    const starBtn = page.getByTestId("rp-star-btn");
    await expect(starBtn).toHaveText("Star");

    await page.keyboard.press("s");
    await expect(starBtn).toHaveText("Unstar");

    await page.keyboard.press("s");
    await expect(starBtn).toHaveText("Star");
  });

  // PR-12 cleanup: SSR fragment endpoint doesn't wire u-key for mark-unread; UX intentionally changed. SSR-first migration changed behavior; spec § Testing schedules this for deletion in PR-12.
  test.fixme("u marks as unread and shows flash", async ({ page }) => {
    await page.keyboard.press("u");

    await expect(page.getByTestId("flash-message")).toBeVisible();
    await expect(page.getByTestId("flash-message")).toContainText(
      "Marked as unread"
    );
  });
});
