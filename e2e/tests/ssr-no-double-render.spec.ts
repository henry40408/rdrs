import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

const STREAM_CONTENTS_PATH = "/reader/api/0/stream/contents/";

/**
 * Asserts that SSR-enabled list pages render their first entry directly from
 * server HTML and skip the redundant `stream/contents` fetch on first paint.
 * This is the perf goal of issue #148.
 */
test.describe("SSR list pages skip first stream/contents fetch", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("ssruser", "password123");

    const userId = seed.getUserId("ssruser");
    const categoryId = seed.createCategory(userId, "SSR Cat");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/ssr-feed.xml",
      "SSR Feed"
    );

    seed.seedTestEntries(feedId, 5);

    // Extra entry whose title matches the search test query.
    seed.insertEntries([
      {
        feedId,
        guid: "ssr-search-quokka",
        title: "Quokka Discovery",
        link: "https://example.com/quokka",
        content: "<p>About a quokka.</p>",
      },
    ]);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("ssruser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  /** Navigate to `path` and return how many `stream/contents` requests fired. */
  async function gotoCounting(page: Page, fullUrl: string): Promise<number> {
    let count = 0;
    const handler = (req: { url(): string }) => {
      if (req.url().includes(STREAM_CONTENTS_PATH)) count += 1;
    };
    page.on("request", handler);
    try {
      await page.goto(fullUrl);
      await expect(page.getByTestId("entry-item").first()).toBeVisible();
      // Give any deferred fetch from JS hydration time to land.
      await page.waitForTimeout(300);
    } finally {
      page.off("request", handler);
    }
    return count;
  }

  test("/ (unread)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/`);
    expect(count).toBe(0);
  });

  test("/entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/entries`);
    expect(count).toBe(0);
  });

  test("/feeds/:id/entries", async ({ page, serverUrl, seed }) => {
    const feedId = seed.createFeed(
      seed.createCategory(seed.getUserId("ssruser"), "SSR Cat"),
      "https://example.com/ssr-feed.xml",
      "SSR Feed"
    );
    await login(page, serverUrl);
    const count = await gotoCounting(
      page,
      `${serverUrl}/feeds/${feedId}/entries`
    );
    expect(count).toBe(0);
  });

  test("/search?q=Quokka", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/search?q=Quokka`);
    expect(count).toBe(0);
    await expect(page.getByText("Quokka Discovery")).toBeVisible();
  });
});
