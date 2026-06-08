import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { When, Then } = createBdd(test);

When("I open the landing page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/`);
});

When("I am on the settings page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/settings`);
});

Then("the category dropdown offers {string}", async ({ page }, label) => {
  await expect(page.getByTestId("feed-category-select")).toContainText(label);
});

Then("the landing page shows the getting-started guide", async ({ page }) => {
  await expect(page.getByTestId("onboarding-guide")).toBeVisible();
});

Then("the landing page does not show the getting-started guide", async ({ page }) => {
  await expect(page.getByTestId("onboarding-guide")).toHaveCount(0);
});

Then("I see an {string} call to action", async ({ page }, label) => {
  await expect(
    page.getByTestId("onboarding-guide").getByRole("link", { name: label })
  ).toBeVisible();
});

Then("I see {string} on the landing page", async ({ page }, text) => {
  await expect(page.getByText(text)).toBeVisible();
});

Then("I see the active WebAuthn RP origin", async ({ page }) => {
  const origin = page.getByTestId("webauthn-rp-origin");
  await expect(origin).toBeVisible();
  await expect(origin).toContainText("http");
});
