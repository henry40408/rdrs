// Exploratory walkthrough of the whole app with scripting genuinely off.
//
// The Rust suite asserts the paths we thought of; this drives a real browser
// with `javaScriptEnabled: false` and reports what a scriptless reader actually
// runs into. It prints findings and does not gate CI — the point is discovery.
//
//   cd e2e && npx playwright test --config=scripts/audit.config.js scripts/nojs-walkthrough.spec.js
import { test, expect } from "@playwright/test";
import { spawnRdrs, spawnMockFeedServer } from "../support/server.js";
import { SeedHelper } from "../support/seed.js";

test.use({ javaScriptEnabled: false });

const USER = "walker";
const PASS = "vulture-mango-77-quilt";

const findings = [];
const note = (where, what) => findings.push(`${where}: ${what}`);

/** Submit a form the way a browser does, by clicking its submit control. */
async function submit(page, formSelector, submitSelector) {
  await page.locator(formSelector).locator(submitSelector).first().click();
  await page.waitForLoadState("domcontentloaded");
}

test("a reader with JavaScript disabled can work the app", async ({ page, context }) => {
  const server = await spawnRdrs();
  const feedServer = await spawnMockFeedServer();

  // Belt and braces: even with javaScriptEnabled:false, make sure no script
  // could execute if the flag were ever misapplied.
  await context.route("**/*.js", (route) => route.abort());

  try {
    // ── 1. First-run setup ────────────────────────────────────────────────
    await page.goto(`${server.url}/setup`);
    await page.fill("#username", USER);
    await page.fill("#password", PASS);
    const confirm = page.locator("#confirm-password");
    if (await confirm.count()) await confirm.fill(PASS);
    await submit(page, "form", 'button[type="submit"]');
    if (!page.url().includes("/login")) {
      note("setup", `expected to land on /login, got ${page.url()}`);
    }

    // ── 2. Sign in ────────────────────────────────────────────────────────
    await page.fill("#username", USER);
    await page.fill("#password", PASS);
    await submit(page, 'form[action="/login"]', 'button[type="submit"]');
    if (page.url().includes("/login")) {
      note("login", `still on /login after submitting: ${await page.title()}`);
    }

    // ── 3. Navigation exists at all ───────────────────────────────────────
    const nav = page.locator("nav.nav-fallback");
    if ((await nav.count()) === 0) {
      note("nav", "no scriptless navigation on the landing page");
    } else {
      const links = await nav.locator("a").evaluateAll((as) =>
        as.map((a) => a.getAttribute("href")),
      );
      // Walk every destination and record anything that is not a 200.
      for (const href of [...new Set(links)]) {
        const res = await page.goto(`${server.url}${href}`);
        if (!res || res.status() !== 200) {
          note("nav", `${href} returned ${res && res.status()}`);
        }
        if ((await page.locator("nav.nav-fallback").count()) === 0) {
          note("nav", `${href} renders without the navigation`);
        }
      }
    }

    // ── 4. Subscribe to a feed ────────────────────────────────────────────
    await page.goto(`${server.url}/feeds`);
    const addForm = page.locator('form[action="/feeds"][method="post"]').first();
    if ((await addForm.count()) === 0) {
      note("feeds", "no add-feed form");
    } else {
      await addForm.locator('input[name="url"]').fill(`${feedServer.url}/feed.xml`);
      await submit(page, 'form[action="/feeds"][method="post"]', 'button[type="submit"]');
      const banner = page.locator('[data-testid="flash-message"]');
      if ((await banner.count()) === 0) {
        note("feeds", "adding a feed produced no visible confirmation");
      }
    }

    // ── 5. The filter bar (the button this branch adds) ────────────────────
    await page.goto(`${server.url}/feeds`);
    const apply = page.locator('[data-testid="feed-filter-apply"]');
    if ((await apply.count()) === 0) {
      note("feeds", "filter bar has no way to submit without scripting");
    } else {
      await page.selectOption("#sort-by", "unread");
      await apply.click();
      await page.waitForLoadState("domcontentloaded");
      if (!page.url().includes("sort=unread")) {
        note("feeds", `Apply did not carry the sort: ${page.url()}`);
      }
    }

    // ── 6. Categories: create, and see it ─────────────────────────────────
    await page.goto(`${server.url}/categories`);
    const catForm = page.locator('form[action="/categories"][method="post"]').first();
    if ((await catForm.count()) === 0) {
      note("categories", "no create form");
    } else {
      await catForm.locator('input[name="name"]').fill("Walkthrough");
      await submit(page, 'form[action="/categories"][method="post"]', 'button[type="submit"]');
      // The name comes back inside the rename form's input, not as text.
      const names = await page
        .locator('form.cat-rename input[name="name"]')
        .evaluateAll((els) => els.map((e) => e.value));
      if (!names.includes("Walkthrough")) {
        note("categories", `the created category is not listed afterwards (${names})`);
      }
      if ((await page.locator('[data-testid="flash-message"]').count()) === 0) {
        note("categories", "creating a category produced no confirmation");
      }
    }

    // ── 7. Read an entry, star it, mark it unread ─────────────────────────
    // The mock feed carries no items, so seed real ones the way the BDD suite
    // does — otherwise the most important flow in a reader goes unwalked.
    {
      const seed = new SeedHelper(server.dbPath);
      const userId = seed.getUserId(USER);
      const catId = seed.createCategory(userId, "Walk");
      const feedId = seed.createFeed(catId, "https://example.invalid/walk", "Walk Feed");
      seed.insertEntries(
        Array.from({ length: 3 }, (_, i) => ({
          feedId,
          guid: `walk-${i}`,
          title: `Walk entry ${i}`,
          link: `https://example.invalid/walk/${i}`,
          content: `<p>Body ${i}</p>`,
        })),
      );
    }
    await page.goto(`${server.url}/entries`);
    const title = page.locator('[data-testid="entry-title-link"]').first();
    if ((await title.count()) === 0) {
      note("entries", "no entries to read (the mock feed may be empty) — skipped");
    } else {
      await title.click();
      await page.waitForLoadState("domcontentloaded");
      if (!page.url().includes("entry=")) {
        note("entries", `opening an entry did not land on a pane URL: ${page.url()}`);
      }
      const pane = page.locator("#reading-pane");
      if ((await pane.count()) === 0) {
        note("entries", "the reading pane did not render after opening an entry");
      }

      const star = page.locator('[data-testid="entry-star-action"]').first();
      if ((await star.count()) === 0) {
        note("entries", "no star control");
      } else {
        await star.click();
        await page.waitForLoadState("domcontentloaded");
        if ((await page.locator(".entry-star.starred").count()) === 0) {
          note("entries", "starring did not visibly take effect");
        }
      }

      const toggle = page.locator('[data-testid="entry-read-toggle"]').first();
      if ((await toggle.count()) === 0) {
        note("entries", "no read/unread toggle");
      } else {
        await toggle.click();
        await page.waitForLoadState("domcontentloaded");
      }
    }

    // ── 8. Search ─────────────────────────────────────────────────────────
    await page.goto(`${server.url}/search`);
    const searchForm = page.locator("form").filter({ has: page.locator('input[name="q"]') }).first();
    if ((await searchForm.count()) === 0) {
      note("search", "no search form");
    } else {
      await searchForm.locator('input[name="q"]').fill("test");
      const btn = searchForm.locator('button[type="submit"]');
      if ((await btn.count()) === 0) {
        note("search", "search form has no submit control");
      } else {
        await btn.first().click();
        await page.waitForLoadState("domcontentloaded");
        if (!page.url().includes("q=test")) {
          note("search", `search did not navigate: ${page.url()}`);
        }
      }
    }

    // ── 9. Preferences ────────────────────────────────────────────────────
    await page.goto(`${server.url}/user-settings`);
    const prefs = page.locator('form[action="/user-settings/preferences"]').first();
    if ((await prefs.count()) === 0) {
      note("preferences", "no preferences form");
    } else if ((await prefs.locator('button[type="submit"]').count()) === 0) {
      note("preferences", "preferences form has no submit control");
    }

    // ── 10. Sign out, and confirm the session is actually gone ────────────
    await page.goto(`${server.url}/`);
    const logout = page.locator('form[action="/logout"] button[type="submit"]').first();
    if ((await logout.count()) === 0) {
      note("logout", "no way to sign out");
    } else {
      await logout.click();
      await page.waitForLoadState("domcontentloaded");
      if (!page.url().includes("/login")) {
        note("logout", `did not land on /login: ${page.url()}`);
      }
      // The session must really be over, not just redirected away from.
      await page.goto(`${server.url}/`);
      if (!page.url().includes("/login")) {
        note("logout", "still signed in after signing out");
      }
    }
  } finally {
    await feedServer.cleanup?.();
    await server.cleanup();
  }

  console.log("\n=== no-JS walkthrough findings ===");
  if (findings.length === 0) console.log("(none)");
  for (const f of findings) console.log(`- ${f}`);
  console.log("=== end ===\n");

  // Report-only, like touch-audit: this is discovery, not a gate.
  expect(true).toBe(true);
});
