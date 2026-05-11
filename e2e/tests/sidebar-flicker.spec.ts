import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

test.describe("sidebar unread badge does not flicker", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("flickeruser", "password123");
    const userId = seed.getUserId("flickeruser");
    const categoryId = seed.createCategory(userId, "FlickerCat");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/flicker.xml",
      "Flicker Feed"
    );
    seed.seedTestEntries(feedId, 3);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("flickeruser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  // PR-12 cleanup: SSR triggers full reload on nav so this CSR-specific stale-state concern doesn't exist; new sidebar polling covers freshness differently. SSR-first migration changed behavior; spec § Testing schedules this for deletion in PR-12.
  test.fixme("after mark-as-read, SPA-nav does not show stale unread count", async ({
    page,
    serverUrl,
  }) => {
    await login(page, serverUrl);

    // Cold load shows 3 unread.
    await expect(page.locator("#unread-count")).toHaveText("3");

    // Mark one entry read via the row action; the in-page badge should drop to 2.
    await page.getByTestId("entry-read-action").first().click();
    await expect(page.locator("#unread-count")).toHaveText("2");

    // Hold the next /api/sidebar response so the synchronous mount-time render
    // is the only thing that has run when we sample the badge.
    await page.route("**/api/sidebar", async (route) => {
      await new Promise((r) => setTimeout(r, 600));
      await route.continue();
    });

    // SPA-navigate to /feeds. New <rdrs-sidebar> mounts and renders synchronously
    // from whatever client-side cache it picked up.
    await page.getByTestId("nav-feeds").click();
    await expect(page.getByTestId("feeds-table")).toBeVisible();

    // Sample the badge while /api/sidebar is still in flight. The bug we're
    // fixing is: the new sidebar paints the *stale* "3" from the bootstrap
    // <script>, then the API response arrives and corrects it to "2" — the
    // visible flicker. After the fix, the synchronous paint must already be
    // the freshest known value.
    const earlyBadge = await page
      .locator("#unread-count")
      .textContent({ timeout: 200 });
    expect(earlyBadge).not.toBe("3");
  });
});
