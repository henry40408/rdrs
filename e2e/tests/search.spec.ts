import { test, expect } from "../fixtures/rdrs.js";

test.describe("Search", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("searchuser", "password123");

    const userId = seed.getUserId("searchuser");
    const categoryId = seed.createCategory(userId, "Search Category");
    const feedId = seed.createFeed(categoryId, "https://example.com/search-feed.xml", "Search Feed");

    // Seed entries with distinct titles for search testing
    seed.insertEntries([
      {
        feedId,
        guid: "search-1",
        title: "Rust Programming Guide",
        link: "https://example.com/rust",
        content: "<p>Learn Rust programming language</p>",
        publishedOffset: "-1 hours",
      },
      {
        feedId,
        guid: "search-2",
        title: "JavaScript Frameworks",
        link: "https://example.com/js",
        content: "<p>Comparing JavaScript frameworks</p>",
        publishedOffset: "-2 hours",
      },
      {
        feedId,
        guid: "search-3",
        title: "Rust Async Runtime",
        link: "https://example.com/async",
        content: "<p>Understanding async in Rust</p>",
        publishedOffset: "-3 hours",
      },
    ]);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("searchuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    // Navigate to search page
    await page.goto(`${serverUrl}/search`);
  });

  test("search by term shows matching entries", async ({ page }) => {
    await page.getByTestId("search-input").fill("Rust");
    await page.keyboard.press("Enter");

    // Should show entries matching "Rust"
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    const count = await page.getByTestId("entry-item").count();
    expect(count).toBe(2); // "Rust Programming Guide" and "Rust Async Runtime"
  });

  test("/ shortcut focuses search input", async ({ page }) => {
    // Click somewhere else first to unfocus search input
    await page.click("body");

    // Wait for the search input to not be focused
    await expect(page.getByTestId("search-input")).not.toBeFocused();

    await page.keyboard.press("/");
    await expect(page.getByTestId("search-input")).toBeFocused();
  });
});
