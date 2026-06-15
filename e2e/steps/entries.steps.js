import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given(
  "I have a feed {string} with {int} test entries in category {string}",
  async ({ seed, currentUser }, feedTitle, count, categoryName) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.createCategory(userId, categoryName);
    const feedId = seed.createFeed(
      categoryId,
      `https://example.com/${currentUser.username}-${feedTitle}.xml`,
      feedTitle
    );
    seed.seedTestEntries(feedId, count);
  }
);

Given("the entry titled {string} is marked read", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  seed.markRead(seed.findEntryIdByTitle(userId, title));
});

// Backdated so the read lands strictly BEFORE the page's render-time
// snapshot — a datetime('now') read in the same second as the render would
// fall inside the >= snapshot boundary and make skip-assertions flaky.
Given(
  "the entry titled {string} was marked read an hour ago",
  async ({ seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    seed.markRead(seed.findEntryIdByTitle(userId, title), "-1 hour");
  }
);

Given("the entry titled {string} is starred", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  seed.markStarred(seed.findEntryIdByTitle(userId, title));
});

Given("the entry titled {string} has a summary", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  seed.insertSummary(seed.findEntryIdByTitle(userId, title), userId);
});

Given("the entry titled {string} has a failed summary", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  seed.insertFailedSummary(seed.findEntryIdByTitle(userId, title), userId);
});

Given("the entry titled {string} has content with a broken image", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  const entryId = seed.findEntryIdByTitle(userId, title);
  seed.setEntryContent(
    entryId,
    '<p>x</p><img src="https://images.internal/missing.jpg" alt="Missing diagram">',
  );
});

Then("the reading pane shows a broken-image fallback", async ({ page }) => {
  await expect(page.locator(".reading-pane-article .rp-broken-image")).toBeVisible();
  await expect(page.locator(".rp-broken-cap")).toContainText("Image unavailable");
});

Given("all entries in category {string} are marked read", async ({ seed, currentUser }, name) => {
  const userId = seed.getUserId(currentUser.username);
  seed.markCategoryRead(userId, name);
});

Given("the feed has {int} entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  seed.seedTestEntries(seed.firstFeedId(userId), count);
});

When("I open the all entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries`);
});

When("I open the read entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/read`);
});

When("I open the starred entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/starred`);
});

When("I open the summarized entries page", async ({ page, serverUrl }) => {
  await page.goto(`${serverUrl}/entries/summarized`);
});

When("I open the entries page for feed {string}", async ({ page, seed, currentUser, serverUrl }, feedTitle) => {
  const userId = seed.getUserId(currentUser.username);
  const feedId = seed.findFeedIdByTitle(userId, feedTitle);
  await page.goto(`${serverUrl}/feeds/${feedId}/entries`);
});

When("I open the entries page for category {string}", async ({ page, seed, currentUser, serverUrl }, name) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.findCategoryIdByName(userId, name);
  await page.goto(`${serverUrl}/categories/${categoryId}/entries`);
});

When("I click the entry titled {string}", async ({ page }, title) => {
  // Click the title link (data-testid="entry-title-link") to trigger the
  // data-swap="#reading-pane" fetch. Clicking the entry-item container is
  // unreliable because installRowClickToOpen bails on any <a> target; the
  // title link is the canonical entry-open action.
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .first()
    .getByTestId("entry-title-link")
    .click();
  // Wait for the reading pane swap to complete — the empty placeholder loses
  // its .reading-pane-empty class once the fragment replaces #reading-pane.
  await page.locator("#reading-pane:not(.reading-pane-empty)").waitFor({ state: "attached" });
});

When("I click {string}", async ({ page }, label) => {
  await page.getByRole("button", { name: label }).click();
});

// Reading-pane prev/next. The button starts disabled and app.js enables it
// once `/api/entries/{id}/neighbors` resolves; Playwright's click waits for
// the actionable (enabled) state, so no explicit wait is needed there.
function paneNavTestId(direction) {
  return direction.toLowerCase() === "next"
    ? "reading-pane-next"
    : "reading-pane-prev";
}

// The pane carries no entry id of its own, but every action form targets
// `/entries/{id}/...` — mirror app.js's currentPaneEntryId() to read it.
async function paneEntryId(page) {
  const action = await page
    .locator('#reading-pane form[action*="/entries/"]')
    .first()
    .getAttribute("action")
    .catch(() => null);
  const m = action?.match(/\/entries\/(\d+)\//);
  return m ? m[1] : null;
}

// Hold one entry's fragment response for 600ms before serving it, recording
// when the route finished (served — or aborted by the stale-response guard).
// Drives the race scenario: a slow stale response must never overwrite the
// entry the user clicked afterwards.
const delayedFragments = new WeakMap();

When(
  "the fragment response for the entry titled {string} is delayed",
  async ({ page, seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    const entryId = seed.findEntryIdByTitle(userId, title);
    const state = {};
    state.done = new Promise((resolve) => {
      state.resolve = resolve;
    });
    delayedFragments.set(page, state);
    await page.route(`**/entries/${entryId}/fragment*`, async (route) => {
      await new Promise((r) => setTimeout(r, 600));
      try {
        await route.continue();
      } catch {
        // The stale-response guard aborted this request while it was held
        // here — exactly the post-fix behaviour; nothing left to serve.
      }
      state.resolve();
    });
  }
);

When(
  "I click the entry titled {string} without waiting for the pane",
  async ({ page }, title) => {
    // Same locator as "I click the entry titled {string}" but WITHOUT the
    // pane-not-empty wait — this click's response is being held by the
    // delayed route, so the pane must still be empty when the next step
    // clicks the second entry.
    await page
      .getByTestId("entry-item")
      .filter({ hasText: title })
      .first()
      .getByTestId("entry-title-link")
      .click();
  }
);

When("the delayed fragment response has settled", async ({ page }) => {
  const state = delayedFragments.get(page);
  if (!state) throw new Error("no delayed fragment route was armed");
  await state.done;
  // Give a (stale) swap one tick to apply before the Then assertions —
  // pre-fix, the bug manifests as the pane flipping back AFTER this point.
  await page.waitForTimeout(100);
});

When(
  "I navigate to the {string} entry in the reading pane",
  async ({ page }, direction) => {
    // Capture the current entry before the click so we can wait until the
    // pane actually swaps to a different one — guards against a follow-up
    // navigation firing before this swap (and its neighbour re-resolve)
    // lands.
    const before = await paneEntryId(page);
    await page.getByTestId(paneNavTestId(direction)).click();
    await expect.poll(() => paneEntryId(page)).not.toBe(before);
  }
);

Then(
  "the reading-pane {string} button is disabled",
  async ({ page }, direction) => {
    await expect(page.getByTestId(paneNavTestId(direction))).toBeDisabled();
  }
);

Then(
  "the reading-pane {string} button is enabled",
  async ({ page }, direction) => {
    await expect(page.getByTestId(paneNavTestId(direction))).toBeEnabled();
  }
);

When("I press the {string} key", async ({ page }, key) => {
  await page.click("body");
  await page.keyboard.press(key);
});

When("I confirm the next dialog", async ({ page }) => {
  // Pre-arms a one-shot dialog handler so the next window.confirm/alert
  // auto-accepts. Used by shortcuts that go through a confirmation prompt
  // (e.g. Shift+K → "Mark all as read?") — register BEFORE the keystroke.
  page.once("dialog", (dialog) => dialog.accept());
});

When("I click the {string} button", async ({ page }, label) => {
  await page.getByRole("button", { name: label }).click();
});

Then("I see {int} entries in the entry list", async ({ page }, count) => {
  await expect(page.getByTestId("entry-item")).toHaveCount(count);
});

Then("I see {int} entry in the entry list", async ({ page }, count) => {
  await expect(page.getByTestId("entry-item")).toHaveCount(count);
});

Then("I see more than {int} entries in the entry list", async ({ page }, count) => {
  // Use polling so async swaps (e.g. Load More fetch) have a chance to land
  // before we assert. Plain `count()` snapshots the DOM at one instant.
  await expect.poll(() => page.getByTestId("entry-item").count()).toBeGreaterThan(count);
});

Then("the first entry is titled {string}", async ({ page }, title) => {
  await expect(page.getByTestId("entry-item").first()).toContainText(title);
});

Then("the reading pane shows the title {string}", async ({ page }, title) => {
  await expect(page.getByTestId("reading-pane-title")).toContainText(title);
});

Then("the reading pane shows the content {string}", async ({ page }, content) => {
  await expect(page.getByTestId("reading-pane-body")).toContainText(content);
});

Then("the reading pane shows the feed title {string}", async ({ page }, title) => {
  await expect(page.getByTestId("reading-pane-feed-title")).toContainText(title);
});

Then("the reading pane shows a published time", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-published-at")).toBeVisible();
});

Then("the second entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").nth(1)).toHaveClass(/selected|active/);
});

Then("the first entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").first()).toHaveClass(/selected|active/);
});

Then("the keyboard shortcut help overlay is visible", async ({ page }) => {
  await expect(page.getByTestId("kb-help")).toBeVisible();
});

Then("the reading pane shows the original feed body", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-body")).toHaveAttribute("data-mode", "original");
});

Then("the reading pane shows the original entry body", async ({ page }) => {
  await expect(page.getByTestId("reading-pane-body")).toHaveAttribute("data-mode", "original");
});

When("the sidebar shows no unread for category {string}", async ({ page }, name) => {
  // <rdrs-sidebar> hydrates from the SSR bootstrap on mount and then
  // re-fetches /api/sidebar asynchronously to refresh badges. Tests that
  // depend on the latest unread counts (e.g. Shift+] skip-empty nav) wait
  // here until the visible badge for `name` is gone, which means both
  // _data and the DOM reflect the freshest payload.
  const link = page.locator(`rdrs-sidebar a[href^="/categories/"]`).filter({ hasText: name });
  await expect(link.locator('.sidebar-badge')).toHaveCount(0);
});

Then("the reading pane is empty", async ({ page }) => {
  await expect(page.locator("#reading-pane.reading-pane-empty")).toBeAttached();
});

When("I reload the page", async ({ page }) => {
  await page.reload();
});

When(
  "I open the inbox deep-linked to entry titled {string}",
  async ({ page, serverUrl, seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    const id = seed.findEntryIdByTitle(userId, title);
    await page.goto(`${serverUrl}/?entry=${id}`);
  }
);

Then(
  "the URL has the ?entry= parameter for {string}",
  async ({ page, seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    const id = seed.findEntryIdByTitle(userId, title);
    // performSwap rewrites the URL after a #reading-pane swap (pushState on
    // first-open from an empty pane, replaceState on subsequent switches);
    // wait until the address-bar `entry` query matches the clicked entry's id.
    await expect
      .poll(() => new URL(page.url()).searchParams.get("entry"))
      .toBe(String(id));
  }
);

Then("the URL has no ?entry= parameter", async ({ page }) => {
  await expect
    .poll(() => new URL(page.url()).searchParams.has("entry"))
    .toBe(false);
});

Then("I am on the unread inbox", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/`);
});

Then("I am on the entries page for feed {string}", async ({ page, seed, currentUser, serverUrl }, feedTitle) => {
  const userId = seed.getUserId(currentUser.username);
  const feedId = seed.findFeedIdByTitle(userId, feedTitle);
  await page.waitForURL(`${serverUrl}/feeds/${feedId}/entries`);
});

Then("I am on the Read filter for feed {string}", async ({ page, seed, currentUser, serverUrl }, feedTitle) => {
  const userId = seed.getUserId(currentUser.username);
  const feedId = seed.findFeedIdByTitle(userId, feedTitle);
  await page.waitForURL(`${serverUrl}/feeds/${feedId}/entries?status=read`);
});

Then("pressing the {string} key opens a new tab at {string}", async ({ page }, key, urlSubstring) => {
  // Popup capture must be armed BEFORE the keystroke that triggers it —
  // waitForEvent is a one-shot listener that misses already-fired events.
  await page.click("body");
  const popupPromise = page.context().waitForEvent("page", { timeout: 5000 });
  await page.keyboard.press(key);
  const popup = await popupPromise;
  // popup.url() reflects the navigation target as soon as the popup event
  // fires; no need to wait for the (external) URL to actually load.
  expect(popup.url()).toContain(urlSubstring);
  await popup.close().catch(() => {});
});

Then("I am on the entries page for category {string}", async ({ page, seed, currentUser, serverUrl }, name) => {
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.findCategoryIdByName(userId, name);
  await page.waitForURL(`${serverUrl}/categories/${categoryId}/entries`);
});

Then("the entry row for {string} shows as read", async ({ page }, title) => {
  // _entry_row.html adds CSS class "entry-read" to the row when is_read is true.
  // There is no data-read attribute — the read state is conveyed by CSS class only.
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row).toHaveClass(/entry-read/);
});

Then("I see no flash message", async ({ page }) => {
  await expect(page.getByTestId("flash-message")).toHaveCount(0);
});

// Barrier for form-action swaps whose only visible signal is a toast (Save /
// Fetch Full Content). The flash is shown right after the reading-pane swap
// lands and the neighbour re-resolve fires, so waiting on it sequences any
// follow-up navigation after the pane has fully settled.
Then("I see a flash message", async ({ page }) => {
  await expect(page.getByTestId("flash-message").first()).toBeVisible();
});

Then("the entry row for {string} shows as starred", async ({ page }, title) => {
  // Editorial redesign: the starred state lives on the star-action toggle —
  // when starred it shows ★ and flips to aria-label "Unstar" + POST /unstar.
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row.getByTestId("entry-star-action")).toHaveAttribute(
    "aria-label",
    "Unstar",
  );
});

Then("the entry row for {string} shows as unread", async ({ page }, title) => {
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row).not.toHaveClass(/entry-read/);
});

Then("I am on the all entries page", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/entries`);
});

Then("I am on the starred entries page", async ({ page, serverUrl }) => {
  await page.waitForURL(`${serverUrl}/entries/starred`);
});

Then("the go-to hint is visible", async ({ page }) => {
  await expect(page.locator(".kbd-hint")).toBeVisible();
});

Then("the go-to hint is gone", async ({ page }) => {
  await expect(page.locator(".kbd-hint")).toHaveCount(0);
});

When("I press the {string} key without refocusing", async ({ page }, key) => {
  // The plain press step clicks <body> first, which would blur the help
  // overlay (focus sits on its Esc button after show()) and can trigger
  // its click-outside-to-close handler. Send the key to the currently
  // focused element instead.
  await page.keyboard.press(key);
});

Then("the keyboard shortcut help overlay is hidden", async ({ page }) => {
  await expect(page.getByTestId("kb-help")).toBeHidden();
});

Then("the sidebar highlights All Entries", async ({ page }) => {
  await expect(page.getByTestId("nav-entries")).toHaveClass(/active/);
});

Then("the sidebar highlights Summarized", async ({ page }) => {
  await expect(page.getByTestId("nav-summarized")).toHaveClass(/active/);
});

Then("the sidebar Summarized item shows a count of {string}", async ({ page }, count) => {
  await expect(page.locator('[data-testid="nav-summarized"] #summarized-count')).toHaveText(count);
});

Then("the sidebar highlights Starred", async ({ page }) => {
  // The Starred sidebar item carries no data-testid — target by href.
  await expect(page.locator('rdrs-sidebar a[href="/entries/starred"]')).toHaveClass(/active/);
});

Then("the help overlay descriptions are aligned", async ({ page }) => {
  // Playwright CSS locators pierce the open shadow root. Compare the x of
  // the first four Navigation-group descriptions (j/k, o/Enter, Space —
  // the wide key combo — and Esc): pre-fix the Space row's key cell
  // overflows its column and pushes its description right.
  const descs = page.locator("rdrs-kb-help .shortcut-desc");
  const x0 = (await descs.nth(0).boundingBox()).x;
  for (let i = 1; i < 4; i++) {
    const { x } = await descs.nth(i).boundingBox();
    expect(Math.abs(x - x0)).toBeLessThan(1);
  }
});

Then("I see the summary error banner", async ({ page }) => {
  await expect(page.locator("[data-summary-error]")).toBeVisible();
});

Then("I do not see the summary error banner", async ({ page }) => {
  await expect(page.locator("[data-summary-error]")).toHaveCount(0);
});

Then("I see a {string} summary action", async ({ page }, label) => {
  await expect(
    page.locator("#rp-summary-container").getByRole("button", { name: label }),
  ).toBeVisible();
});

When("I click the {string} summary action", async ({ page }, label) => {
  await page
    .locator("#rp-summary-container")
    .getByRole("button", { name: label })
    .click();
  // Wait for the #rp-summary-container swap to settle rather than using
  // networkidle (flaky with the app's background sidebar polling).
  await page.locator("[data-summary-error]").waitFor({ state: "detached" });
});
