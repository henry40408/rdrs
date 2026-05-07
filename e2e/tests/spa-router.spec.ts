import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

test.describe("SPA router", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("spauser", "password123");
    const userId = seed.getUserId("spauser");
    const catId = seed.createCategory(userId, "Cat A");
    const feedId = seed.createFeed(catId, "https://example.com/cat-a.xml", "Feed A");
    seed.seedTestEntries(feedId, 5);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("spauser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  /**
   * Counts top-level Document loads. SPA navigation must not increment.
   * Captures every navigation request (incl. redirects) on the main frame.
   */
  function trackDocumentLoads(page: Page): { count: () => number; dispose: () => void } {
    let count = 0;
    const handler = (req: {
      resourceType(): string;
      isNavigationRequest(): boolean;
      frame(): { parentFrame(): unknown };
    }) => {
      if (
        req.isNavigationRequest() &&
        req.resourceType() === "document" &&
        !req.frame().parentFrame()
      ) {
        count += 1;
      }
    };
    page.on("request", handler);
    return {
      count: () => count,
      dispose: () => page.off("request", handler),
    };
  }

  test("same-element nav: /entries -> /entries/starred (no document reload)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.goto(`${serverUrl}/entries`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.getByTestId("tab-starred").click();
      await expect(page).toHaveURL(`${serverUrl}/entries/starred`);
      await expect(page.locator("rdrs-entries-page")).toHaveAttribute("data-mode", "starred");
      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("cross-element nav: / -> /feeds -> /statistics (no document reload)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expect(page.locator("rdrs-entries-page")).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.getByTestId("nav-feeds").click();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();
      await expect(page.locator("rdrs-entries-page")).toHaveCount(0);

      await page.getByTestId("nav-statistics").click();
      await expect(page).toHaveURL(`${serverUrl}/statistics`);
      await expect(page.locator("rdrs-statistics-page")).toBeVisible();

      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("popstate: back/forward swaps elements correctly", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.getByTestId("nav-feeds").click();
    await expect(page.locator("rdrs-feeds-page")).toBeVisible();
    await page.getByTestId("nav-statistics").click();
    await expect(page.locator("rdrs-statistics-page")).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.goBack();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();

      await page.goBack();
      await expect(page).toHaveURL(`${serverUrl}/`);
      await expect(page.locator("rdrs-entries-page")).toBeVisible();

      await page.goForward();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();

      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("modifier-click opens in new tab without disturbing current page", async ({ page, serverUrl, context }) => {
    await login(page, serverUrl);

    const popupPromise = context.waitForEvent("page");
    await page.getByTestId("nav-feeds").click({ modifiers: ["ControlOrMeta"] });
    const popup = await popupPromise;
    await popup.waitForLoadState();
    await expect(popup).toHaveURL(`${serverUrl}/feeds`);
    await popup.close();

    await expect(page).toHaveURL(`${serverUrl}/`);
  });

  test("non-routed link triggers full reload", async ({ page, serverUrl }) => {
    await login(page, serverUrl);

    const tracker = trackDocumentLoads(page);
    try {
      await page.evaluate(() => {
        const a = document.createElement("a");
        a.href = "/login";
        a.textContent = "go login";
        a.id = "test-fallback-link";
        // Position above the sidebar so the click isn't intercepted.
        a.style.cssText = "position:fixed;top:8px;right:8px;z-index:9999;background:#fff;padding:4px;";
        document.body.appendChild(a);
      });
      await page.click("#test-fallback-link");
      await page.waitForURL(`${serverUrl}/login`);
      expect(tracker.count()).toBeGreaterThanOrEqual(1);
    } finally {
      tracker.dispose();
    }
  });
});
