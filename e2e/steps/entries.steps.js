import { createBdd } from "playwright-bdd";
import Database from "better-sqlite3";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

function entryIdByTitle(seed, currentUser, title) {
  const db = new Database(seed.dbPath);
  try {
    const userId = seed.getUserId(currentUser.username);
    const row = db
      .prepare(
        `SELECT e.id FROM entry e
         JOIN feed f ON e.feed_id = f.id
         JOIN category c ON f.category_id = c.id
         WHERE c.user_id = ? AND e.title = ?`
      )
      .get(userId, title);
    if (!row) throw new Error(`Entry '${title}' not found`);
    return row.id;
  } finally {
    db.close();
  }
}

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
  const id = entryIdByTitle(seed, currentUser, title);
  seed.markRead(id);
});

Given("the entry titled {string} is starred", async ({ seed, currentUser }, title) => {
  const id = entryIdByTitle(seed, currentUser, title);
  seed.markStarred(id);
});

Given("the entry titled {string} has a summary", async ({ seed, currentUser }, title) => {
  const id = entryIdByTitle(seed, currentUser, title);
  const userId = seed.getUserId(currentUser.username);
  seed.insertSummary(id, userId);
});

Given("all entries in category {string} are marked read", async ({ seed, currentUser }, name) => {
  const userId = seed.getUserId(currentUser.username);
  const db = new Database(seed.dbPath);
  try {
    db.prepare(
      `UPDATE entry SET read_at = datetime('now')
       WHERE feed_id IN (
         SELECT f.id FROM feed f
         JOIN category c ON f.category_id = c.id
         WHERE c.user_id = ? AND c.name = ?
       )`
    ).run(userId, name);
  } finally {
    db.close();
  }
});

Given("the feed has {int} entries", async ({ seed, currentUser }, count) => {
  const userId = seed.getUserId(currentUser.username);
  const db = new Database(seed.dbPath);
  let feedId;
  try {
    const row = db
      .prepare(
        `SELECT f.id FROM feed f JOIN category c ON f.category_id = c.id WHERE c.user_id = ? LIMIT 1`
      )
      .get(userId);
    if (!row) throw new Error("No feed found for user");
    feedId = row.id;
  } finally {
    db.close();
  }
  seed.seedTestEntries(feedId, count);
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
  const db = new Database(seed.dbPath);
  let feedId;
  try {
    const row = db
      .prepare(
        `SELECT f.id FROM feed f JOIN category c ON f.category_id = c.id WHERE c.user_id = ? AND f.title = ?`
      )
      .get(userId, feedTitle);
    if (!row) throw new Error(`Feed '${feedTitle}' not found`);
    feedId = row.id;
  } finally {
    db.close();
  }
  await page.goto(`${serverUrl}/feeds/${feedId}/entries`);
});

When("I open the entries page for category {string}", async ({ page, seed, currentUser, serverUrl }, name) => {
  const userId = seed.getUserId(currentUser.username);
  const db = new Database(seed.dbPath);
  let categoryId;
  try {
    const row = db
      .prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`)
      .get(userId, name);
    if (!row) throw new Error(`Category '${name}' not found`);
    categoryId = row.id;
  } finally {
    db.close();
  }
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

Then("I am on the entries page for category {string}", async ({ page, seed, currentUser, serverUrl }, name) => {
  const userId = seed.getUserId(currentUser.username);
  const db = new Database(seed.dbPath);
  let categoryId;
  try {
    const row = db
      .prepare(`SELECT id FROM category WHERE user_id = ? AND name = ?`)
      .get(userId, name);
    if (!row) throw new Error(`Category '${name}' not found`);
    categoryId = row.id;
  } finally {
    db.close();
  }
  await page.waitForURL(`${serverUrl}/categories/${categoryId}/entries`);
});

Then("the entry row for {string} shows as read", async ({ page }, title) => {
  // _entry_row.html adds CSS class "entry-read" to the row when is_read is true.
  // There is no data-read attribute — the read state is conveyed by CSS class only.
  const row = page.getByTestId("entry-item").filter({ hasText: title }).first();
  await expect(row).toHaveClass(/entry-read/);
});
