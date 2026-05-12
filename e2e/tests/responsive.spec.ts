import { test, expect } from "../fixtures/rdrs.js";

// ── Mobile tests (375×667) ──────────────────────────────────────────

test.describe("Mobile layout", () => {
  test.use({ viewport: { width: 375, height: 667 } });

  test.beforeAll(async ({ api, seed }) => {
    await api.register("mobileuser", "password123");

    const userId = seed.getUserId("mobileuser");
    const categoryId = seed.createCategory(userId, "Mobile Category");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/mobile-feed.xml",
      "Mobile Feed"
    );

    seed.seedTestEntries(feedId, 5);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("mobileuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
  });

  test("sidebar is hidden by default, hamburger toggles it", async ({
    page,
  }) => {
    const sidebar = page.locator("#sidebar");
    const toggle = page.locator(".sidebar-toggle");

    // Sidebar should be off-screen, hamburger visible
    await expect(toggle).toBeVisible();
    await expect(sidebar).not.toHaveClass(/open/);

    // Click hamburger to open
    await toggle.click();
    await expect(sidebar).toHaveClass(/open/);

    // Close button inside sidebar
    const closeBtn = page.locator(".sidebar-close");
    await expect(closeBtn).toBeVisible();
    await closeBtn.click();
    await expect(sidebar).not.toHaveClass(/open/);
  });

  test("entry list is full-width single column", async ({ page }) => {
    const listPane = page.locator(".list-pane");
    const box = await listPane.boundingBox();
    // List pane should span full viewport width (minus any small rounding)
    expect(box!.width).toBeGreaterThan(370);
  });

});

// ── Phone card layout tests (375×667) ───────────────────────────────

test.describe("Phone table card layout", () => {
  test.use({ viewport: { width: 375, height: 667 } });

  test.beforeAll(async ({ api, seed }) => {
    await api.register("phonecardsuser", "password123");

    const userId = seed.getUserId("phonecardsuser");
    seed.createCategory(userId, "Card Category");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("phonecardsuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  });

  test("categories table renders as cards on phone", async ({
    page,
    serverUrl,
  }) => {
    await page.goto(`${serverUrl}/categories`);
    await expect(page.locator("table.mobile-cards")).toBeVisible();

    // At 375px (<=600px), thead should be visually hidden
    const thead = page.locator("table.mobile-cards thead");
    await expect(thead).toHaveCSS("display", "none");

    // Table cells should have data-label attributes for card layout
    const firstCell = page.locator("table.mobile-cards td[data-label]").first();
    await expect(firstCell).toBeVisible();
  });
});

// ── Tablet tests (768×1024) ─────────────────────────────────────────

test.describe("Tablet layout", () => {
  test.use({ viewport: { width: 768, height: 1024 } });

  test.beforeAll(async ({ api, seed }) => {
    await api.register("tabletuser", "password123");

    const userId = seed.getUserId("tabletuser");
    const categoryId = seed.createCategory(userId, "Tablet Category");
    const feedId = seed.createFeed(
      categoryId,
      "https://example.com/tablet-feed.xml",
      "Tablet Feed"
    );

    seed.seedTestEntries(feedId, 5);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("tabletuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
  });

  test("sidebar is drawer mode, not always-visible", async ({ page }) => {
    const sidebar = page.locator("#sidebar");
    const toggle = page.locator(".sidebar-toggle");

    // At 768px (<=1024px), sidebar should be collapsed as drawer
    await expect(toggle).toBeVisible();
    await expect(sidebar).not.toHaveClass(/open/);

    // Open and verify it overlays
    await toggle.click();
    await expect(sidebar).toHaveClass(/open/);
  });

  test("tables remain in table layout at tablet width", async ({
    page,
    serverUrl,
  }) => {
    await page.goto(`${serverUrl}/categories`);
    await expect(page.locator("table.mobile-cards")).toBeVisible();

    // At 768px (>600px), thead should still be visible (not card layout)
    const thead = page.locator("table.mobile-cards thead");
    await expect(thead).not.toHaveCSS("display", "none");
  });

  test("entry list is single-column full-width", async ({ page }) => {
    const listPane = page.locator(".list-pane");
    const box = await listPane.boundingBox();
    // At tablet width, list should fill the viewport
    expect(box!.width).toBeGreaterThan(760);
  });
});
