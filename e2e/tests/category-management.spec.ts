import { test, expect } from "../fixtures/rdrs.js";

test.describe("Category Management", () => {
  test.beforeAll(async ({ api }) => {
    await api.register("catuser", "password123");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("catuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    await page.goto(`${serverUrl}/categories`);
    await expect(page.getByTestId("category-name-input")).toBeVisible();
  });

  test("add category appears in list", async ({ page }) => {
    await page.getByTestId("category-name-input").fill("New Test Category");
    await page.getByTestId("add-category-btn").click();

    await expect(page.getByTestId("flash-message").first()).toContainText(
      "Category created"
    );

    // Each row's name now lives in <input value="..."> inside a rename form.
    await expect(
      page
        .getByTestId("categories-table")
        .locator('input[value="New Test Category"]')
    ).toBeVisible();
  });

  test("rename category updates name", async ({ page }) => {
    // Create a category first
    await page.getByTestId("category-name-input").fill("Rename Me");
    await page.getByTestId("add-category-btn").click();
    await expect(page.getByTestId("flash-message").first()).toContainText(
      "Category created"
    );

    // Rename via the inline form (each row is its own POST form).
    const row = page
      .getByTestId("categories-table")
      .locator("tr", { has: page.locator('input[value="Rename Me"]') });
    const input = row.locator('input[name="name"]');
    await input.fill("Renamed Category");
    await row.getByRole("button", { name: "save" }).click();

    await expect(page.getByTestId("flash-message").last()).toContainText(
      "Category renamed"
    );
    await expect(
      page
        .getByTestId("categories-table")
        .locator('input[value="Renamed Category"]')
    ).toBeVisible();
  });

  test("delete category removes from list", async ({ page }) => {
    await page.getByTestId("category-name-input").fill("Delete Me");
    await page.getByTestId("add-category-btn").click();
    await expect(page.getByTestId("flash-message").first()).toContainText(
      "Category created"
    );

    // Inline `onsubmit="return confirm(...)"` shows a native dialog;
    // accept it before submit.
    page.on("dialog", (dialog) => dialog.accept());

    const row = page
      .getByTestId("categories-table")
      .locator("tr", { has: page.locator('input[value="Delete Me"]') });
    await row.getByRole("button", { name: "delete" }).click();

    await expect(page.getByTestId("flash-message").last()).toContainText(
      "deleted"
    );
  });
});
