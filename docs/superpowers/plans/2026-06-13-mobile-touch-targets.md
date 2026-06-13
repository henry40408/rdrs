# Mobile Touch-Target Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every interactive control reach a ≥44×44px tap target on mobile (≤1024px), CSS-only, verified by e2e.

**Architecture:** Add a single new design token (`--touch-min: 44px`) and a "mobile touch baseline" sub-section inside the existing `@media (max-width: 1024px)` block in `static/css/app.css`. Broad selectors raise the floor for all interactive elements; a few rules handle icon-only and text-only controls. A new generic e2e step asserts control sizes, exercised by new `@mobile` scenarios. No HTML/Rust/JS-behavior changes.

**Tech Stack:** CSS (`static/css/app.css`), Playwright-BDD e2e (`e2e/features/*.feature`, `e2e/steps/*.steps.js`).

---

## Notes for the implementer

- **Environment:** Before any e2e command, re-source the OpenSSL env on this box:
  `source /tmp/rdrs-env.sh`. The e2e global-setup builds and launches the `rdrs`
  binary, which needs it.
- **Run e2e from the `e2e/` dir.** `pwd` first (multi-project workspace rule).
- **Commits are GPG-signed** (`git commit -S`). End each commit message with the
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- **Stage files explicitly by name** — never `git add -A`/`.`.
- All new CSS rules MUST live **inside** the `@media (max-width: 1024px)` block
  (it ends at the lone `}` on line 1770) so desktop is untouched.

---

## Task 1: Add the e2e tap-target assertion step and failing scenarios

This is the failing test (TDD red). The two new scenarios assert ≥44px on
controls that are currently 22–38px, so they fail before the CSS change.

**Files:**
- Modify: `e2e/steps/responsive.steps.js` (append two step defs)
- Modify: `e2e/features/responsive.feature` (append two `@mobile` scenarios)

- [ ] **Step 1: Add the generic size-assertion steps**

Append to `e2e/steps/responsive.steps.js` (after the last step, line 125):

```javascript
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
```

- [ ] **Step 2: Add the failing scenarios**

Append to `e2e/features/responsive.feature` (after the last desktop scenario,
line 107):

```gherkin
  @mobile
  Scenario: Inbox and drawer controls meet the 44px touch minimum on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the ".sidebar-toggle" control is at least 44px wide
    And the ".sidebar-toggle" control is at least 44px tall
    And the ".entry-action-btn" control is at least 44px tall
    When I tap the hamburger
    Then the ".sidebar-close" control is at least 44px wide
    And the ".sidebar-close" control is at least 44px tall
    And the ".sidebar-item" control is at least 44px tall
    When I tap the sidebar close button
    And I click the entry titled "Test Entry 1"
    Then the reading pane is visible on mobile
    And the ".reading-pane-back-link" control is at least 44px tall

  @mobile
  Scenario: Table action links meet the 44px touch minimum on mobile
    Given I am viewing on a mobile screen
    And I have a category named "Test Category"
    When I open the categories page
    Then the ".action-link" control is at least 44px tall
```

- [ ] **Step 3: Run the new scenarios to verify they FAIL**

```bash
pwd   # expect /home/nixos/Develop/claude/rdrs
source /tmp/rdrs-env.sh
cd e2e && npx playwright test --grep @mobile
```

Expected: the two new scenarios FAIL on the size assertions (e.g.
`.sidebar-toggle` height ~38 < 44, `.entry-action-btn` height < 44,
`.action-link` height < 44). The 8 pre-existing `@mobile` scenarios still PASS.

- [ ] **Step 4: Commit the failing tests**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/steps/responsive.steps.js e2e/features/responsive.feature
git commit -S -m "test(e2e): assert 44px mobile tap targets (failing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add the `--touch-min` token and the mobile touch baseline CSS

This is the implementation (TDD green).

**Files:**
- Modify: `static/css/app.css` (`:root` token at line ~49; new rules inside the
  `@media (max-width: 1024px)` block; `.entry-item` padding at line 1737)

- [ ] **Step 1: Add the `--touch-min` token**

In `static/css/app.css`, in the `/* Borders */` area of `:root` (lines 48-49),
add the token after `--border-accent-width: 3px;`:

```css
    /* Borders */
    --border-accent-width: 3px;

    /* Touch */
    --touch-min: 2.75rem;   /* 44px — WCAG 2.5.5 / iOS minimum tap target */
```

- [ ] **Step 2: Fix the entry-row padding regression**

In `static/css/app.css`, the `@media (max-width: 1024px)` rule currently shrinks
the row padding (lines 1736-1738):

```css
    .entry-item {
        padding: var(--space-3) var(--space-4);
    }
```

Change it to keep the comfortable 16px vertical padding:

```css
    .entry-item {
        padding: var(--space-4);
    }
```

- [ ] **Step 3: Append the mobile touch baseline block**

In `static/css/app.css`, insert the following **inside** the
`@media (max-width: 1024px)` block, immediately before its closing `}` on
line 1770 (i.e. after the `...padding-top: var(--space-6); } }` rule that ends on
line 1769):

```css
    /* ===== Mobile touch baseline — 44px minimum tap targets ===== */

    /* Buttons are already inline-flex & centered (base styles); just raise
       them to the touch minimum. */
    button,
    .btn {
        min-height: var(--touch-min);
    }

    /* Link- and text-styled controls have little or no padding by default —
       turn them into real flex boxes so min-height centers the label and the
       whole box is tappable, not just the glyph run. */
    .action-link,
    .actions a,
    .tab-bar a,
    .reading-pane-nav-btn,
    .stats-period-btn,
    .feed-filter-link,
    .entry-action-btn,
    .reading-pane-back-link,
    .entry-item-meta a,
    .breadcrumb a,
    .search-result-title,
    .link a {
        display: inline-flex;
        align-items: center;
        min-height: var(--touch-min);
    }

    /* The entry-row toggles ship with padding:0 — give them horizontal room. */
    .entry-action-btn {
        padding: var(--space-1) var(--space-2);
    }

    /* Square icon-only controls: width AND height must reach the minimum. */
    .sidebar-toggle,
    .sidebar-close,
    .banner-dismiss {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-width: var(--touch-min);
        min-height: var(--touch-min);
    }

    /* Drawer rows. */
    .sidebar-item {
        min-height: var(--touch-min);
    }

    /* Form controls: touch height + 16px font to suppress iOS focus-zoom. */
    input[type="text"],
    input[type="password"],
    input[type="url"],
    input[type="email"],
    input[type="number"],
    input[type="search"],
    input[type="date"],
    select,
    textarea {
        min-height: var(--touch-min);
        font-size: var(--font-base);
    }

    /* Legibility: lift the smallest interactive/secondary text to 14px. */
    .entry-item-meta,
    .entry-item-actions,
    .action-link {
        font-size: var(--font-sm);
    }

    /* Anti-mis-tap: widen gaps between adjacent tappable controls. */
    .entry-item-actions {
        gap: var(--space-4);
    }

    .tab-bar {
        gap: var(--space-2);
    }
```

- [ ] **Step 4: Run the new scenarios to verify they PASS**

```bash
pwd
source /tmp/rdrs-env.sh
cd e2e && npx playwright test --grep @mobile
```

Expected: all `@mobile` scenarios PASS (the 8 pre-existing + the 2 new).

- [ ] **Step 5: Commit the CSS**

```bash
cd /home/nixos/Develop/claude/rdrs
git add static/css/app.css
git commit -S -m "feat(ui): enforce 44px touch targets on mobile

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Full responsive regression sweep

Confirm tablet/desktop scenarios still pass (the change is media-scoped, but
verify no leakage).

**Files:** none (verification only)

- [ ] **Step 1: Run the full responsive feature**

```bash
pwd
source /tmp/rdrs-env.sh
cd e2e && npx playwright test responsive
```

Expected: all `@mobile`, `@tablet`, `@desktop` scenarios PASS.

- [ ] **Step 2: Run the mobile keyboard-shortcuts feature (shares the overlay)**

```bash
cd /home/nixos/Develop/claude/rdrs
source /tmp/rdrs-env.sh
cd e2e && npx playwright test keyboard_shortcuts_mobile
```

Expected: PASS (reading-pane overlay still opens/closes correctly).

- [ ] **Step 3: If anything failed, fix and re-run**

If a tablet/desktop scenario regressed, confirm every new rule is inside the
`@media (max-width: 1024px)` block (desktop is 1280px wide, tablet 768px). The
tablet viewport (768px) IS within the ≤1024px range, so tablet now also gets the
larger targets — that is intended and the tablet scenarios assert layout, not
sizes, so they should still pass.

---

## Self-Review

**Spec coverage:**
- 44px min on all controls (Task 2 Step 3 baseline block). ✅
- New `--touch-min` token (Task 2 Step 1). ✅
- Text-only controls get real hit area (inline-flex group). ✅
- Icon-only specials `.banner-dismiss`, `.sidebar-close` (square-icon group). ✅
- `.sidebar-item` drawer rows. ✅
- Fix `.entry-item` padding regression (Task 2 Step 2). ✅
- Anti-mis-tap spacing (`.entry-item-actions`, `.tab-bar` gaps). ✅
- Font: 12.8px→14px for meta/actions; inputs→16px iOS anti-zoom. ✅
- e2e tap-target assertions + new scenarios (Task 1). ✅
- All pages covered via shared classes (buttons, action-link, inputs, tabs). ✅
- CSS-only, no HTML/Rust/JS changes. ✅

**Placeholder scan:** No TBD/TODO; all CSS and step code is concrete. ✅

**Consistency:** Step phrasing `the {string} control is at least {int}px tall/wide`
is identical in the step defs (Task 1 Step 1) and the scenarios (Task 1 Step 2).
Selectors used in scenarios (`.sidebar-toggle`, `.entry-action-btn`,
`.sidebar-close`, `.sidebar-item`, `.reading-pane-back-link`, `.action-link`) all
have matching rules in Task 2 Step 3. ✅
