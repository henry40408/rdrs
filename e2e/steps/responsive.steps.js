import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

const VIEWPORTS = {
  mobile: { width: 375, height: 667 },
  tablet: { width: 768, height: 1024 },
  desktop: { width: 1280, height: 800 },
  wide: { width: 1400, height: 900 },
};

Given("I am viewing on a {word} screen", async ({ page }, kind) => {
  const v = VIEWPORTS[kind];
  if (!v) throw new Error(`Unknown viewport: ${kind}`);
  await page.setViewportSize(v);
});

Given("I have a feed with {int} test entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, `Cat-${currentUser.username}`);
  const feedId = seed.createFeed(categoryId, `https://example.com/${currentUser.username}.xml`, "Mobile Feed");
  seed.seedTestEntries(feedId, count);
});

Then("the entry-row actions are vertically centered on the meta line", async ({ page }) => {
  // The star + open-original cluster (.rail-actions) and the feed meta line
  // (.entry-item-meta) share grid row 2; their vertical centers must coincide.
  // The old absolute-overlay positioning used a hand-tuned `bottom` offset that
  // drifted on mobile (meta grew via the feed-link tap padding), leaving the
  // actions several px low. align-self:center makes them coincide at any size.
  const row = page.getByTestId("entry-item").first();
  const delta = await row.evaluate((n) => {
    const c = (sel) => {
      const r = n.querySelector(sel).getBoundingClientRect();
      return r.y + r.height / 2;
    };
    return Math.abs(c(".rail-actions") - c(".entry-item-meta"));
  });
  expect(delta).toBeLessThanOrEqual(1.5);
});

Given("I have read entries across several days", async ({ seed, currentUser }) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, `Cat-${currentUser.username}`);
  const feedId = seed.createFeed(
    categoryId,
    `https://example.com/${currentUser.username}.xml`,
    "Stats Feed",
  );
  // One read entry per day across the default 7-day window so the chart
  // renders a full row of bars (incl. the rightmost, whose tooltip is what
  // used to overflow the viewport).
  const ids = seed.seedTestEntries(feedId, 8);
  ids.forEach((id, dayOffset) => seed.markRead(id, `-${dayOffset} days`));
});

When("I open the inbox", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/`);
});

Given("I have read entries spanning several weeks", async ({ seed, currentUser }) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, `Cat-${currentUser.username}`);
  const feedId = seed.createFeed(
    categoryId,
    `https://example.com/${currentUser.username}.xml`,
    "Stats Feed",
  );
  // One read entry per day across ~30 days so a 90d range has plenty of
  // activity to bucket into bars.
  const ids = seed.seedTestEntries(feedId, 30);
  ids.forEach((id, dayOffset) => seed.markRead(id, `-${dayOffset} days`));
});

When("I open the statistics page for the {string} period", async ({ page, serverUrl }, period) => {
  await page.goto(`${serverUrl}/statistics?period=${period}`);
});

When("I hover the last daily-read bar", async ({ page }) => {
  await page.locator(".stats-bar-col").last().hover();
});

When("I hover daily-read bar number {int}", async ({ page }, n) => {
  await page.locator(".stats-bar-col").nth(n - 1).hover();
});

Then("the visible daily-read tooltip is within the viewport", async ({ page }) => {
  const rect = await page.evaluate(() => {
    const tip = [...document.querySelectorAll(".stats-bar-tip")].find(
      (t) => getComputedStyle(t).visibility === "visible",
    );
    if (!tip) return null;
    const r = tip.getBoundingClientRect();
    return { left: r.left, right: r.right, vw: document.documentElement.clientWidth };
  });
  expect(rect, "a tooltip should be visible").not.toBeNull();
  expect(rect.left).toBeGreaterThanOrEqual(-0.5);
  expect(rect.right).toBeLessThanOrEqual(rect.vw + 0.5);
});

Then("the daily-read chart is visible", async ({ page }) => {
  await expect(page.locator(".stats-chart")).toBeVisible();
});

Then("the daily-read chart has at most {int} bars", async ({ page }, max) => {
  const count = await page.locator(".stats-bar-col").count();
  expect(count).toBeGreaterThan(0);
  expect(count).toBeLessThanOrEqual(max);
});

Then("some daily-read axis labels are hidden", async ({ page }) => {
  const total = await page.locator(".stats-bar-label").count();
  const visible = await page.locator(".stats-bar-label:visible").count();
  expect(total).toBeGreaterThan(0);
  expect(visible).toBeLessThan(total);
});

Then("the daily-read bars are each at least {int}px wide", async ({ page }, min) => {
  const cols = await page.locator(".stats-bar-col").all();
  expect(cols.length).toBeGreaterThan(0);
  for (const col of cols) {
    const box = await col.boundingBox();
    expect(box).not.toBeNull();
    expect(box.width).toBeGreaterThanOrEqual(min);
  }
});

Then("the page has no horizontal scroll", async ({ page }) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow).toBeLessThanOrEqual(0);
});

When("I open the categories page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/categories`);
});

When("I open the all-entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries`);
});

When("I open the feeds page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds`);
});

When("I open the import page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/feeds/import`);
});

When("I open the edit page for feed {string}", async ({ page }, feedTitle) => {
  await page
    .getByTestId("feeds-table")
    .locator("tr")
    .filter({ hasText: feedTitle })
    .getByRole("link", { name: "edit" })
    .click();
});

When("I expand the {string} disclosure", async ({ page }, label) => {
  await page.locator("summary", { hasText: label }).click();
});

When("I tap the hamburger", async ({ page }) => {
  await page.locator(".sidebar-toggle").click();
});

When("I tap the sidebar close button", async ({ page }) => {
  await page.locator(".sidebar-close").click();
});

When("I tap outside the sidebar", async ({ page }) => {
  // Click the dimmed area on the right half of the viewport, well clear of
  // the left-anchored drawer — exercises the document-level tap-outside close.
  const v = page.viewportSize();
  await page.mouse.click(v.width - 10, Math.floor(v.height / 2));
});

Then("the sidebar is visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toHaveClass(/open/);
});

Then("the sidebar is not visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).not.toHaveClass(/open/);
});

Then("the hamburger button is visible", async ({ page }) => {
  await expect(page.locator(".sidebar-toggle")).toBeVisible();
});

Then("the entry list pane is at least {int}px wide", async ({ page }, minWidth) => {
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeGreaterThanOrEqual(minWidth);
});

Then("the categories table is shown as cards", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).toHaveCSS("display", "none");
  await expect(page.locator("table.mobile-cards td[data-label]").first()).toBeVisible();
});

Then("the categories table is shown as a table", async ({ page }) => {
  await expect(page.locator("table.mobile-cards thead")).not.toHaveCSS("display", "none");
});

Then("the sidebar is always-visible", async ({ page }) => {
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator(".sidebar-toggle")).toBeHidden();
});

Then("the entry list pane is narrower than the viewport", async ({ page }) => {
  const viewport = page.viewportSize();
  const box = await page.locator(".list-pane").boundingBox();
  expect(box.width).toBeLessThan(viewport.width * 0.9);
});

Then("the reading pane is visible on mobile", async ({ page }) => {
  // At ≤1024px width the reading pane is `display: none` by default and only
  // surfaces when the `.reading-pane-active` overlay class is present. Assert
  // both the class is applied AND the element is actually visible — class
  // alone would pass even if a future CSS regression unset `display: block`.
  const pane = page.locator("#reading-pane");
  await expect(pane).toHaveClass(/reading-pane-active/);
  await expect(pane).toBeVisible();
});

When("I tap the reading-pane back button", async ({ page }) => {
  await page.getByTestId("reading-pane-back").click();
});

When("a flash banner is shown", async ({ page }) => {
  // Drive the page-level <rdrs-flash> API directly — same entry point the
  // app's own JS uses (window.flash is installed by rdrs-flash.js).
  await page.evaluate(() =>
    window.flash.show("success", "Marked older than 1 week entries as read.")
  );
  await expect(page.locator(".banner")).toBeVisible();
});

Then(
  "the flash banner is vertically centered on a wide touch tablet",
  async ({ browser, page, serverUrl }) => {
    // Reproduce iPad-landscape: a WIDE (>1024px, so the persistent split layout
    // rather than the mobile drawer) TOUCH viewport. Touch triggers
    // `@media (hover: none)`, which bumps `.banner-dismiss` to 44px tall; the
    // base `.banner { align-items: start }` then pinned the message to the top
    // of the inflated grid row while the `align-self: center` timestamp sat
    // lower — the visible misalignment + blank space. `hover: none` cannot be
    // faked via viewport size or page.emulateMedia, so spin a dedicated
    // hasTouch context (reusing the signed-in cookies) at 1180px.
    const ctx = await browser.newContext({
      viewport: { width: 1180, height: 820 },
      hasTouch: true,
    });
    try {
      await ctx.addCookies(await page.context().cookies());
      const tp = await ctx.newPage();
      await tp.goto(`${serverUrl}/`);
      await tp.evaluate(() => window.flash.show("success", "Marked as unread."));
      const banner = tp.locator(".banner").first();
      await expect(banner).toBeVisible();
      const m = await banner.evaluate((n) => {
        const centerY = (sel) => {
          const r = n.querySelector(sel).getBoundingClientRect();
          return r.y + r.height / 2;
        };
        const d = n.querySelector(".banner-dismiss").getBoundingClientRect();
        return {
          msg: centerY(".banner-message"),
          time: centerY(".banner-time"),
          dismissW: d.width,
          dismissH: d.height,
        };
      });
      // Message and timestamp share the row's vertical center.
      expect(Math.abs(m.msg - m.time)).toBeLessThanOrEqual(1.5);
      // Dismiss keeps a full 44px tap target on BOTH axes at this width.
      expect(m.dismissW).toBeGreaterThanOrEqual(44);
      expect(m.dismissH).toBeGreaterThanOrEqual(44);
    } finally {
      await ctx.close();
    }
  },
);

Then("the flash banner sits below the hamburger", async ({ page }) => {
  const toggle = await page.locator(".sidebar-toggle").boundingBox();
  const banner = await page.locator(".banner").first().boundingBox();
  // Below the button (no overlap)…
  expect(banner.y).toBeGreaterThanOrEqual(toggle.y + toggle.height);
  // …and full-width: the banner's left edge reaches past the floating
  // button's left edge instead of being indented to clear it.
  expect(banner.x).toBeLessThan(toggle.x);
});

Then("the reading pane overlay is dismissed", async ({ page }) => {
  // closeReadingPane() strips .reading-pane-active and restores the empty
  // placeholder; at ≤1024px the pane without the active class is
  // `display: none`, so it must be both class-free AND actually hidden.
  const pane = page.locator("#reading-pane");
  await expect(pane).not.toHaveClass(/reading-pane-active/);
  await expect(pane).toBeHidden();
});

Then("the {string} control is at least {int}px tall", async ({ page }, selector, min) => {
  const box = await page.locator(selector).first().boundingBox();
  expect(box).not.toBeNull();
  expect(box.height).toBeGreaterThanOrEqual(min);
});

Then("the {string} control is at least {int}px wide", async ({ page }, selector, min) => {
  const box = await page.locator(selector).first().boundingBox();
  expect(box).not.toBeNull();
  expect(box.width).toBeGreaterThanOrEqual(min);
});

Then("the {string} element is visible", async ({ page }, selector) => {
  await expect(page.locator(selector).first()).toBeVisible();
});

Then("the entry-list filter bar does not overflow the list pane", async ({ page }) => {
  // The filter bar holds the status-filter select, the Mark-as-Read select, and
  // the search box, and uses `flex-wrap`, so inside the fixed-width list pane it
  // may legitimately wrap onto a second row — that is by design and not what
  // this guards. The real invariant is that nothing overflows the pane
  // horizontally: no control reaches past the pane's right edge and the pane
  // grows no horizontal scrollbar, so every control stays reachable however many
  // the bar holds.
  const pane = page.locator(".list-pane");
  await expect(pane).toBeVisible();
  const overflow = await pane.evaluate((p) => {
    const paneRight = p.getBoundingClientRect().right;
    const groups = [...p.querySelectorAll(".filter-bar > .form-group")];
    const pastRight = Math.max(
      0,
      ...groups.map((g) => g.getBoundingClientRect().right - paneRight),
    );
    return {
      count: groups.length,
      horizontalScroll: p.scrollWidth - p.clientWidth,
      pastRight,
    };
  });
  expect(overflow.count).toBeGreaterThanOrEqual(2);
  // Sub-pixel tolerance for rounding.
  expect(overflow.pastRight).toBeLessThanOrEqual(1);
  expect(overflow.horizontalScroll).toBeLessThanOrEqual(1);
});
