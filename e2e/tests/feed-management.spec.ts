import { test, expect } from "../fixtures/rdrs.js";

test.describe("Feed Management", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("feeduser", "password123");

    const userId = seed.getUserId("feeduser");
    seed.createCategory(userId, "Feed Test Category");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("feeduser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    await page.goto(`${serverUrl}/feeds`);
    // Wait for category dropdown to be populated
    await expect(page.getByTestId("feed-category-select")).not.toContainText(
      "Loading"
    );
  });

  test("add feed appears in feeds table", async ({ page, feedServerUrl }) => {
    await page.getByTestId("feed-url-input").fill(`${feedServerUrl}/feed.xml`);
    await page.getByTestId("feed-category-select").selectOption({ index: 0 });
    await page.getByTestId("add-feed-btn").click();

    // Wait for success flash
    await expect(page.getByTestId("flash-message")).toContainText("Feed added");

    // Feed should appear in the table
    await expect(page.getByTestId("feeds-table")).toContainText(
      "Test Feed"
    );
  });
});
