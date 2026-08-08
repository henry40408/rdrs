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

Given("the {string} feed has a favicon", async ({ seed, currentUser }, feedTitle) => {
  const userId = seed.getUserId(currentUser.username);
  const feedId = seed.findFeedIdByTitle(userId, feedTitle);
  // 1x1 transparent PNG — enough for feed_has_icon to render an <img> favicon.
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M8AAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
    "base64",
  );
  seed.insertIcon(feedId, png, "image/png", "https://example.com/icon.png");
});

// Point an entry at a link the readability fetcher rejects outright. The SSRF
// guard in utils/url_validation.rs blocks loopback before any network I/O, so
// Fetch Full Content answers immediately with its error flash instead of
// waiting on DNS — which is what a scenario about the *round trip* needs, and
// what the seeded https://example.com links cannot give: they resolve (or hang)
// depending on whether the machine has internet, and the fetch fails either way
// once the extractor sees a 404.
Given("the entry titled {string} cannot have its full content fetched",
  async ({ seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    seed.setEntryLink(seed.findEntryIdByTitle(userId, title), "http://127.0.0.1/blocked");
  });

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

Given("the entry titled {string} has a pending summary", async ({ seed, currentUser }, title) => {
  const userId = seed.getUserId(currentUser.username);
  seed.insertPendingSummary(seed.findEntryIdByTitle(userId, title), userId);
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

Given(
  "the entry titled {string} contains a line-numbered code block",
  async ({ seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    // Mirror Rouge's line-numbered output: an outer <pre> wrapping a <code> +
    // <table> whose cells each hold their own nested <pre> (gutter + code).
    const html =
      '<div class="highlight"><pre class="highlight"><code>' +
      '<table class="rouge-table"><tbody><tr>' +
      '<td class="rouge-gutter"><pre class="lineno">1\n2\n3\n</pre></td>' +
      '<td class="rouge-code"><pre>line one\nline two\nline three\n</pre></td>' +
      '</tr></tbody></table></code></pre></div>';
    seed.setEntryContent(seed.findEntryIdByTitle(userId, title), html);
  },
);

Then(
  "the nested code-block pre has no padding while the outer pre does",
  async ({ page }) => {
    const outer = page.locator(".reading-pane-article pre").first();
    // The innermost <pre> (Rouge gutter/code) must be neutralised to 0 padding.
    const inner = page.locator(".reading-pane-article pre pre").first();
    await expect(inner).toHaveCount(1);
    const innerPad = await inner.evaluate(
      (el) => getComputedStyle(el).paddingTop,
    );
    const outerPad = await outer.evaluate(
      (el) => getComputedStyle(el).paddingTop,
    );
    expect(innerPad).toBe("0px");
    // Outer keeps its block padding (non-zero).
    expect(parseFloat(outerPad)).toBeGreaterThan(0);
  },
);

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

// `?status=all` keeps read entries listed, which is what the morph scenarios
// need: on the unread view a row that is marked read simply leaves, and a row
// that is gone proves nothing about whether the ones that stayed were rebuilt.
When(
  "I open the entries page for category {string} showing all statuses",
  async ({ page, seed, currentUser, serverUrl }, name) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.findCategoryIdByName(userId, name);
    await page.goto(`${serverUrl}/categories/${categoryId}/entries?status=all`);
  }
);

When("the entry list favicons have loaded", async ({ page }) => {
  const icons = page.locator("[data-entries-list] img.entry-favicon");
  await expect(icons.first()).toBeVisible();
  await expect
    .poll(async () => icons.evaluateAll((imgs) => imgs.every((i) => i.complete && i.naturalWidth > 0)))
    .toBe(true);
});

// Tags by JS property, not attribute — an attribute would change `outerHTML`,
// which the swap logic compares, and would also be something the morph then has
// to decide whether to strip.
When("I tag the entry list contents", async ({ page }) => {
  const counts = await page.evaluate(() => {
    const list = document.querySelector("[data-entries-list]");
    const rows = [...list.querySelectorAll("[data-entry-row]")];
    const icons = [...list.querySelectorAll("img.entry-favicon")];
    list.__e2eMorphTag = true;
    for (const node of [...rows, ...icons]) node.__e2eMorphTag = true;
    return { rows: rows.length, icons: icons.length };
  });
  expect(counts.rows).toBeGreaterThan(0);
  expect(counts.icons).toBeGreaterThan(0);
});

Then("every entry in the list is marked read", async ({ page }) => {
  const rows = page.locator("[data-entries-list] [data-entry-row]");
  await expect(rows.first()).toBeVisible();
  await expect
    .poll(async () => rows.evaluateAll((els) => els.length > 0 && els.every((e) => e.classList.contains("entry-read"))))
    .toBe(true);
});

Then("the entry list contents are still the ones I tagged", async ({ page }) => {
  const kept = await page.evaluate(() => {
    const list = document.querySelector("[data-entries-list]");
    const nodes = [...list.querySelectorAll("[data-entry-row]"), ...list.querySelectorAll("img.entry-favicon")];
    return {
      container: list.__e2eMorphTag === true,
      total: nodes.length,
      tagged: nodes.filter((n) => n.__e2eMorphTag).length,
    };
  });
  expect(kept.container).toBe(true);
  expect(kept.total).toBeGreaterThan(0);
  expect(kept.tagged).toBe(kept.total);
});

When(
  "I open the entries page for category {string} searching for {string}",
  async ({ page, seed, currentUser, serverUrl }, name, q) => {
    const userId = seed.getUserId(currentUser.username);
    const categoryId = seed.findCategoryIdByName(userId, name);
    await page.goto(`${serverUrl}/categories/${categoryId}/entries?q=${encodeURIComponent(q)}`);
  }
);

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
const delayedFullContentFetches = new WeakMap();

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
  "the fetch full content response for the entry titled {string} is delayed",
  async ({ page, seed, currentUser }, title) => {
    const userId = seed.getUserId(currentUser.username);
    const entryId = seed.findEntryIdByTitle(userId, title);
    const state = {};
    state.done = new Promise((resolve) => {
      state.resolve = resolve;
    });
    delayedFullContentFetches.set(page, state);
    await page.route(`**/entries/${entryId}/fetch-full-content*`, async (route) => {
      await new Promise((r) => setTimeout(r, 600));
      try {
        await route.continue();
      } catch {}
      state.resolve();
    });
  }
);

Then("I see a {string} fetch full content action", async ({ page }, label) => {
  const action = page.locator('form[action*="/fetch-full-content"] button');
  await expect(action).toHaveAccessibleName(label);
  await expect(action).toContainText(label);
});

Then("I see a {string} button", async ({ page }, label) => {
  await expect(page.getByRole("button", { name: label })).toBeVisible();
});

Then("the delayed fetch full content response has settled", async ({ page }) => {
  const state = delayedFullContentFetches.get(page);
  if (!state) throw new Error("no delayed full-content route was armed");
  await state.done;
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

// The category shortcuts (`[`, `]`, `{`, `}`) read the sidebar's category list
// and do nothing at all when it is empty (see the early return in app.js), so a
// keypress that races the sidebar's fetch silently no-ops and the assertion
// that follows fails for reasons that have nothing to do with the shortcut.
When("the sidebar has loaded its categories", async ({ page }) => {
  await expect(page.locator("#sidebar-categories a").first()).toBeVisible();
});

When("I press the {string} key", async ({ page }, key) => {
  await page.click("body");
  await page.keyboard.press(key);
});

const sidebarCategoryLink = (page, name) =>
  page
    .locator("#sidebar-categories a[data-category-id]")
    .filter({ hasText: name })
    .first();

When("I click the sidebar category {string}", async ({ page }, name) => {
  await sidebarCategoryLink(page, name).click();
});

Then("the sidebar highlights category {string}", async ({ page }, name) => {
  await expect(sidebarCategoryLink(page, name)).toHaveClass(/active/);
});

const sidebarFeedLink = (page, title) =>
  page.locator(".sidebar-feed[data-feed-id]").filter({ hasText: title }).first();

When("I click the sidebar feed {string}", async ({ page }, title) => {
  await sidebarFeedLink(page, title).click();
});

Then("the sidebar lists feed {string}", async ({ page }, title) => {
  await expect(sidebarFeedLink(page, title)).toBeVisible();
});

Then("the sidebar does not list feed {string}", async ({ page }, title) => {
  await expect(sidebarFeedLink(page, title)).toHaveCount(0);
});

Then("the sidebar highlights feed {string}", async ({ page }, title) => {
  await expect(sidebarFeedLink(page, title)).toHaveClass(/active/);
});

Then("the sidebar feed {string} shows {int} unread", async ({ page }, title, count) => {
  await expect(sidebarFeedLink(page, title).locator(".sidebar-badge")).toHaveText(String(count));
});

// A row built by a re-render carries no `data-e2e-tag`, so a tag set before an
// interaction and still there afterwards proves the row — and the favicon inside
// it — was patched in place rather than rebuilt. Rebuilding an <img> costs a
// blank frame in WebKit, which is what reconciling the feed list avoids.
When("I tag the sidebar feed rows", async ({ page }) => {
  const tagged = await page.evaluate(() => {
    const rows = [...document.querySelectorAll(".sidebar-feed[data-feed-id]")];
    for (const row of rows) {
      row.dataset.e2eTag = "1";
      const icon = row.querySelector(".entry-favicon");
      if (icon) icon.dataset.e2eTag = "1";
    }
    return rows.length;
  });
  expect(tagged).toBeGreaterThan(0);
});

Then("the sidebar feed rows are still the ones I tagged", async ({ page }) => {
  await expect(
    page.locator(".sidebar-feed[data-feed-id]:not([data-e2e-tag])")
  ).toHaveCount(0);
  await expect(
    page.locator(".sidebar-feed[data-feed-id] .entry-favicon:not([data-e2e-tag])")
  ).toHaveCount(0);
});

Then("the sidebar feed {string} shows its icon", async ({ page }, title) => {
  const icon = sidebarFeedLink(page, title).locator("img.entry-favicon");
  await expect(icon).toBeVisible();
  await expect(icon).toHaveAttribute("src", /\/api\/feeds\/\d+\/icon/);
});

When("I click the breadcrumb link {string}", async ({ page }, label) => {
  await page.getByTestId("breadcrumb").getByRole("link", { name: label, exact: true }).click();
});

Then("the browser is on {string}", async ({ page }, path) => {
  await expect(page).toHaveURL(new RegExp(`${path.replace(/\//g, "\\/")}$`));
});

// The feed name inside an entry row points at the same /feeds/{id}/entries the
// sidebar does, so it must take the same in-place swap rather than reloading.
When("I click the feed name in the entry titled {string}", async ({ page }, title) => {
  await page
    .getByTestId("entry-item")
    .filter({ hasText: title })
    .first()
    .locator(".entry-feed")
    .click();
});

Then("the sidebar feed {string} shows an initial chip", async ({ page }, title) => {
  // The no-icon fallback, same as the entry rows: first letter, uppercased.
  const chip = sidebarFeedLink(page, title).locator(".entry-favicon-chip");
  await expect(chip).toHaveText(title.slice(0, 1).toUpperCase());
});


// A document load wipes anything hung off `window`, so a marker set before the
// interaction and still readable after it proves the category switch stayed in
// the same document (the whole point of the list-pane swap: a reload resets the
// sidebar's own scroll offset).
When("I mark the document for reload detection", async ({ page }) => {
  await page.evaluate(() => { window.__rdrsDocumentMarker = true; });
});

Then("the document did not reload", async ({ page }) => {
  expect(await page.evaluate(() => window.__rdrsDocumentMarker === true)).toBe(true);
});

// The reported bug in one assertion: with enough categories to make
// `.sidebar-nav` scroll, a document reload (or an innerHTML re-render of the
// sidebar) sends it back to the top and the category the reader just clicked
// scrolls out of view.
When("I scroll the sidebar categories to the bottom", async ({ page }) => {
  const offset = await page.evaluate(() => {
    const nav = document.querySelector(".sidebar-nav");
    nav.scrollTop = nav.scrollHeight;
    window.__rdrsSidebarScroll = nav.scrollTop;
    return nav.scrollTop;
  });
  expect(offset, "sidebar nav must actually overflow for this scenario").toBeGreaterThan(0);
});

// Deliberately the *last* category: Playwright scrolls a target into view
// before clicking it, so clicking one above the fold would move `.sidebar-nav`
// itself and the assertion that follows would measure the test's own scrolling
// rather than the swap's effect.
When("I click the last sidebar category", async ({ page }) => {
  const link = page.locator("#sidebar-categories a[data-category-id]").last();
  const name = (await link.locator(".sidebar-item-label").innerText()).trim();
  await link.click();
  await expect(page.locator(".list-pane-header h1")).toContainText(name);
});

Then("the sidebar is still scrolled where it was", async ({ page }) => {
  const [noted, now] = await page.evaluate(() => [
    window.__rdrsSidebarScroll,
    document.querySelector(".sidebar-nav").scrollTop,
  ]);
  expect(noted, "noted offset is gone — the document reloaded").toBeGreaterThan(0);
  // Not exact equality: the open category's feed list mounts and unmounts as
  // the reader moves, which legitimately changes the scroll extent (and a
  // bottom-anchored offset then gets clamped). What must hold is that the
  // sidebar stays where it was rather than snapping back to the top — a reload
  // or a full re-render lands on 0, which this catches.
  expect(Math.abs(now - noted)).toBeLessThan(80);
});

Given("I have {int} more categories", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  for (let i = 1; i <= count; i++) seed.createCategory(userId, `Filler ${i}`);
});

Then("the list header shows {string}", async ({ page }, title) => {
  await expect(page.locator(".list-pane-header h1")).toContainText(title);
});

When("I go back in the browser", async ({ page }) => {
  await page.goBack();
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

// Tag every element inside every entry row. A node the swap replaced comes back
// from the server untagged, so what is still tagged afterwards is exactly what
// survived — the check for "this interaction left the list alone".
//
// The tag is a JS property, not a `data-` attribute: an attribute would change
// the node's `outerHTML`, and `outerHTML` is what performSwap compares to
// decide a row fragment is unchanged. Tagging by attribute defeats the very
// skip this asserts.
When("I tag the entry rows", async ({ page }) => {
  const tagged = await page.evaluate(() => {
    const nodes = document.querySelectorAll("[data-entry-row], [data-entry-row] *");
    for (const node of nodes) node.__e2eTag = true;
    return nodes.length;
  });
  expect(tagged).toBeGreaterThan(0);
});

Then("the entry rows are still the ones I tagged", async ({ page }) => {
  const untagged = await page.evaluate(() =>
    [...document.querySelectorAll("[data-entry-row], [data-entry-row] *")]
      .filter((node) => !node.__e2eTag)
      .map((node) => `${node.nodeName.toLowerCase()}.${node.className} in #${node.closest("[data-entry-row]")?.id}`));
  expect(untagged).toEqual([]);
});

// Arms the wait for the *next* list-pane response before the click that causes
// it, so the assertion can be sure the response landed rather than guessing with
// a timeout. Tags are JS properties rather than attributes — see "I tag the
// entry rows" for why that matters here.
const panePending = new WeakMap();
const paneStamp = new WeakMap();

When("I tag the entry list pane", async ({ page }) => {
  const state = await page.evaluate(() => {
    const pane = document.querySelector("[data-list-pane]");
    pane.__e2ePaneTag = true;
    const rows = [...pane.querySelectorAll("[data-entry-row]")];
    const icons = [...pane.querySelectorAll("img")];
    for (const node of [...rows, ...icons]) node.__e2ePaneTag = true;
    return {
      rows: rows.length,
      icons: icons.length,
      stamp: pane.querySelector("[data-snapshot-at]")?.getAttribute("data-snapshot-at"),
    };
  });
  expect(state.rows).toBeGreaterThan(0);
  expect(state.icons).toBeGreaterThan(0);
  paneStamp.set(page, state.stamp);
  panePending.set(page, page.waitForResponse((r) => r.url().includes("pane=1")));
});

// `data-snapshot-at` has one-second resolution, so two clicks inside the same
// second carry the same stamp and "did it advance?" would be unanswerable.
When("I let the render stamp age", async ({ page }) => {
  await page.waitForTimeout(1100);
});

Then("the entry list pane is still the one I tagged", async ({ page }) => {
  await panePending.get(page);
  // One frame for the swap logic that runs on the response to have its say.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  const kept = await page.evaluate(() => {
    const pane = document.querySelector("[data-list-pane]");
    const nodes = [...pane.querySelectorAll("[data-entry-row]"), ...pane.querySelectorAll("img")];
    return {
      pane: pane.__e2ePaneTag === true,
      total: nodes.length,
      tagged: nodes.filter((n) => n.__e2ePaneTag).length,
    };
  });
  expect(kept.pane).toBe(true);
  expect(kept.total).toBeGreaterThan(0);
  expect(kept.tagged).toBe(kept.total);
});

Then("the list's render stamp has advanced", async ({ page }) => {
  const before = paneStamp.get(page);
  expect(before).toBeTruthy();
  await expect
    .poll(async () => page.evaluate(() =>
      document.querySelector("[data-list-pane] [data-snapshot-at]")?.getAttribute("data-snapshot-at")))
    .not.toBe(before);
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

Then("the reading pane shows an image favicon", async ({ page }) => {
  // Wire Room: the favicon leads the mono dispatch eyebrow (was .reading-pane-meta).
  const favicon = page.locator(".dispatch-eyebrow img.entry-favicon");
  await expect(favicon).toBeVisible();
  await expect(favicon).toHaveAttribute("src", /\/icon$/);
});

// Synchronous snapshot right after the (delayed) navigation click: the swap
// handler has already run cancelPaneImages() on the still-visible outgoing
// pane, so the favicon's src reveals whether it was wrongly blanked. Reads the
// attribute once (no auto-retry) so it can't be masked by the next entry
// eventually landing.
Then("the reading pane favicon still has its image", async ({ page }) => {
  const src = await page.locator(".dispatch-eyebrow img.entry-favicon").getAttribute("src");
  expect(src, "reading-pane favicon must keep its src during navigation").toBeTruthy();
});

Then("the second entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").nth(1)).toHaveClass(/selected|active/);
});

Then("the first entry is selected", async ({ page }) => {
  await expect(page.getByTestId("entry-item").first()).toHaveClass(/selected|active/);
});

// `.selected` is client-side only, so a stale highlight left on a row the
// reader has navigated away from shows up here as a count of 2.
Then("exactly one entry is selected", async ({ page }) => {
  await expect(page.locator("[data-entry-row].selected")).toHaveCount(1);
});

Then("the selected entry is titled {string}", async ({ page }, title) => {
  await expect(page.locator("[data-entry-row].selected")).toContainText(title);
});

Then("the keyboard shortcut help overlay is visible", async ({ page }) => {
  await expect(page.getByTestId("kb-help")).toBeVisible();
});

Then("the reading pane shows the original feed body", async ({ page }) => {
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
  const context = page.context();
  // Seeded entry links point at https://example.com/… (support/seed.js). What
  // this scenario asserts is *which URL* the shortcut targets, not that the
  // page loads — so stub the origin rather than depend on the outbound network.
  // Without the stub the popup's navigation fails DNS resolution and
  // popup.url() collapses to chrome-error://chromewebdata/, which fails the
  // assertion on any machine (or sandbox) without internet access. Routes
  // registered on the context also cover pages opened later, so this applies
  // to the popup even though it doesn't exist yet.
  await context.route("https://example.com/**", (route) =>
    route.fulfill({
      status: 200,
      contentType: "text/html",
      body: "<!doctype html><title>stubbed external page</title>",
    })
  );
  // Popup capture must be armed BEFORE the keystroke that triggers it —
  // waitForEvent is a one-shot listener that misses already-fired events.
  await page.click("body");
  const popupPromise = context.waitForEvent("page", { timeout: 5000 });
  await page.keyboard.press(key);
  const popup = await popupPromise;
  // A popup can surface as about:blank before its navigation commits, so
  // settle it first instead of racing the assertion against the navigation.
  await popup.waitForLoadState("domcontentloaded").catch(() => {});
  expect(popup.url()).toContain(urlSubstring);
  await popup.close().catch(() => {});
  await context.unroute("https://example.com/**");
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

Then("the help overlay resolves its design tokens", async ({ page }) => {
  // The shadow stylesheet references tokens with no `var(--x, fallback)`
  // defaults, so a token renamed in app.css would leave the modal unstyled —
  // transparent panel, default text colour — while every other help-overlay
  // assertion (visible / hidden / aligned) still passed. Compare what the
  // shadow DOM actually computed against the same tokens resolved on :root,
  // which stays correct under either theme.
  const probe = await page.evaluate(() => {
    const modal = document.querySelector("rdrs-kb-help").shadowRoot.querySelector(".modal");
    // Resolve each token through a throwaway element in the light DOM and read
    // it back as a *computed* value, so both sides of the comparison go through
    // the same normalisation (the browser rewrites colours to rgb() and strips
    // quotes from font stacks — comparing against the raw token text fails on
    // formatting alone).
    const probeValue = (property, token) => {
      const el = document.createElement("span");
      el.style[property] = `var(${token})`;
      document.body.appendChild(el);
      const value = getComputedStyle(el)[property];
      el.remove();
      return value;
    };
    return {
      background: getComputedStyle(modal).backgroundColor,
      expectedBackground: probeValue("backgroundColor", "--color-panel"),
      color: getComputedStyle(modal).color,
      expectedColor: probeValue("color", "--color-text"),
      fontFamily: getComputedStyle(modal).fontFamily,
      expectedFontFamily: probeValue("fontFamily", "--font-ui"),
    };
  });
  expect(probe.background).toBe(probe.expectedBackground);
  expect(probe.color).toBe(probe.expectedColor);
  expect(probe.fontFamily).toBe(probe.expectedFontFamily);
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

Then("the feed link does not span the full meta row", async ({ page }) => {
  // The feed-title <a> must shrink-wrap its text. A full-width block link made
  // clicks on the blank space after a short feed name navigate to the feed
  // (installRowClickToOpen defers to any anchor under the pointer) instead of
  // falling through to the row's open-entry handler. With a short feed name the
  // anchor box must be narrower than its flex:1 text container.
  const row = page.getByTestId("entry-item").first();
  const link = await row.locator(".entry-feed").boundingBox();
  const container = await row.locator(".entry-meta-text").boundingBox();
  expect(link).not.toBeNull();
  expect(container).not.toBeNull();
  expect(link.width).toBeLessThan(container.width);
});
