import { test, expect } from "../fixtures/rdrs.js";

test.describe("Theme Switching", () => {
  test.beforeAll(async ({ api }) => {
    await api.register("themeuser", "password123");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("themeuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);

    await page.goto(`${serverUrl}/user-settings`);
    await expect(page.getByTestId("theme-select")).toBeVisible();
  });

  test("switch to dark theme sets data-theme attribute", async ({ page }) => {
    await page.getByTestId("theme-select").selectOption("dark");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  });

  test("switch to light theme sets data-theme attribute", async ({ page }) => {
    await page.getByTestId("theme-select").selectOption("light");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  });

  test("switch to system removes data-theme attribute", async ({ page }) => {
    // First set to dark
    await page.getByTestId("theme-select").selectOption("dark");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

    // Then switch to system
    await page.getByTestId("theme-select").selectOption("system");
    await expect(page.locator("html")).not.toHaveAttribute("data-theme");
  });
});
