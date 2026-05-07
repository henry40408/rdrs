# SPA Router Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace cross-page full-reload navigation with in-place client-side routing for every CSR page (all 13 routes spanning 7 distinct page elements). After this PR, clicking the sidebar or in-page links does not trigger a `Document` reload — the page-element is swapped in place via `history.pushState`, eliminating the cross-page flicker.

**Architecture:** A standalone `static/js/router.js` module loaded by `app_shell.html` after the page-module script. It owns a hard-coded route table (URL pattern → element tag + script path) mirroring the server's URL → handler map, attaches a single document-level capturing click listener for `<a>` interception, and a `popstate` listener for browser back/forward. `navigateTo(path)` dynamically imports the page module on demand (browser-cached by URL, no shipping all modules eagerly), creates a fresh page element, and replaces the contents of a new `#page-host` wrapper div. Page elements do NOT import the router; they emit standard `<a>` links and the router intercepts at the document level.

**Tech Stack:** Vanilla JS (native custom elements, ES modules, `history.pushState`, no framework or build step) + Askama template (one-line wrapper edit) + Playwright e2e for coverage.

**Spec:** [`docs/superpowers/specs/2026-05-07-spa-router-design.md`](../specs/2026-05-07-spa-router-design.md)

**Branch:** `refactor/spa-router` (already cut from current `main`, holds the spec commit).

**Environment:** Source `/tmp/rdrs-env.sh` before every `cargo` / `cargo nextest` / `npm` / Playwright invocation.

---

## File Structure

| File | Status | Responsibility |
|------|--------|---------------|
| `static/js/router.js` | NEW | Route table, click + popstate listeners, `navigateTo()`, page-element swap, sequence guard for in-flight nav |
| `templates/app_shell.html` | EDIT | Wrap `<{{ element_tag }}>` in `<div id="page-host">…</div>`; add `<script type="module" src="/static/js/router.js?v={{ git_version }}">` after the page-module script |
| `src/handlers/static_assets.rs` | EDIT | Add `js/router.js` to the `FILES` allowlist |
| `e2e/tests/spa-router.spec.ts` | NEW | Verify same-element nav, cross-element nav, popstate, click filters, fallback to full reload for unmatched routes |

`<rdrs-entries-page>` and the other six page elements are **not** modified. Their `connectedCallback` already does the right thing on each fresh mount (read sidebar bootstrap, fetch user-settings, render). The router just constructs them in the right order.

---

## Task 1: Scaffold router + wire shell + asset allowlist

Land the plumbing first: an empty router, the new `<div id="page-host">` wrapper, and the asset registration. Subsequent tasks add behaviour.

**Files:**
- Create: `static/js/router.js`
- Modify: `templates/app_shell.html` (two small edits)
- Modify: `src/handlers/static_assets.rs:8-63` (append entry to `FILES`)

- [ ] **Step 1: Create the empty router scaffold**

```js
// static/js/router.js
// SPA router — intercepts internal-link clicks and swaps the page element
// in place instead of triggering a full document reload.
//
// Loaded by app_shell.html after the page-module script. The first paint is
// still server-rendered (handler returns the shell with element_tag +
// script_path); the router takes over from there.
//
// Filled in by the next commit. This commit just lands the file so the
// shell's <script> tag has something to load.

console.debug('[rdrs-router] loaded');
```

- [ ] **Step 2: Add the wrapper div + script tag in `app_shell.html`**

In `templates/app_shell.html`, find the line:

```html
    <{{ element_tag }}></{{ element_tag }}>
```

Replace with:

```html
    <div id="page-host"><{{ element_tag }}></{{ element_tag }}></div>
```

In the same file, find the existing page-module script line (around line 60):

```html
    <script type="module" src="{{ script_path }}?v={{ git_version }}"></script>
```

Append immediately after it:

```html
    <script type="module" src="/static/js/router.js?v={{ git_version }}"></script>
```

- [ ] **Step 3: Register `router.js` in the static-assets allowlist**

In `src/handlers/static_assets.rs`, the `FILES` array (around line 8-63) ends with the most recently added page entry. Append before the closing `];`:

```rust
    ("js/router.js", include_str!("../../static/js/router.js")),
```

- [ ] **Step 4: Build to verify the embedded source compiles + the shell renders**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
```

Expected: clean build.

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test test_unread_page_returns_shell 2>&1 | tail -5
```

Expected: pass. The shell still renders; `<rdrs-entries-page>` is now wrapped in `<div id="page-host">` but the test only asserts the element-tag substring, which is unchanged.

- [ ] **Step 5: Commit**

```bash
pwd  # /home/nixos/Develop/claude/rdrs
git add static/js/router.js templates/app_shell.html src/handlers/static_assets.rs
git commit -S -m "$(cat <<'EOF'
feat(spa): scaffold SPA router module

Lands an empty router.js plus the shell-side wiring: page element is
now wrapped in <div id="page-host"> (the swap target the router will
use), and a <script type="module" src="/static/js/router.js"> tag
loads after the page module. Behaviour added in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Write the failing e2e spec

Test-first: spec defines what the router must do. With the no-op router from Task 1, every assertion that requires SPA behaviour must fail. The spec exercises every code path that Task 3 will then implement.

**Files:**
- Create: `e2e/tests/spa-router.spec.ts`

- [ ] **Step 1: Create the spec**

```ts
// e2e/tests/spa-router.spec.ts
import { Page } from "@playwright/test";
import { test, expect } from "../fixtures/rdrs.js";

test.describe("SPA router", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("spauser", "password123");
    const userId = seed.getUserId("spauser");
    const catId = seed.createCategory(userId, "Cat A");
    const feedId = seed.createFeed(catId, "https://example.com/cat-a.xml", "Feed A");
    seed.seedTestEntries(feedId, 5);
  });

  async function login(page: Page, serverUrl: string): Promise<void> {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("spauser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  }

  /**
   * Counts top-level Document loads. SPA navigation must not increment.
   * Captures every navigation request (incl. redirects) on the main frame.
   */
  function trackDocumentLoads(page: Page): { count: () => number; dispose: () => void } {
    let count = 0;
    const handler = (req: { resourceType(): string; isNavigationRequest(): boolean; frame(): { parentFrame(): unknown } }) => {
      if (
        req.isNavigationRequest() &&
        req.resourceType() === "document" &&
        !req.frame().parentFrame()
      ) {
        count += 1;
      }
    };
    page.on("request", handler);
    return {
      count: () => count,
      dispose: () => page.off("request", handler),
    };
  }

  test("same-element nav: /entries -> /entries/starred (no document reload)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.goto(`${serverUrl}/entries`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.getByTestId("tab-starred").click();
      await expect(page).toHaveURL(`${serverUrl}/entries/starred`);
      // Page element is the same instance (mode swapped). Header rerenders.
      await expect(page.locator("rdrs-entries-page")).toHaveAttribute("data-mode", "starred");
      // Allow async settling.
      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("cross-element nav: / -> /feeds -> /admin (no document reload)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await expect(page.locator("rdrs-entries-page")).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.getByTestId("nav-feeds").click();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();
      await expect(page.locator("rdrs-entries-page")).toHaveCount(0);

      await page.getByTestId("nav-admin").click();
      await expect(page).toHaveURL(`${serverUrl}/admin`);
      await expect(page.locator("rdrs-admin-page")).toBeVisible();

      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("popstate: back/forward swaps elements correctly", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    await page.getByTestId("nav-feeds").click();
    await expect(page.locator("rdrs-feeds-page")).toBeVisible();
    await page.getByTestId("nav-admin").click();
    await expect(page.locator("rdrs-admin-page")).toBeVisible();

    const tracker = trackDocumentLoads(page);
    try {
      await page.goBack();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();

      await page.goBack();
      await expect(page).toHaveURL(`${serverUrl}/`);
      await expect(page.locator("rdrs-entries-page")).toBeVisible();

      await page.goForward();
      await expect(page).toHaveURL(`${serverUrl}/feeds`);
      await expect(page.locator("rdrs-feeds-page")).toBeVisible();

      await page.waitForTimeout(200);
      expect(tracker.count()).toBe(0);
    } finally {
      tracker.dispose();
    }
  });

  test("modifier-click and target=_blank fall through to browser default", async ({ page, serverUrl, context }) => {
    await login(page, serverUrl);

    // Cmd/Ctrl-click should open in a new tab (browser default), NOT swap in place.
    const popupPromise = context.waitForEvent("page");
    await page.getByTestId("nav-feeds").click({ modifiers: ["ControlOrMeta"] });
    const popup = await popupPromise;
    await popup.waitForLoadState();
    await expect(popup).toHaveURL(`${serverUrl}/feeds`);
    await popup.close();

    // The originating tab stayed on /.
    await expect(page).toHaveURL(`${serverUrl}/`);
  });

  test("non-routed link triggers full reload", async ({ page, serverUrl }) => {
    await login(page, serverUrl);

    const tracker = trackDocumentLoads(page);
    try {
      // Click an external-style link by injecting one and clicking it.
      await page.evaluate(() => {
        const a = document.createElement("a");
        a.href = "/login";
        a.textContent = "go login";
        a.id = "test-fallback-link";
        document.body.appendChild(a);
      });
      await page.click("#test-fallback-link");
      await page.waitForURL(`${serverUrl}/login`);
      // /login isn't in the route table → router lets the browser navigate.
      expect(tracker.count()).toBeGreaterThanOrEqual(1);
    } finally {
      tracker.dispose();
    }
  });
});
```

- [ ] **Step 2: Run the spec — expect failures**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test tests/spa-router.spec.ts 2>&1 | tail -20
```

Expected: 4 of 5 tests fail (same-element, cross-element, popstate, modifier-click). The "non-routed link" test passes because Task 1's no-op router doesn't intercept — full reloads happen normally.

- [ ] **Step 3: Commit the failing spec**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/tests/spa-router.spec.ts
git commit -S -m "$(cat <<'EOF'
test(spa): write failing e2e spec for SPA router

Covers same-element nav (tab swap inside <rdrs-entries-page>),
cross-element nav (sidebar feeds -> admin), popstate back/forward,
modifier-click bypass, and full-reload fallback for routes not in
the table. The router is still a no-op so the first four fail —
Task 3 implements the actual logic.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Implement the router

The full router behaviour: route table, navigateTo with sequence guard + dynamic import, document-level click intercept with all the standard filters, popstate.

**Files:**
- Modify: `static/js/router.js` (replace the scaffold from Task 1)

- [ ] **Step 1: Replace the scaffold with the full implementation**

Overwrite `static/js/router.js`:

```js
// static/js/router.js
// SPA router — intercepts internal-link clicks and swaps the page element
// in place instead of triggering a full document reload. Loaded by
// app_shell.html after the page-module script.
//
// First paint is still server-rendered (handler returns the shell with
// element_tag + script_path). The router takes over from there:
//   - Document-level click handler intercepts internal <a> clicks.
//   - history.pushState updates the URL.
//   - Dynamic import() loads the matching page module (cached after first).
//   - The fresh page element replaces #page-host's contents; its
//     connectedCallback runs and the page initialises normally.
//
// Page-element modules do NOT import this file. They emit plain
// <a href="/..."> links. The router intercepts at the document level.

const ROUTES = [
    { pattern: /^\/$/,                                      element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/entries$/,                               element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/entries\/(?:read|starred|summarized)$/,  element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/feeds\/\d+\/entries$/,                   element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/categories\/\d+\/entries$/,              element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/search$/,                                element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/feeds$/,                                 element: 'rdrs-feeds-page',         script: '/static/js/pages/feeds.js' },
    { pattern: /^\/categories$/,                            element: 'rdrs-categories-page',    script: '/static/js/pages/categories.js' },
    { pattern: /^\/admin$/,                                 element: 'rdrs-admin-page',         script: '/static/js/pages/admin.js' },
    { pattern: /^\/settings$/,                              element: 'rdrs-settings-page',      script: '/static/js/pages/settings.js' },
    { pattern: /^\/user-settings$/,                         element: 'rdrs-user-settings-page', script: '/static/js/pages/user-settings.js' },
    { pattern: /^\/statistics$/,                            element: 'rdrs-statistics-page',    script: '/static/js/pages/statistics.js' },
];

function matchRoute(pathname) {
    return ROUTES.find(r => r.pattern.test(pathname)) ?? null;
}

let navSeq = 0;

async function navigateTo(path, opts = {}) {
    const url = new URL(path, location.origin);
    const route = opts.route ?? matchRoute(url.pathname);
    if (!route) {
        // Unknown path — let the browser navigate. Treat the in-app call
        // like a regular full-page transition.
        location.href = path;
        return;
    }

    if (!opts.skipPushState) {
        history.pushState(null, '', path);
    }

    const mySeq = ++navSeq;
    try {
        await import(route.script);
    } catch (err) {
        // Module fetch failed (network, server 5xx). Fall back to a full
        // reload so the user gets either the server-rendered shell or a
        // recoverable error page.
        location.href = path;
        return;
    }
    if (mySeq !== navSeq) return;     // superseded by a later nav

    const host = document.getElementById('page-host');
    if (!host) {
        // Shell didn't render the host (shouldn't happen in production).
        // Fall back to full reload rather than mounting somewhere weird.
        location.href = path;
        return;
    }
    const newEl = document.createElement(route.element);
    host.replaceChildren(newEl);

    if (!opts.skipPushState) {
        window.scrollTo(0, 0);
    }
    // popstate-driven nav inherits the browser's auto scroll restoration.
}

function shouldIntercept(event, anchor) {
    if (event.defaultPrevented) return false;
    if (event.button !== 0) return false;                                      // right/middle click
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return false;
    if (anchor.target && anchor.target !== '' && anchor.target !== '_self') return false;
    if (anchor.hasAttribute('download')) return false;
    const rel = anchor.getAttribute('rel');
    if (rel && rel.split(/\s+/).includes('external')) return false;

    let url;
    try {
        url = new URL(anchor.href, location.origin);
    } catch {
        return false;
    }
    if (url.origin !== location.origin) return false;

    return matchRoute(url.pathname) !== null;
}

document.addEventListener('click', (event) => {
    const anchor = event.target.closest('a');
    if (!anchor) return;
    if (!shouldIntercept(event, anchor)) return;

    const url = new URL(anchor.href, location.origin);
    event.preventDefault();
    navigateTo(url.pathname + url.search);
});

window.addEventListener('popstate', () => {
    navigateTo(location.pathname + location.search, { skipPushState: true });
});
```

- [ ] **Step 2: Build (Rust embeds the new content via include_str!)**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
```

Expected: clean build.

- [ ] **Step 3: Run the SPA spec — expect pass**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test tests/spa-router.spec.ts 2>&1 | tail -10
```

Expected: 5 / 5 pass.

- [ ] **Step 4: Run the rest of the e2e suite to catch regressions**

```bash
source /tmp/rdrs-env.sh && npx playwright test 2>&1 | tail -10
```

Expected: all green except the documented `entry-actions :: keyboard s toggles star` flake (pre-existing on `main` since #170).

If any *non-flake* spec fails, debug before committing. Common regressions to watch for:

- A page-element's `connectedCallback` does something that assumed first-paint state (e.g. reads bootstrap JSON only). The router mounts a *fresh* element on each nav; subsequent mounts re-read the same inlined bootstrap (which is fine — bootstrap stays in the DOM). If an element wrote-once-and-removed the bootstrap script tag, that's a bug worth fixing.
- A page-element's `disconnectedCallback` doesn't clean up window listeners. Each nav adds another listener; over time, leaks accumulate. `<rdrs-entry-list>` already cleans up; new pages should be checked.
- popstate handlers in `<rdrs-entry-list>` may fire alongside the router's. The list element registers a `popstate` listener for its own `?entry=N` reading-pane handling. When the router-driven popstate fires, the element handler also runs — but on the OLD instance which is about to be removed. Verify no errors thrown.

- [ ] **Step 5: Format check + commit**

```bash
cd /home/nixos/Develop/claude/rdrs
source /tmp/rdrs-env.sh && cargo fmt --check
git add static/js/router.js
git commit -S -m "$(cat <<'EOF'
feat(spa): implement router — full SPA navigation across CSR routes

Document-level click handler intercepts internal <a> clicks (with the
standard filters: modifier keys, target=_blank, download, rel=external,
cross-origin), looks the URL up in a hard-coded route table, and either
navigates in-place via history.pushState + dynamic import + element swap
or falls through to a regular full-reload for unrecognised paths.

The route table mirrors the server's URL → handler map. There are 13
CSR routes across 7 distinct page elements; the entries family (8
routes) shares <rdrs-entries-page> which infers its mode from
location.pathname on connect.

Sequence guard discards a nav whose import landed after a newer one
started. popstate is wired with skipPushState so back/forward never
push their own entry. Module fetch failures fall back to a full
reload.

Page-element code is unchanged. They keep emitting <a href="/..."> and
the router intercepts at document level.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Final verification + push + open PR + STOP

- [ ] **Step 1: Full Rust suite + clippy + fmt**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -5
source /tmp/rdrs-env.sh && cargo fmt --check
source /tmp/rdrs-env.sh && cargo clippy -- -D warnings 2>&1 | tail -5
```

All clean.

- [ ] **Step 2: Full e2e suite**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test 2>&1 | tail -10
```

Expected: green except the documented `entry-actions :: keyboard s toggles star` flake.

- [ ] **Step 3: Restore screenshots if regenerated**

```bash
cd /home/nixos/Develop/claude/rdrs
git status --short | grep screenshots && git restore screenshots/ || true
```

- [ ] **Step 4: Push + open PR**

```bash
git push -u origin refactor/spa-router
```

```bash
gh pr create --title "feat(spa): client-side router for in-place navigation across CSR routes" --body "$(cat <<'EOF'
## Summary

Step 8 (final) of the CSR migration. Adds a single \`static/js/router.js\` module that intercepts internal \`<a>\` clicks and swaps the page element in place, eliminating the cross-page reload flicker. Covers all 13 CSR routes across 7 distinct page elements (the entries family of 8 routes shares \`<rdrs-entries-page>\`).

**Spec:** \`docs/superpowers/specs/2026-05-07-spa-router-design.md\`
**Plan:** \`docs/superpowers/plans/2026-05-07-spa-router.md\`
**Predecessors:** #175 (B1), #176 (B2), #177 (B3) — together completed the CSR migration of the entries family.

## What changed

### New
- \`static/js/router.js\` — route table, document-level click intercept, \`navigateTo()\` with sequence guard and dynamic import, popstate handler.
- \`e2e/tests/spa-router.spec.ts\` — covers same-element nav, cross-element nav, popstate restore, modifier-click bypass, and full-reload fallback for routes not in the table.

### Edited
- \`templates/app_shell.html\` — page element is now wrapped in \`<div id=\"page-host\">\` (the swap target the router uses); \`<script type=\"module\" src=\"/static/js/router.js\">\` is loaded after the page-module script.
- \`src/handlers/static_assets.rs\` — \`js/router.js\` registered in the \`FILES\` allowlist.

### Untouched
- Every page element. They keep emitting plain \`<a href=\"/...\">\` links and the router intercepts at the document level.
- Every server handler. First-paint behaviour is unchanged: handler returns the shell with \`element_tag\` + \`script_path\`; router takes over from there.
- Sidebar / flash bootstrap. Sidebar has its own update lifecycle (mark-as-read invalidation, etc.), not router-driven.

## Behaviour

- Sidebar click \`/feeds\` from \`/\` → no document reload, \`<rdrs-entries-page>\` swapped for \`<rdrs-feeds-page>\`, URL updates via \`pushState\`.
- Tab \`All / Read / Starred / Summarized\` from any entries route → no document reload, same \`<rdrs-entries-page>\` instance is replaced with a fresh one (re-renders header + list for the new mode).
- Cmd-click, target=_blank, external link, rel=external → router stays out of the way (browser default).
- Module fetch fail / unknown route / missing \`#page-host\` → fall back to \`location.href = path\` (server-rendered shell or error page).
- Browser back/forward → \`popstate\` handler navigates without pushing a new history entry; browser auto-restores scroll.

## Test plan

- [ ] \`source /tmp/rdrs-env.sh && cargo nextest run\` — all pass
- [ ] \`source /tmp/rdrs-env.sh && cd e2e && npx playwright test\` — all pass except the documented \`entry-actions :: keyboard s toggles star\` flake (pre-existing on \`main\` since #170)
- [ ] Manually click around: sidebar links, tabs, breadcrumbs in feed/category views — verify the page snaps in place without a flash
- [ ] Manually verify browser back/forward across mixed routes (\`/\` → \`/feeds\` → \`/admin\` → back → back → forward)
- [ ] Manually verify cmd-click opens in a new tab; \"View Original\" button in reading pane goes to the external URL
- [ ] Manually verify logout still works (uses \`flash.redirect\` → full reload to \`/login\`, by design)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: STOP — manual review**

Surface PR URL and stop. Do **not** merge.

---

## Self-Review

**Spec coverage:**

- New `static/js/router.js` → Tasks 1 + 3. ✓
- Route table mirroring server handlers → Task 3. ✓
- Click intercept with all filters → Task 3 + Task 2 spec coverage. ✓
- popstate handling → Task 3 + Task 2 spec coverage. ✓
- `app_shell.html` wrapper + script tag → Task 1. ✓
- `static_assets.rs` allowlist → Task 1. ✓
- Sequence guard → Task 3. ✓
- Module-fetch fallback → Task 3. ✓
- Scroll behaviour (top on push, browser-restore on popstate) → Task 3. ✓
- E2E coverage → Task 2. ✓

**Placeholder scan:** None. Every code block is concrete; every shell command has expected output described.

**Type / signature consistency:**

- `navigateTo` opts: `{ route, skipPushState }` — used consistently in click handler (none) and popstate (`skipPushState: true`). ✓
- `matchRoute` returns `route | null`; `navigateTo` checks for null. ✓
- Page-element tags in route table all match the names emitted by their respective `customElements.define()` calls (verified in `static/js/pages/*.js`). ✓
- `#page-host` referenced in router matches the wrapper added in `app_shell.html`. ✓

**Risks pre-flagged in plan:**

- Page-element `connectedCallback` may consume bootstrap JSON destructively. Task 3 step 4 calls this out as a regression to watch for; the e2e suite catches it.
- popstate from `<rdrs-entry-list>` fires alongside router's. Same step 4. The list element's handler runs before the router replaces the host children, so it operates on its own (still-mounted) instance.
- First module-fetch after deploy is uncached (router uses unversioned URL). Documented in spec as acceptable trade-off; not a regression.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-07-spa-router.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute in this session using executing-plans.

Per user instruction (manual review at PR open), execute inline and stop at PR creation.
