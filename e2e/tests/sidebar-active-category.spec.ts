import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

test.describe("sidebar marks the current category as active", () => {
  let aId = 0;
  let bId = 0;
  let aFeedId = 0;

  test.beforeAll(async ({ api, seed }) => {
    await api.register("activecatuser", "password123");
    const userId = seed.getUserId("activecatuser");
    aId = seed.createCategory(userId, "ActiveCatA");
    bId = seed.createCategory(userId, "ActiveCatB");
    aFeedId = seed.createFeed(aId, "https://example.com/a.xml", "Feed A");
    seed.createFeed(bId, "https://example.com/b.xml", "Feed B");
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("activecatuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  test("/categories/{id}/entries highlights the matching category", async ({
    page,
    serverUrl,
  }) => {
    await login(page, serverUrl);

    await page.goto(`${serverUrl}/categories/${aId}/entries`);
    await expect(page.locator("rdrs-entries-page")).toBeVisible();

    const aLink = page.locator(`a.sidebar-item[href="/categories/${aId}/entries"]`);
    const bLink = page.locator(`a.sidebar-item[href="/categories/${bId}/entries"]`);
    await expect(aLink).toHaveClass(/(^|\s)active(\s|$)/);
    await expect(bLink).not.toHaveClass(/(^|\s)active(\s|$)/);
  });

  test("/feeds/{id}/entries highlights the feed's parent category", async ({
    page,
    serverUrl,
  }) => {
    await login(page, serverUrl);

    await page.goto(`${serverUrl}/feeds/${aFeedId}/entries`);
    await expect(page.locator("rdrs-entries-page")).toBeVisible();

    const aLink = page.locator(`a.sidebar-item[href="/categories/${aId}/entries"]`);
    const bLink = page.locator(`a.sidebar-item[href="/categories/${bId}/entries"]`);
    await expect(aLink).toHaveClass(/(^|\s)active(\s|$)/);
    await expect(bLink).not.toHaveClass(/(^|\s)active(\s|$)/);
  });
});
