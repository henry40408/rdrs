import { createBdd } from "playwright-bdd";
import { test } from "../support/fixtures.js";

const { When } = createBdd(test);

When("I visit {string}", async ({ page, serverUrl }, path) => {
  await page.goto(`${serverUrl}${path}`);
});
