import { test, expect } from "../fixtures/rdrs.js";

test.describe("Authentication", () => {
  test("register → redirect to login → flash success → login → home page", async ({
    page,
    serverUrl,
  }) => {
    // Go to register page
    await page.goto(`${serverUrl}/register`);
    await expect(page.getByTestId("register-form")).toBeVisible();

    // Fill and submit registration
    await page.getByTestId("register-username").fill("testuser");
    await page.getByTestId("register-password").fill("password123");
    await page.getByTestId("register-confirm-password").fill("password123");
    await page.getByTestId("register-submit").click();

    // Should redirect to login page with success flash
    await page.waitForURL(`${serverUrl}/login`);
    await expect(page.getByTestId("flash-message")).toBeVisible();
    await expect(page.getByTestId("flash-message")).toContainText(
      "Registration successful"
    );

    // Login with the new account
    await page.getByTestId("username-input").fill("testuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();

    // Should arrive at home page (unread)
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("main-nav")).toBeVisible();
  });

  test("wrong password shows error", async ({ page, serverUrl }) => {
    // First register a user
    await page.goto(`${serverUrl}/register`);
    await page.getByTestId("register-username").fill("testuser2");
    await page.getByTestId("register-password").fill("password123");
    await page.getByTestId("register-confirm-password").fill("password123");
    await page.getByTestId("register-submit").click();
    await page.waitForURL(`${serverUrl}/login`);

    // Try to login with wrong password
    await page.getByTestId("username-input").fill("testuser2");
    await page.getByTestId("password-input").fill("wrongpassword");
    await page.getByTestId("login-submit").click();

    // Should show error message
    await expect(page.getByTestId("login-error")).toBeVisible();
  });

  test("password mismatch shows client-side error", async ({
    page,
    serverUrl,
  }) => {
    await page.goto(`${serverUrl}/register`);

    await page.getByTestId("register-username").fill("testuser3");
    await page.getByTestId("register-password").fill("password123");
    await page.getByTestId("register-confirm-password").fill("different456");
    await page.getByTestId("register-submit").click();

    // Should show client-side validation error
    await expect(page.getByTestId("register-error")).toBeVisible();
    await expect(page.getByTestId("register-error")).toContainText(
      "Passwords do not match"
    );

    // Should stay on register page
    expect(page.url()).toContain("/register");
  });
});
