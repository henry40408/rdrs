# Mobile Touch-Target Enhancement — Design

**Date:** 2026-06-13
**Branch:** `feat/mobile-touch-targets`
**Status:** Approved design, pending implementation plan

## Problem

On mobile, most interactive elements are too small — hard to tap and easy to
mis-tap. An audit of `static/css/app.css`, the Askama templates, and the
responsive infrastructure confirmed the root cause:

- The design tokens (`--space-1`=4px, `--space-2`=8px, `--font-xs`=12.8px) are
  tuned for a compact **desktop** layout.
- The `@media (max-width: 1024px)` block does **not** enlarge any touch target.
  It actively *shrinks* `.entry-item` vertical padding from 16px → 12px
  (`app.css:1736`).
- No media query raises button/link/select sizes or interactive font sizes for
  touch.

Confirmed-good baseline (no change needed):
- Viewport meta is correct: `width=device-width, initial-scale=1.0`
  (`templates/base.html:5`).
- The sidebar drawer JS (`toggleSidebar`/`closeSidebar`, tap-outside-to-close)
  and the reading-pane overlay JS already work.
- The e2e harness (`e2e/features/responsive.feature`, `responsive.steps.js`,
  `VIEWPORTS.mobile = 375×667`) is in place — but has **no** tap-target size
  assertions.

### Worst offenders (from audit)

Text-only, `padding: 0` (effective target ≈ text box only):
- `.entry-action-btn` (entry row read/star toggles) — repeated on every row
- `.reading-pane-back-link` (mobile-only escape route)
- `.entry-item-meta a`, `.breadcrumb a`, `.search-result-title`, `.link a`

Fixed-tiny / compact:
- `.banner-dismiss` — fixed 22×22px
- `.sidebar-close` — `padding: var(--space-1)` (4px)
- `.action-link` / `.btn-sm` / `.actions a` — 4×8px padding + 12.8px font
  (table actions in feeds/categories/admin, reading-pane action buttons,
  "Mark Above as Read")
- `.tab-bar a` — 8×12px, tabs only 4px apart
- `.stats-period-btn`, `.feed-filter-link`, `.stats-date-input` — 4px vertical
  padding

All fall below the **44×44px** WCAG 2.5.5 / iOS touch minimum.

## Goals

- Every interactive control reaches a **≥44×44px** effective tap target on
  mobile (≤1024px viewport).
- Reduce mis-tap risk by widening gaps between adjacent tappable controls.
- Raise the smallest interactive font sizes for legibility and to prevent iOS
  focus-zoom.
- Cover **all** pages (entry list, reading pane, sidebar, tables, statistics,
  auth) — most share the same classes, so one pass covers them.

## Non-Goals

- No HTML/template changes — this is a **CSS-only** change so it stays
  centralized and low-regression.
- No new font-size tokens; reuse existing `--font-sm`/`--font-base`.
- No change to desktop layout or to the existing drawer/overlay behavior.
- Main body text scale is unchanged (`body` is already 18px).

## Approach

**Global touch baseline + minimal token tuning**, applied inside the existing
`@media (max-width: 1024px)` block in `static/css/app.css`. A small set of
broad selectors raises the floor for all interactive elements; a few special
cases handle icon-only controls.

### 1. New token (`:root`)

```css
--touch-min: 2.75rem;   /* 44px — WCAG 2.5.5 / iOS touch minimum */
```

This is the only new token. Define it near the existing spacing tokens.

### 2. Global touch baseline (inside `@media (max-width: 1024px)`)

A new sub-section that:

- **Padded controls** — `button, .btn, .action-link, .actions a, .tab-bar a,
  .reading-pane-nav-btn, .stats-period-btn, .feed-filter-link, select, input,
  textarea` → `min-height: var(--touch-min)`. Where the element is not already
  flex, set `display: inline-flex; align-items: center` so the increased height
  vertically centers the label. (Block/full-width buttons keep their width.)
- **Text-only controls** — `.entry-action-btn, .reading-pane-back-link,
  .entry-item-meta a, .breadcrumb a, .search-result-title, .link a` → add
  `min-height: var(--touch-min)`, vertical padding, and
  `display: inline-flex; align-items: center` so the hit area is a real block,
  not just the glyph run.
- **Icon-only specials**:
  - `.banner-dismiss` → `min-width/min-height: var(--touch-min)` (from 22×22).
  - `.sidebar-close` → padding raised to reach 44×44 (from `--space-1`).
- **Sidebar items** — `.sidebar-item` → `min-height: var(--touch-min)` for the
  drawer.

### 3. Fix the reverse setting

`.entry-item` mobile padding (`app.css:1736`) currently shrinks to
`var(--space-3)` (12px) vertical. Restore to `var(--space-4)` (16px) vertical so
the most-used list rows stay comfortable.

### 4. Anti-mis-tap spacing

Widen gaps between adjacent tappable controls on mobile:
- `.entry-item-actions` gap (currently `--space-3`) → larger.
- `.actions` / `.tab-bar` gaps → larger.

### 5. Interactive font sizes (minimal)

- Raise mobile `--font-xs` contexts that are interactive/secondary —
  `.entry-item-meta`, `.entry-item-actions`, `.action-link` — to
  `var(--font-sm)` (14px).
- Ensure `input, select, textarea` render at `var(--font-base)` (16px) on
  mobile to suppress iOS focus-zoom (some are currently 14px).

Main text (entry title, sidebar item label) is **not** scaled up — per the
"only fix the too-small" decision.

## Testing

Add e2e tap-target assertions (matching existing conventions) so the change is
verified and protected from regression:

- New step in `e2e/steps/responsive.steps.js`:
  `Then("the {string} control is at least {int}px tall")` (and `…wide`) using
  `boundingBox()`.
- New `@mobile` scenario in `e2e/features/responsive.feature` asserting key
  controls are ≥44px on the mobile viewport: hamburger (`.sidebar-toggle`),
  sidebar close (`.sidebar-close`), entry row action (`.entry-action-btn`),
  reading-pane back (`.reading-pane-back-link`), and a table action link
  (`.action-link`).

The step is generic (takes a selector + size) so future controls are easy to
add. Existing responsive scenarios must continue to pass.

### Verification commands

Run from the repo root (re-source `/tmp/rdrs-env.sh` first per project setup):

- `cd e2e && npx playwright test --grep @mobile` — new + existing mobile
  scenarios green.
- Full e2e responsive sweep green.

## Files Touched

- `static/css/app.css` — add `--touch-min`; new touch-baseline rules and
  spacing/font tweaks inside `@media (max-width: 1024px)`; fix `.entry-item`
  mobile padding.
- `e2e/steps/responsive.steps.js` — new generic size-assertion step(s).
- `e2e/features/responsive.feature` — new `@mobile` tap-target scenario.

No template, Rust, or JS-behavior changes.

## Risks & Mitigations

- **Layout shift on desktop** — all rules live inside `@media (max-width:
  1024px)`; desktop is untouched. Mitigation: keep every new rule inside the
  media block.
- **`inline-flex` breaking full-width/block buttons** — apply `inline-flex`
  only to inline controls; leave `.btn-block` and form-submit buttons as-is
  (they already fill width and just need `min-height`).
- **`min-height` not enlarging text-only links without padding** — pair
  `min-height` with `inline-flex; align-items: center` so height takes effect.
- **iOS focus-zoom** — guaranteed by forcing 16px font on inputs.
