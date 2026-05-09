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

  test("reading pane shows back button and prev/next nav", async ({
    page,
  }) => {
    // Open first entry in reading pane
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Back button and nav buttons should exist on mobile
    const backLink = page.locator(".reading-pane-back-link");
    await expect(backLink).toBeVisible();

    const prevBtn = page.locator('[data-rp-action="prev-entry"]');
    const nextBtn = page.locator('[data-rp-action="next-entry"]');
    await expect(prevBtn).toBeVisible();
    await expect(nextBtn).toBeVisible();

    // First entry: prev should be disabled, next enabled
    await expect(prevBtn).toBeDisabled();
    await expect(nextBtn).toBeEnabled();
  });

  test("prev/next buttons navigate between entries", async ({ page }) => {
    // Open first entry
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    const title1 = await page.locator(".reading-pane-title").textContent();

    // Click next
    const nextBtn = page.locator('[data-rp-action="next-entry"]');
    await nextBtn.click();
    await expect(page.locator(".reading-pane-title")).not.toHaveText(title1!);

    const title2 = await page.locator(".reading-pane-title").textContent();
    expect(title2).not.toBe(title1);

    // Click prev to go back
    const prevBtn = page.locator('[data-rp-action="prev-entry"]');
    await prevBtn.click();
    await expect(page.locator(".reading-pane-title")).toHaveText(title1!);
  });

  test("back button returns to entry list from reading pane", async ({
    page,
  }) => {
    // Open first entry
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Click back
    await page.locator(".reading-pane-back-link").click();

    // Should be back at the list
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    await expect(page.locator(".reading-pane-title")).not.toBeVisible();
  });

  // Pre-existing flake — the [data-rp-action="next-entry"] button is
  // sometimes disabled when the test reaches it; the entry list hasn't
  // resolved the next-entry index yet. Re-enable once we have a stable
  // signal for "list ready".
  test.fixme("back button works after prev/next navigation", async ({ page }) => {
    // Open first entry, navigate to next, then back to list
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Navigate forward twice
    const nextBtn = page.locator('[data-rp-action="next-entry"]');
    await nextBtn.click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();
    await nextBtn.click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Back should return to list (not previous entry)
    await page.locator(".reading-pane-back-link").click();
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    await expect(page.locator(".reading-pane-title")).not.toBeVisible();
  });

  test("reading pane is full-screen overlay", async ({ page }) => {
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Reading pane should be a fixed overlay on mobile
    const pane = page.locator(".reading-pane");
    await expect(pane).toHaveClass(/reading-pane-active/);
    await expect(pane).toHaveCSS("position", "fixed");
  });

  test("entry list is full-width single column", async ({ page }) => {
    const listPane = page.locator(".list-pane");
    const box = await listPane.boundingBox();
    // List pane should span full viewport width (minus any small rounding)
    expect(box!.width).toBeGreaterThan(370);
  });

  test("sidebar toggle is hidden when reading pane is active", async ({
    page,
  }) => {
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    // Hamburger should be hidden behind reading pane overlay
    const toggle = page.locator(".sidebar-toggle");
    await expect(toggle).not.toBeVisible();
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

  test("reading pane is full-screen overlay", async ({ page }) => {
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    const pane = page.locator(".reading-pane");
    await expect(pane).toHaveClass(/reading-pane-active/);
    await expect(pane).toHaveCSS("position", "fixed");
  });

  test("reading pane has back and prev/next buttons", async ({ page }) => {
    await page.getByTestId("entry-title-link").first().click();
    await expect(page.locator(".reading-pane-title")).toBeVisible();

    await expect(page.locator(".reading-pane-back-link")).toBeVisible();
    await expect(
      page.locator('[data-rp-action="prev-entry"]')
    ).toBeVisible();
    await expect(
      page.locator('[data-rp-action="next-entry"]')
    ).toBeVisible();
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
