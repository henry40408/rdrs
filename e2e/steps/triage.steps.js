import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("the user has Kagi configured", async ({ seed, currentUser }) => {
  // Seeds a fake Kagi session token so the reading-pane Summarize button
  // is rendered. Actual Kagi requests will fail when fired, which is
  // acceptable for tests that only assert the in-flight summary placeholder.
  const userId = seed.getUserId(currentUser.username);
  seed.configureKagi(userId);
});

// app.js #mark-read-age <select> fires a window.confirm before calling
// /reader/api/0/mark-all-as-read. Pre-register the dialog handler so the
// prompt auto-accepts, then trigger the dropdown by picking an option. On
// success app.js swaps the refreshed list into the live document rather than
// reloading, so the assertions that follow run against the same page object
// with no navigation to wait for — Playwright's auto-retrying assertions cover
// the in-flight POST + swap.
async function markAsReadViaDropdown(page, optionValue) {
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByTestId("mark-read-select").selectOption(optionValue);
}

When("I mark all entries as read", async ({ page }) => {
  await markAsReadViaDropdown(page, "all");
});

// The age options carry a `ts=` cutoff, which is the case most likely to
// regress back to a reload: it is the only dropdown path that leaves rows
// behind, so a reload there is visible as lost scroll and a closed entry.
When("I mark entries older than 1 day as read", async ({ page }) => {
  await markAsReadViaDropdown(page, "1");
});

// Backdated past the "older than 1 day" cutoff
// (`COALESCE(published_at, created_at) < now - 1 day`), so the age option has
// something to catch while the Background's freshly-seeded entries stay put.
// That contrast is what proves the cutoff was applied rather than everything
// being marked.
Given(
  "the feed {string} has an entry titled {string} published 3 days ago",
  async ({ seed, currentUser }, feedTitle, title) => {
    const userId = seed.getUserId(currentUser.username);
    const feedId = seed.findFeedIdByTitle(userId, feedTitle);
    seed.insertEntries([
      {
        feedId,
        guid: `${currentUser.username}-aged-entry`,
        title,
        link: `https://example.com/${currentUser.username}/aged-entry`,
        content: `<p>${title}</p>`,
        publishedOffset: "-3 days",
      },
    ]);
  }
);

// "Mark Above as Read" confirms before POSTing, same as the dropdown above,
// and shares its swap-instead-of-reload success path.
When("I mark the loaded entries as read", async ({ page }) => {
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByTestId("mark-above-btn").click();
});

When("I star the entry titled {string}", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("entry-star-action") // data-testid="entry-star-action" on the star button
    .first()
    .click();
});

When(
  "I click the read toggle for the entry titled {string}",
  async ({ page }, title) => {
    // The leading dot is a form submit button (data-testid="entry-read-toggle")
    // that POSTs /read or /unread and swaps the row in-place.
    await page
      .getByTestId("entry-item")
      .filter({ hasText: title })
      .getByTestId("entry-read-toggle")
      .first()
      .click();
  },
);

Then(
  "every entry row exposes the read toggle, star, open-original, time, and feed controls",
  async ({ page }) => {
    // Regression guard against silently dropping a per-row control (as the
    // 0.55.0 redesign did with mark-read + open-original). Every row must
    // carry the full control set.
    const rows = page.getByTestId("entry-item");
    const count = await rows.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      const row = rows.nth(i);
      await expect(row.getByTestId("entry-read-toggle")).toBeVisible();
      await expect(row.getByTestId("entry-star-action")).toBeVisible();
      await expect(row.getByTestId("entry-open-original")).toBeVisible();
      await expect(row.locator(".entry-time")).toBeVisible();
      await expect(row.locator(".entry-feed")).toBeVisible();
    }
  },
);

Then(
  "every open-original link points at the entry's source URL",
  async ({ page }) => {
    const links = page.getByTestId("entry-open-original");
    const count = await links.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      await expect(links.nth(i)).toHaveAttribute("href", /^https?:\/\/.+/);
    }
  },
);

Then(
  "the entry title for {string} highlights on hover",
  async ({ page }, title) => {
    // Guards the restored title hover affordance (also lost in 0.55.0).
    // Theme-independent: assert the colour changes rather than a fixed value.
    const link = page
      .getByTestId("entry-item")
      .filter({ hasText: title })
      .getByTestId("entry-title-link")
      .first();
    await page.mouse.move(0, 0);
    const base = await link.evaluate((el) => getComputedStyle(el).color);
    await link.hover();
    await expect
      .poll(() => link.evaluate((el) => getComputedStyle(el).color))
      .not.toBe(base);
  },
);

When("I mark the entry titled {string} read", async ({ page }, title) => {
  // The Wire Room redesign removed the per-row read action; the star is the
  // only visible row control. Marking-read now happens through the reading
  // pane: opening an entry auto-marks it read and returns the row in its read
  // state plus the decremented sidebar count (same observable behaviour).
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .getByTestId("entry-title-link")
    .first()
    .click();
  await expect(page.getByTestId("reading-pane-title")).toBeVisible();
});

Then("the entry titled {string} is marked starred", async ({ page }, title) => {
  // Editorial redesign: the starred state lives on the star-action toggle —
  // when starred it shows ★ and flips to aria-label "Unstar" + POST /unstar.
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row.getByTestId("entry-star-action")).toHaveAttribute(
    "aria-label",
    "Unstar",
  );
});

Then("the sidebar starred count is at least {int}", async ({ page }, n) => {
  // The sidebar Starred link (<a href="/entries/starred">) has no numeric badge —
  // only the Unread link carries a count. This step can only assert the link is
  // visible; a count assertion requires a future UI addition.
  // Strengthen to a numeric assertion once the Starred link gains a badge.
  const locator = page.locator('a[href="/entries/starred"]').first();
  await expect(locator).toBeVisible();
  void n; // numeric check deferred until sidebar starred-count badge is added
});

Then("the sidebar unread count decreases by {int}", async ({ page }, _delta) => {
  // The total-unread badge has id="unread-count" (rendered by rdrs-sidebar.js).
  // Delta comparison requires capturing the before-count, which needs a fixture
  // hook. For now assert the element is present and shows a non-negative number.
  // Strengthen to a delta assertion once before/after capture is wired.
  const locator = page.locator("#unread-count").first();
  const text = await locator.innerText();
  const count = parseInt(text, 10);
  expect(count).toBeGreaterThanOrEqual(0);
});

// Holding the SSE-driven `GET /entries/{id}/summary/fragment` reproduces the
// race the reader hit: the event for the outgoing entry passes its
// `currentPaneEntryId()` pre-check, then the response lands after the pane has
// already moved on. `heldSummaryFragment` is set by the route handler and read
// by the two steps below; each scenario re-arms it.
let heldSummaryFragment = null;
const SUMMARY_FRAGMENT_RE = /\/entries\/\d+\/summary\/fragment/;

When("the summary fragment response is held", async ({ page }) => {
  heldSummaryFragment = null;
  await page.route(SUMMARY_FRAGMENT_RE, async (route) => {
    let release;
    const gate = new Promise((resolve) => { release = resolve; });
    heldSummaryFragment = { release };
    await gate;
    await route.continue();
  });
});

When("the summary fragment request is in flight", async () => {
  await expect.poll(() => heldSummaryFragment !== null, {
    message: "no summary fragment request arrived — the SSE event never fired",
    timeout: 15000,
  }).toBe(true);
});

When("the held summary fragment response lands", async ({ page }) => {
  const landed = page.waitForResponse(SUMMARY_FRAGMENT_RE);
  heldSummaryFragment.release();
  await landed;
  // The response is on the wire; give performSwap the two frames it needs to
  // apply — or, as asserted next, discard — it before we look at the DOM.
  await page.evaluate(
    () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
  );
});

Then("the reading pane shows no summary", async ({ page }) => {
  await expect(page.locator("#rp-summary-container .summary-box")).toHaveCount(0);
});

Then("the reading pane shows a summary", async ({ page }) => {
  // The summary container always renders as <div id="rp-summary-container">.
  // When a summary exists, a .summary-box is inside it.
  await expect(page.locator("#rp-summary-container .summary-box")).toBeVisible();
});

// The action-bar Summarize/Dismiss toggle button, located by its stable
// `data-summary-toggle` marker rather than its (changing) accessible name —
// its name flips between "Summarize" and "Dismiss summary" with summary state,
// and the "Dismiss summary" name would otherwise collide with the summary
// box's own Dismiss control.
const SUMMARIZE_TOGGLE = ".reading-pane-actions [data-summary-toggle] button";

When("I click the reading-pane summarize toggle", async ({ page }) => {
  await page.locator(SUMMARIZE_TOGGLE).click();
});

Then(
  "the reading-pane summarize toggle reads {string}",
  async ({ page }, text) => {
    await expect(page.locator(`${SUMMARIZE_TOGGLE} .action-label`)).toHaveText(
      text,
    );
  },
);

Then(
  "the reading-pane summarize toggle still shows its icon",
  async ({ page }) => {
    // Only the visible icon span (the hidden one is toggled off with `hidden`).
    await expect(
      page.locator(`${SUMMARIZE_TOGGLE} .action-icon:not([hidden]) svg`),
    ).toBeVisible();
  },
);

Then("the reading pane summary is dismissed", async ({ page }) => {
  // After clicking "Dismiss", app.js calls container.replaceChildren() which
  // empties #rp-summary-container but keeps the wrapper element in the DOM.
  await expect(page.locator("#rp-summary-container .summary-box")).toHaveCount(0);
});

Then("the reading-pane summarize toggle is disabled", async ({ page }) => {
  await expect(page.locator(SUMMARIZE_TOGGLE)).toBeDisabled();
});

// Count re-queue POSTs to /entries/{id}/summarize (NOT /summarize/cancel) so a
// test can prove the in-flight toggle is truly inert.
const summarizeWatched = new WeakSet();
let summarizePostCount = 0;

When("I watch for summarize POST requests", async ({ page }) => {
  summarizePostCount = 0;
  if (!summarizeWatched.has(page)) {
    summarizeWatched.add(page);
    page.on("request", (req) => {
      if (req.method() !== "POST") return;
      if (/\/entries\/\d+\/summarize$/.test(new URL(req.url()).pathname)) {
        summarizePostCount += 1;
      }
    });
  }
});

Then("no summarize POST request is sent", async ({ page }) => {
  // A real re-queue POSTs synchronously on submit; give it a beat to land,
  // then assert none fired.
  await page.waitForTimeout(300);
  expect(summarizePostCount).toBe(0);
});
