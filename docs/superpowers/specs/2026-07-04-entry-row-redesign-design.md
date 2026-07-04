# Entry-Row Redesign — Design

**Date:** 2026-07-04
**Status:** Approved (design reviewed iteratively in the Visual Companion)

## Motivation

The 0.55.0 "Wire Room" redesign removed three per-row controls from the entry
list and left one padding bug:

1. **Mark read/unread** row control — gone; only `m` (keyboard) and the reading
   pane could toggle read state. The leading `.unread-dot` became decorative.
2. **Open original** row link — gone; only `v` (keyboard) opens the source URL.
3. **Title hover** affordance — the title no longer signalled interactivity.
4. **Star padding bug** — `.entry-star` used `margin: -3px -4px 0 0`, an
   asymmetric top/bottom offset that made the star look vertically off-centre.

This redesign restores the two removed *actions* as always-visible row controls,
resolves the star padding bug structurally, removes the low-value category link
from the meta line, and fixes assorted layout issues surfaced during review
(single-line whitespace, dot-to-title alignment, meta/action alignment, summary
badge placement, aggressive touch title wrapping).

Scope is **the entry row only** (`templates/_entry_row.html` +
`static/css/app.css` entry-list block, plus the required e2e/screenshot
refresh). No handler, model, route, or JS behaviour changes — the
`/entries/{id}/{read,unread,star,unstar}` endpoints and the
`_entry_actions_multi.html` swap response already exist and are reused as-is.

## Final Layout

Row is a **CSS grid, 3 columns × 2 rows**:

```
 col1 (marker)  col2 (content, 1fr)              col3 (timerow, auto)
┌────────────┬─────────────────────────────────┬──────────────────────┐
│  ● dot     │  Entry title, up to 3 lines,     │        [✦ 12m]       │  row1
│  (toggle)  │  -webkit-line-clamp:3 …           │                      │
│            ├─────────────────────────────────┤                      │
│            │  ▣ favicon  Feed name (ellipsis) │                      │  row2
└────────────┴─────────────────────────────────┴──────────────────────┘
                                                    [★ ↗]  ← absolute overlay,
                                                            centred on the meta row
```

- **col1 / marker (row1):** the unread **dot = read/unread toggle** only. No
  star here. Because the marker column holds a single control (not a vertical
  stack), it is shorter than a one-line content block, so single-line rows have
  **no trailing whitespace**.
- **col2 / content:** title (row1) over meta (row2).
- **col3 / timerow (row1):** summary badge + relative time, right-aligned,
  top-aligned to the title's first line.
- **`[★ ↗]` actions:** `position: absolute`, bottom-right, vertically centred on
  the **meta** row. They are *not* grid items, so they do not reserve column
  width — this lets the title column stay wide (see "Touch title width" below).

### Marker — dot = read/unread toggle

- A `<form method="post">` posting to `/entries/{id}/read` (when unread) or
  `/entries/{id}/unread` (when read), with `data-swap="#entry-row-{id}"`,
  mirroring the existing `.entry-star-form`. The server already returns
  `_entry_actions_multi.html` (row + sidebar-unread swap).
- The submit button is `.unread-toggle`, a circular hit target containing a 9px
  `::before` dot.
  - **Unread:** filled — `background: var(--color-accent)`.
  - **Read:** hollow ring — `background: transparent; box-shadow: inset 0 0 0
    1.5px var(--color-text-muted)`.
- **Hover** (mirrors the star): `background: var(--color-accent-subtle)`; on read
  rows the ring turns accent (`box-shadow: inset 0 0 0 1.5px var(--color-accent)`).
- **Vertical alignment:** the dot's centre sits on the title's first-line optical
  centre. Achieved with `margin-top: -1px` on the 24px pointer box (`padding-top`
  on the marker is `0`). Rationale: first-line box = `16px × 1.35 = 21.6px`, its
  centre is `10.8px` from the content top; a 9px dot centred in a 24px box needs
  the box top at `≈ -1.2px`. On touch the box is 36px, so the nudge scales to
  `margin-top: -7px` to keep the same centre.

The decorative-only `.unread-dot` element and its `.entry-time`-embedded markup
are removed.

### Content — title + meta

- **Title** (`.entry-item-title`): unchanged type ramp — `font-ui`, 16px,
  weight 600 (unread) / 500 (read), `line-height: 1.35`, `-webkit-line-clamp: 3`,
  `word-break: break-word`. Restore a **hover affordance**: `color:
  var(--color-accent)` on `.entry-item:hover .entry-item-title` (title is the
  primary click target).
- **Meta** (`.entry-item-meta`): favicon + **feed only**. The category link
  (`<a href="/categories/{id}/entries">`) and the `.meta-sep` separator are
  **removed**. Feed name single-line with ellipsis (`.entry-feed { display:block;
  overflow:hidden; white-space:nowrap; text-overflow:ellipsis }` inside a
  `flex:1; min-width:0` `.entry-meta-text`).
  - Meta gets `padding-right: var(--meta-pad)` so the feed ellipsises *before*
    the absolute `[★ ↗]` overlay (pointer `≈ 48px`, touch `≈ 80px`).
  - Meta is `align-self: center` in row2 so its centre matches the actions'
    centre.

### Timerow — summary badge + time

- col3 / row1, right-aligned, `align-self: start`, `padding-top: 2px` to sit on
  the title's first line. Inline-flex, `gap: 5px`, `white-space: nowrap`:
  `[summary?] [time]`.
- **Summary badge** (`.entry-status`) moves here from the meta-line tail (it was
  jarring abutting the actions). Semantics unchanged, only relocated + recoloured
  per state:
  - `completed` → **filled** (`icons::summary(true)`, `is-filled`), `color:
    var(--color-accent)`. *(Review caught the earlier hollow preview — completed
    must be the filled sparkle.)*
  - `pending` / `processing` → hollow, `color: var(--color-text-muted)`.
  - `failed` → filled, `color: var(--color-error)`.
  - The `.entry-status` container and `:empty { display:none }` rule are kept so
    the SSE `renderSummaryBadge` path (app.js) keeps working; only its DOM
    position and per-state colour move.
- **Time** (`.entry-time`): mono, 13px, muted. No longer stacks over a dot.

### Actions — `[★ ↗]` overlay

- Container `.rail-actions`: `position: absolute; right: 14px; bottom:
  var(--act-bottom)`, `display:flex; gap:2px`. `--act-bottom` centres the cluster
  on the meta row: meta centre is `padding-bottom(12) + meta-line(21)/2 = 22.5px`
  from the item bottom, so `--act-bottom = 22.5 − actionHeight/2` → **10.5px**
  (pointer, 24px) / **4.5px** (touch, 36px).
- **Star** (`.entry-star`, inside `.entry-star-form`): the existing star form,
  moved out of col3 into this overlay. Always visible (**no hover-reveal**),
  `color: var(--color-text-muted)`; hover → accent + `--color-accent-subtle`;
  `.starred` → accent, filled icon. **The `margin: -3px -4px 0 0` padding bug is
  deleted** — the star now centres in a fixed square via `place-items: center`,
  so no asymmetric nudge is needed.
- **Open-original** (`.entry-open-ext`): an `<a>` to the entry's external URL
  (`data-entry-link` already carries it; render `href` directly), 13px `↗`
  (`icons::external`). `color: var(--color-text-muted)`; hover → accent. Shown on
  all devices; on touch it is a 36px target (the row itself remains the primary
  open-in-reader target via `installRowClickToOpen`).

### Removed / changed elements summary

| Element | Change |
| --- | --- |
| `.unread-dot` (decorative) | **removed**; replaced by `.unread-toggle` (functional) |
| category `<a>` + `.meta-sep` | **removed** from meta line |
| `.entry-star` `margin:-3px -4px 0 0` | **removed** (padding bug fixed structurally) |
| `.entry-status` (summary) | **relocated** meta-tail → timerow; per-state colour |
| open-original `↗` | **restored** as `.entry-open-ext` overlay action |
| mark read/unread | **restored** as `.unread-toggle` form |
| title hover | **restored** (`color: accent`) |

## Sizing & Touch

- **Pointer:** dot / star / open-ext = **24px** hit boxes (`--sz: 24px`), which
  is exactly WCAG 2.5.8 AA (24px) — a safety floor even on hybrid devices.
- **Touch:** **36px** (`--sz: 36px`), a deliberate compromise below the previous
  44px (WCAG 2.5.5 AAA) but comfortably above AA. Chosen because 44px forced
  oversized rows; 36px reads as comfortable while keeping density.
- **Breakpoint:** touch sizing applies under **`@media (any-pointer: coarse)`**
  (not `hover: none`). Rationale: `any-pointer: coarse` is true whenever the
  device *has* a touch input — including hybrids (touch laptops, iPad + trackpad)
  — so touch targets are comfortable wherever touch is possible; pure-mouse
  desktops stay compact. Worst case (a hybrid treated as pointer) still yields
  24px = AA.
- The existing `--touch-min: 2.75rem` (44px) token is **left unchanged** (it is
  used by many other controls); the entry-row controls use their own 36px sizing
  via `--sz`. Do not repurpose `--touch-min`.
- The feed link keeps its taller mobile hit box (`display:inline-block; padding:
  .35rem 0`) from the existing mobile block.

### Touch title width

With the actions as grid items, two 36px touch buttons force col3 to ~72px,
squeezing the title column and causing noticeably more aggressive wrapping than
pointer, with a wide empty gutter beside the title's lower lines. Making
`.rail-actions` an **absolute overlay** removes it from column flow: col3 then
carries only the timerow (~36px), so the title column width is near-identical on
pointer and touch and the right-side blank shrinks from ~72px to ~36px.
(`-webkit-line-clamp` uses `display:-webkit-box`, which does not honour floats,
so a float-to-wrap-around-time approach is not viable — the overlay is the
correct mechanism.)

## Alignment rules (exact)

- **dot ↔ title first line:** `margin-top: -1px` (pointer) / `-7px` (touch).
- **time ↔ title first line:** `align-self:start; padding-top: 2px`.
- **`[★ ↗]` ↔ meta line centre:** `bottom: 10.5px` (pointer) / `4.5px` (touch).
- **meta ↔ actions:** meta `align-self:center` in row2; actions centred via the
  `bottom` value above — both land on the meta row's centre line.

## Hover behaviour (no hover-reveal)

All controls are **always visible**. Hover only adds emphasis:

| Target | Hover |
| --- | --- |
| row | `background: var(--color-bg-secondary)` |
| `.unread-toggle` | `background: var(--color-accent-subtle)`; read-row ring → accent |
| `.entry-star` | `color: accent` + `background: accent-subtle` |
| `.entry-open-ext` | `color: accent` |
| `.entry-item-title` | `color: accent` |
| `.entry-feed` | `color: accent` |

## Preserved contracts (e2e / JS / SSE)

These hooks MUST survive the rewrite:

- Element IDs / attrs: `#entry-row-{id}`, `data-entry-row`, `data-entry-id`,
  `data-entry-link`, `data-testid="entry-item"`, title link
  `data-swap="#reading-pane"` + `data-testid="entry-title-link"`, star button
  `data-testid="entry-star-action"`.
- Classes: `.entry-item`, `.entry-read`, `.selected`, `.entry-item-title`,
  `.entry-item-meta`, `.entry-favicon`, `.entry-status`, `.entry-star-form`,
  `.entry-star`.
- `.entry-status:empty { display:none }` + the SSE `renderSummaryBadge` target.
- The feed link `<a href="/feeds/{id}/entries">` stays (only the category link is
  removed).
- New: `.unread-toggle` (form submit) and `.entry-open-ext` (`href` from
  `data-entry-link`). Give the dot form button a `data-testid` (e.g.
  `entry-read-toggle`) and the open link a `data-testid` (e.g.
  `entry-open-original`) for e2e.

## Files

- `templates/_entry_row.html` — restructure to the 2-row grid; add dot toggle
  form + open-ext link; move summary badge to timerow; drop category + `.meta-sep`
  + decorative `.unread-dot`.
- `static/css/app.css` — rewrite the `.entry-item` block (grid rows, marker,
  timerow, overlay actions, alignment nudges); update the mobile/touch blocks to
  `@media (any-pointer: coarse)` with `--sz: 36px` and remove the now-stale
  `.entry-star` mobile overrides that assumed the old position; delete the
  `.unread-dot` rules and the star `margin` hack.
- Rebuild required before e2e/screenshots (assets are `include_str!`-embedded).

## Testing

- **Rust:** `cargo build`, `cargo fmt --all -- --check`, `cargo clippy
  --all-targets -- -D warnings`. No new Rust logic, but `row_view_from` /
  `RowView` must still expose `link`, `feed_id`, `feed_title`, `is_read`,
  `is_starred`, `summary_status_str`, `feed_has_icon`, `feed_color_index`,
  `feed_initial` (all already present). If `category_id` / `category_name` become
  unused in the template, leave the model fields (other templates use them) —
  only the row template stops rendering them.
- **e2e (`e2e/`):** rebuild the binary first (`cargo build`), then run the entry
  list / star / read-unread / sse-live-updates features. Update selectors only if
  a spec used the removed category link or the old star position; the preserved
  `data-testid`s should keep most specs green. Mark-read/open-original get new
  `data-testid`s — add coverage if a feature file targets them.
- **Screenshots:** `cargo build` then `cd e2e && npm run screenshots`; commit the
  four regenerated images under `screenshots/` since the row visibly changes.
  (Known caveat: the generator can emit sub-100-byte non-deterministic favicon
  diffs; re-run to confirm real vs noise before committing.)

## Out of scope / non-goals

- No changes to keyboard shortcuts (`m` / `v` / `f` keep working via existing
  handlers).
- No changes to the reading pane, sidebar, or GReader API.
- No new dependencies, bundlers, or JS frameworks (SSR + progressive-enhancement
  ceiling unchanged).
- `--touch-min` semantics unchanged; category model fields unchanged.
