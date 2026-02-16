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

    // Wait for the flash message
    await expect(page.getByTestId("flash-message")).toContainText(
      "Category created"
    );

    // Category should appear in the table
    await expect(page.getByTestId("categories-table")).toContainText(
      "New Test Category"
    );
  });

  test("rename category updates name", async ({ page }) => {
    // First create a category
    await page.getByTestId("category-name-input").fill("Rename Me");
    await page.getByTestId("add-category-btn").click();
    await expect(page.getByTestId("flash-message")).toContainText(
      "Category created"
    );

    // Click rename link
    await page.getByRole("link", { name: "rename" }).first().click();

    // Edit inline input
    const editInput = page.locator(".cat-edit-input").first();
    await expect(editInput).toBeVisible();
    await editInput.fill("Renamed Category");

    // Click save
    await page.getByRole("link", { name: "save" }).first().click();

    await expect(page.getByTestId("flash-message").last()).toContainText(
      "Category renamed"
    );
    await expect(page.getByTestId("categories-table")).toContainText(
      "Renamed Category"
    );
  });

  test("delete category removes from list", async ({ page }) => {
    // Create a category to delete
    await page.getByTestId("category-name-input").fill("Delete Me");
    await page.getByTestId("add-category-btn").click();
    await expect(page.getByTestId("flash-message")).toContainText(
      "Category created"
    );

    // Accept the confirm dialog
    page.on("dialog", (dialog) => dialog.accept());

    // Click delete
    await page.getByRole("link", { name: "delete" }).first().click();

    await expect(page.getByTestId("flash-message").last()).toContainText(
      "deleted"
    );
  });
});
