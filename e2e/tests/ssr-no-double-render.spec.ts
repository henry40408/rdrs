import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

const STREAM_CONTENTS_PATH = "/reader/api/0/stream/contents/";

/**
 * First paint must fire EXACTLY one `stream/contents` fetch — every list
 * route is CSR, so first paint always issues the one fetch needed to
 * populate the entries list. Two would mean a render race or a
 * connectedCallback double-fire (we hit that during B1 — see the
 * about:blank flush in `gotoCounting`).
 */
test.describe("First paint fires exactly one stream/contents fetch", () => {
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
    // Park on about:blank first so in-flight requests from the prior
    // navigation (e.g. post-login `/` page, which is now CSR and fires its
    // own stream/contents) settle before we start counting. Without this
    // step the listener catches both the lingering post-login fetch and
    // the fresh fetch for the target URL.
    await page.goto("about:blank");

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
    expect(count).toBe(1);
  });

  test("/entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/entries`);
    expect(count).toBe(1);
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
    expect(count).toBe(1);
  });

  test("/search?q=Quokka", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/search?q=Quokka`);
    expect(count).toBe(1);
    await expect(page.getByText("Quokka Discovery")).toBeVisible();
  });
});

/**
 * Comprehensive Load More regression: every SSR-backed list route must let the
 * client paginate without re-rendering the boundary entry. This catches the
 * SSR/API continuation mismatch that broke /, /entries, and /categories before.
 */
test.describe("Load More appends without duplicates on every list route", () => {
  // Default entries-per-page is 30; per-feed seed > 30 so Load More has work.
  const PER_FEED = 35;

  // Stash IDs so the test bodies (which take fixtures separately) can reach them.
  let unreadCategoryId: number;
  let readCategoryId: number;
  let unreadFeedId: number;
  let readFeedId: number;

  test.beforeAll(async ({ api, seed }) => {
    await api.register("loadmoreuser", "password123");
    const userId = seed.getUserId("loadmoreuser");

    unreadCategoryId = seed.createCategory(userId, "Unread Cat");
    unreadFeedId = seed.createFeed(
      unreadCategoryId,
      "https://example.com/lm-unread.xml",
      "Unread Feed"
    );

    readCategoryId = seed.createCategory(userId, "Read Cat");
    readFeedId = seed.createFeed(
      readCategoryId,
      "https://example.com/lm-read.xml",
      "Read Feed"
    );

    // Helper: seed N entries with id-correlated published_at (lower id = older).
    const buildEntries = (
      feedId: number,
      prefix: string,
      titlePrefix: string
    ) =>
      Array.from({ length: PER_FEED }, (_, idx) => {
        const i = idx + 1;
        return {
          feedId,
          guid: `${prefix}-${i}`,
          title: `${titlePrefix} ${i}`,
          link: `https://example.com/${prefix}/${i}`,
          content: `<p>${titlePrefix} ${i}</p>`,
          publishedOffset: `-${PER_FEED - i + 1} hours`,
        };
      });

    seed.insertEntries(buildEntries(unreadFeedId, "lm-unread", "TestEntry"));
    const readIds = seed.insertEntries(
      buildEntries(readFeedId, "lm-read", "TestEntry")
    );

    // For "Read Feed" — mark every entry as read AND starred AND summarized.
    // Timestamps for read_at / starred_at also correlate with id so each sort
    // criterion (PublishedAt / ReadAt / StarredAt) yields the same row order
    // — otherwise the API's `e.id < c` continuation could legitimately skip rows.
    readIds.forEach((entryId, idx) => {
      const i = idx + 1;
      const offset = `-${PER_FEED - i + 1} minutes`;
      seed.markRead(entryId, offset);
      seed.markStarred(entryId, offset);
      seed.insertSummary(entryId, userId, `Summary ${i}`);
    });
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("loadmoreuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  async function expectNoDuplicatesAfterLoadMore(
    page: Page,
    fullUrl: string
  ): Promise<void> {
    await page.goto(fullUrl);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    const beforeIds = await page
      .getByTestId("entry-item")
      .evaluateAll((els) => els.map((el) => el.id));
    expect(new Set(beforeIds).size).toBe(beforeIds.length);

    const loadMore = page.getByTestId("load-more-btn");
    await expect(loadMore).toBeVisible();
    await loadMore.click();

    // Wait for the second page to land (entry count grows).
    await expect
      .poll(() => page.getByTestId("entry-item").count())
      .toBeGreaterThan(beforeIds.length);

    const afterIds = await page
      .getByTestId("entry-item")
      .evaluateAll((els) => els.map((el) => el.id));

    // No duplicates — primary regression assertion.
    expect(new Set(afterIds).size).toBe(afterIds.length);
    // Initial entries are still present (Load More appended, didn't reset).
    for (const id of beforeIds) {
      expect(afterIds).toContain(id);
    }
  }

  test("/ (unread)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(page, `${serverUrl}/`);
  });

  test("/entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(page, `${serverUrl}/entries`);
  });

  test("/entries/read", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(page, `${serverUrl}/entries/read`);
  });

  test("/entries/starred", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(page, `${serverUrl}/entries/starred`);
  });

  test("/entries/summarized", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(
      page,
      `${serverUrl}/entries/summarized`
    );
  });

  test("/categories/:id/entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(
      page,
      `${serverUrl}/categories/${unreadCategoryId}/entries`
    );
  });

  test("/feeds/:id/entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(
      page,
      `${serverUrl}/feeds/${unreadFeedId}/entries`
    );
  });

  test("/search?q=TestEntry", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expectNoDuplicatesAfterLoadMore(
      page,
      `${serverUrl}/search?q=TestEntry`
    );
  });
});

/**
 * Regression: composite cursor (#164) must surface entries with high ids
 * but old timestamps on Load More. The legacy `e.id < c` cursor silently
 * skipped these, hiding back-dated / re-imported entries from the user.
 */
test.describe("Load More surfaces back-dated entries (composite cursor #164)", () => {
  const PER_PAGE = 30; // matches default entries_per_page

  test.beforeAll(async ({ api, seed }) => {
    await api.register("backdateduser", "password123");
    const userId = seed.getUserId("backdateduser");
    const catId = seed.createCategory(userId, "Backdated Cat");
    const feedId = seed.createFeed(
      catId,
      "https://example.com/backdated.xml",
      "Backdated Feed"
    );

    // Page 1 fill: PER_PAGE recent entries (older ids = older timestamps,
    // newest-first ordering means they sort to the top of page 1).
    const recent = Array.from({ length: PER_PAGE }, (_, idx) => {
      const i = idx + 1;
      return {
        feedId,
        guid: `recent-${i}`,
        title: `Recent ${i}`,
        link: `https://example.com/recent/${i}`,
        content: `<p>Recent ${i}</p>`,
        publishedOffset: `-${PER_PAGE - i + 1} hours`,
      };
    });

    // Back-dated: 3 entries with NEW high ids but OLD timestamps. These
    // would be silently skipped by the legacy `e.id < c` cursor on page 2.
    const backdated = [1, 2, 3].map((i) => ({
      feedId,
      guid: `bd-${i}`,
      title: `Backdated ${i}`,
      link: `https://example.com/bd/${i}`,
      content: `<p>Backdated ${i}</p>`,
      publishedOffset: `-${30 + i} days`,
    }));

    // Insert recent first so back-dated rows get higher ids.
    seed.insertEntries(recent);
    seed.insertEntries(backdated);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("backdateduser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  test("Load More on / shows back-dated entries", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.goto(`${serverUrl}/`);

    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    const beforeCount = await page.getByTestId("entry-item").count();
    expect(beforeCount).toBe(PER_PAGE);

    // None of the back-dated entries should be on page 1 (they're older).
    for (const i of [1, 2, 3]) {
      await expect(page.getByText(`Backdated ${i}`, { exact: true })).not.toBeVisible();
    }

    await page.getByTestId("load-more-btn").click();
    await expect
      .poll(() => page.getByTestId("entry-item").count())
      .toBeGreaterThan(beforeCount);

    // All 3 back-dated entries must appear after Load More.
    for (const i of [1, 2, 3]) {
      await expect(page.getByText(`Backdated ${i}`, { exact: true })).toBeVisible();
    }
  });
});
