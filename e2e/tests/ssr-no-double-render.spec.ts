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

test.describe("Load More appends without duplicates after SSR hydration", () => {
  const TOTAL = 35;

  // Use a fresh username so we don't collide with the suite above.
  test.beforeAll(async ({ api, seed }) => {
    await api.register("loadmoreuser", "password123");

    const userId = seed.getUserId("loadmoreuser");
    const categoryId = seed.createCategory(userId, "LoadMore Cat");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/loadmore-feed.xml",
      "LoadMore Feed"
    );

    // Seed entries so id order matches real usage: lower id = older published_at.
    // This is the assumption the GReader API pagination relies on (`e.id < continuation`
    // returning *older* entries when sorting by published_at DESC).
    const entries = Array.from({ length: TOTAL }, (_, idx) => {
      const i = idx + 1;
      return {
        feedId,
        guid: `loadmore-guid-${i}`,
        title: `Test Entry ${i}`,
        link: `https://example.com/entry/${i}`,
        content: `<p>Content ${i}</p>`,
        publishedOffset: `-${TOTAL - i + 1} hours`,
      };
    });
    seed.insertEntries(entries);
  });

  test("clicking Load More yields unique entry ids", async ({
    page,
    serverUrl,
  }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("loadmoreuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    // Wait for SSR hydration.
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    const firstPageIds = await page
      .getByTestId("entry-item")
      .evaluateAll((els) => els.map((el) => el.id));
    expect(new Set(firstPageIds).size).toBe(firstPageIds.length);

    // Click Load More and wait for the second page to land.
    await expect(page.getByTestId("load-more-btn")).toBeVisible();
    await page.getByTestId("load-more-btn").click();
    await expect(page.getByTestId("entry-item")).toHaveCount(TOTAL);

    const allIds = await page
      .getByTestId("entry-item")
      .evaluateAll((els) => els.map((el) => el.id));
    expect(allIds).toHaveLength(TOTAL);
    // Failure here means the SSR continuation off-by-one came back: the boundary entry
    // got re-fetched on Load More and rendered twice.
    expect(new Set(allIds).size).toBe(TOTAL);
  });
});
