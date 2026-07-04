# Fluid sidebar / list-pane widths for mid-width screens

**Date:** 2026-07-04
**Branch:** `fix/tablet-pane-proportions`
**Status:** Design — awaiting review

## Problem

On the iPad Air 11" (M3) in landscape (1180 × 820 CSS px) the three-pane reading
layout feels cramped: the sidebar and entry list take up too large a share of the
width, squeezing the reading pane. On a 1440 × 900 monitor the same layout feels
well balanced.

The three-pane layout applies at any viewport wider than the `max-width: 1024px`
collapse breakpoint. Both the sidebar and the entry list are **fixed widths** that
do not scale with the viewport:

- `--sidebar-width: 232px`
- `--list-pane-width: 392px` (bumped to `400px` at `min-width: 1600px`)

So the fixed chrome is a constant **624px** regardless of screen width. As a share
of the viewport that is:

| Viewport | Fixed chrome | reading-pane | chrome share |
| --- | --- | --- | --- |
| 1440 × 900 (comfortable) | 624px | 816px (content capped 720) | 43% |
| 1180 × 820 iPad Air landscape (cramped) | 624px | **556px** (below the 720 cap) | **53%** |

The iPad sits just above the 1024px breakpoint, so it uses the desktop three-pane
layout, and the un-scaling 624px chrome eats over half the width.

## Chosen approach — B: fluid widths via `clamp()`

Make the two layout variables scale proportionally between a floor and the current
values, instead of adding a new media query. Selected over the alternatives (A: a
targeted `1025–1400px` media query; C: raising the collapse breakpoint to ~1200px)
because it is the smallest change (two variable values, no new breakpoint), fixes
**every** width between 1024px and 1440px smoothly with no boundary jumps, and
preserves the comfortable side-by-side three-pane layout the user likes at 1440px.

### Change

In `static/css/app.css` `:root`:

```css
--sidebar-width: clamp(200px, 16vw, 232px);
--list-pane-width: clamp(320px, 28vw, 392px);
```

Everything else is unchanged. Both variables already flow through consistently:

- `--sidebar-width` drives `.sidebar { width; min-width }` and
  `.main-content { margin-left }` — they stay in sync because they read the same
  variable.
- `--list-pane-width` drives `.list-pane { width; min-width }`.
- The `@media (min-width: 1600px)` override to `--list-pane-width: 400px` stays as
  is; it takes over above 1600px and preserves the wide-screen bump. (Between 1400
  and 1600 the clamp caps at 392, then steps to 400 at 1600 — an 8px step,
  negligible.)
- The `@media (max-width: 1024px)` block already overrides `.list-pane { width:
  100%; min-width: 0 }` and hides/repositions the sidebar, so the clamp has no
  effect below the collapse breakpoint.

### Resulting behaviour

| Viewport | sidebar | list-pane | reading-pane | chrome share |
| --- | --- | --- | --- | --- |
| 1180 (iPad Air landscape) | 200 (floor) | 330 | **650** | 45% |
| 1440 (unchanged feel) | ~230 → 232 | 392 (cap) | ~816 | 43% |
| ≥1600 | 232 | 400 (existing override) | — | — |

- Sidebar reaches its 232px cap at ~1450px viewport; below ~1250px it rests at the
  200px floor.
- List-pane reaches its 392px cap at 1400px viewport; below ~1143px it rests at the
  320px floor.
- At 1180px the reading pane recovers from 556px to ~650px.

## Out of scope

- No change to the 1024px collapse breakpoint or the mobile/drawer layout.
- No change to `--content-max-width` (720px) or reading-pane internals.
- Floor values (`200px` / `320px`) are the proposed defaults; adjustable at review
  (e.g. hold the list floor at 340px).

## Verification

1. `cargo build` (assets are `include_str!`-embedded, so a rebuild is required
   before E2E / screenshots see CSS edits).
2. Visual check at 1180 × 820, 1280, 1366, and 1440 × 900 — confirm smooth scaling,
   no layout break, list floor never clips entry rows.
3. Regenerate README screenshots: `cd e2e && npm run screenshots` (the four images
   under `screenshots/` are part of any UI change).
4. `cargo fmt --all -- --check` is irrelevant (CSS only); no Rust or test changes.

## Risks

- Very low. Pure CSS variable change. Main risk is a floor value that clips content
  at the narrow end — mitigated by the 320px list floor (wider than the 1024px
  collapse) and by the visual check above.
