# Fluid Sidebar / List-Pane Widths Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the sidebar and entry-list widths scale fluidly on mid-width screens so the reading pane is no longer cramped on iPad Air 11" landscape (1180px), while leaving 1440px unchanged.

**Architecture:** Change two CSS custom properties in `static/css/app.css` `:root` from fixed pixels to `clamp()` so they scale between a floor and their current cap. Both variables already flow through every consumer (`.sidebar`, `.main-content`, `.list-pane`) so no rule bodies change. Verify visually across viewports and regenerate the README screenshots.

**Tech Stack:** Vanilla CSS (no build tooling); assets are `include_str!`-embedded into the Rust binary, so a `cargo build` is required before E2E/screenshots see the edit. Screenshots via Playwright (`e2e/`).

## Global Constraints

- Assets are compiled into the binary via `include_str!`/`include_bytes!`; **`cargo build` before any E2E/screenshot run** or it runs against stale CSS.
- UI changes MUST regenerate the four `screenshots/` images (referenced by `README.md`) in the same change.
- Commits MUST be GPG-signed; stage files explicitly by name (no `git add -A`/`.`).
- No new media query, no change to the `max-width: 1024px` collapse breakpoint, the `min-width: 1600px` `--list-pane-width: 400px` override, or `--content-max-width: 720px`.
- Floor values are fixed: sidebar `200px`, list-pane `320px` (approved, do not change).

---

### Task 1: Make sidebar and list-pane widths fluid

**Files:**
- Modify: `static/css/app.css:89-90` (the `--sidebar-width` and `--list-pane-width` declarations in `:root`)

**Interfaces:**
- Consumes: nothing (leaf change).
- Produces: fluid `--sidebar-width` / `--list-pane-width`; consumers `.sidebar { width; min-width }`, `.main-content { margin-left }`, `.list-pane { width; min-width }` inherit the new behaviour unchanged.

- [ ] **Step 1: Confirm the current declarations**

Read `static/css/app.css` around lines 88–91. Expected:

```css
    /* Layout */
    --sidebar-width: 232px;
    --list-pane-width: 392px;
```

- [ ] **Step 2: Replace the two fixed values with clamp()**

```css
    /* Layout — fluid on mid-width screens (1024–1440) so the reading pane
       isn't cramped on ~1180px tablets; both cap at their prior fixed value. */
    --sidebar-width: clamp(200px, 16vw, 232px);
    --list-pane-width: clamp(320px, 28vw, 392px);
```

Leave every other declaration in the block (`--content-max-width`, `--page-max-width`, etc.) untouched. Leave the `@media (min-width: 1600px) { --list-pane-width: 400px; }` and `@media (max-width: 1024px)` blocks untouched.

- [ ] **Step 3: Rebuild so the embedded CSS is current**

Run: `cargo build`
Expected: builds successfully (CSS is embedded at compile time).

- [ ] **Step 4: Visual verification across viewports**

Use the `verify` skill / a Playwright-driven browser to load a logged-in entries page and check these widths (DevTools or computed style):

| Viewport | Expected sidebar | Expected list-pane | Expected reading-pane | Check |
| --- | --- | --- | --- | --- |
| 1180 × 820 | 200px (floor) | ~330px | ~650px | reading pane visibly roomier; no clipped entry rows |
| 1280 × 800 | ~205px | ~358px | wider still | smooth, no jump |
| 1440 × 900 | 232px (cap) | 392px (cap) | ~816px | **identical to before this change** |
| 1024 × 768 | drawer (hidden) | 100% | overlay | collapse layout unaffected |

Expected: smooth scaling, no horizontal overflow, list floor (320px) never clips the entry row time/star columns.

- [ ] **Step 5: Regenerate README screenshots**

Run: `cd e2e && npm run screenshots`
Expected: the four images under `screenshots/` are rewritten (light/dark unread list + keyboard-help). Confirm they render without layout breakage.

- [ ] **Step 6: Commit**

```bash
git add static/css/app.css screenshots/
git commit -S -m "$(cat <<'EOF'
fix(ui): fluid sidebar/list-pane widths for mid-width screens

The sidebar (232px) and entry list (392px) were fixed widths, so their
combined 624px chrome ate 53% of an iPad Air 11" landscape (1180px)
viewport, cramping the reading pane to 556px. Make both variables fluid
via clamp() so they scale between a floor (200/320px) and their prior
value; caps preserve the 1440px layout and the >=1600px override.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: one commit containing `static/css/app.css` and the updated `screenshots/`.

---

## Notes / Out of scope

- **No new E2E test.** A viewport-width assertion would be brittle (pixel maths against clamp/vw) and the change carries no logic branch to guard — YAGNI. Verification is the visual check (Step 4) plus the regenerated screenshots. If desired later, an optional follow-up could add a Playwright check asserting `.reading-pane` is wider than `.list-pane` at 1180px.
- Floor values (`200px` / `320px`) are locked per the approved spec.

## Self-Review

- **Spec coverage:** The single spec change (two clamp values) → Task 1 Step 2. Behaviour table, 1600px override preservation, 1024px collapse non-interference, verification (rebuild → visual → screenshots) → Steps 3–5. Covered.
- **Placeholder scan:** No TBD/TODO; exact code and commands in every step.
- **Type consistency:** N/A (CSS); variable names `--sidebar-width` / `--list-pane-width` match the spec and existing consumers verbatim.
