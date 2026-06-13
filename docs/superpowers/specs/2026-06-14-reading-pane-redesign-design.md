# Reading Pane Redesign — Design

**Date:** 2026-06-14
**Branch:** `feat/reading-pane-redesign`
**Status:** Approved design, pending implementation plan

## Problem

The reading pane has the same mobile ergonomics problem the entry list row had
before its redesign (see
`docs/superpowers/specs/2026-06-13-entry-item-redesign-design.md`): the chrome
elements are small and cramped, hard to tap and easy to mis-tap on a phone. The
specific offenders:

- **Action buttons** (`Star`, `Mark Unread`, `Fetch Full Content` / `Show
  Original`, `Summarize`, `Save`) are a wrapping cluster of five equal-weight
  `.btn-secondary .btn-sm` buttons. On a narrow viewport they wrap into uneven
  rows that are noisy to scan and easy to fat-finger.
- **Top toolbar** (`.reading-pane-back`) packs the Back affordance and the
  prev/next nav into one small sticky bar.
- **Header** title is serif `--font-3xl` (2rem) on every viewport; the meta
  line is plain muted text with no feed identity.

The 44px touch baseline from the mobile-touch-targets work already raised raw
button heights, but the *composition* on mobile is still awkward — the same
"redesign from scratch" treatment the entry row received is wanted here.

## Goals

- Redesign the reading-pane **chrome** (toolbar, header/meta, action controls,
  summary-box controls) for both desktop and mobile, sharing the entry-row
  visual language (favicon chip, icon + label controls, generous tap targets).
- On mobile, move the action controls into a **fixed bottom action bar** that is
  thumb-reachable and stays available while the article scrolls.
- On desktop, present the actions as a single same-level row under the meta line
  (no grouping), icon + label, that never line-wraps its labels.
- Keep every action's accessible name stable so existing e2e and a11y hold.

## Non-Goals

- **Article body typography is unchanged** — `.reading-pane-article` and all its
  descendant rules (`p`, `h1`–`h6`, `pre`, `blockquote`, `img`, tables, …) stay
  exactly as they are. Long-form reading already works.
- No change to the pane's data flow, swap targets, form actions, or the
  prev/next neighbor JS. This is a presentation-layer redesign (template + CSS +
  a small Rust view addition for the favicon).
- No new JS behavior. The bottom bar is pure CSS (`position: fixed`).
- No new build tooling (vanilla only, per project constraints).

## Approach

Adopt **Approach B** from brainstorming: a mobile bottom action bar, desktop
inline actions. The chrome reuses the entry-row redesign vocabulary.

### 1. Header + meta (both viewports)

- **Title** stays serif (`--font-display`) but shrinks: `--font-2xl` (1.5rem)
  on desktop, `--font-xl` (1.25rem) on mobile. This matches the entry-row
  decision to shrink the desktop title so it reads in proportion next to the
  sidebar.
- **Meta** gains a **favicon chip** matching the entry row: the feed favicon
  (`/api/feeds/{feed_id}/icon`) when present, else a colored initial chip
  (`.entry-favicon-chip .fav-c{0..5}`). The chip sits before the feed title;
  `feed · author · time` follow with the existing mid-dot separator logic.

### 2. Action controls — shared `.rp-action` class

Replace the per-button `.btn-secondary .btn-sm` with a dedicated `.rp-action`
control used by every pane action. Each control is `icon span (aria-hidden) +
label span`, mirroring `_entry_row.html`:

```html
<button type="submit" class="rp-action" aria-label="Mark Unread">
  <span class="action-icon" aria-hidden="true">↺</span>
  <span class="action-label">Unread</span>
</button>
```

**Accessible name is carried by `aria-label`** (the icon is `aria-hidden`, so
the visible short label does not pollute the name). This keeps the names the
e2e suite relies on stable while letting the visible label be short:

| Action            | `aria-label` (stable) | visible label   | icon |
|-------------------|-----------------------|-----------------|------|
| Star / Unstar     | `Star` / `Unstar`     | `Star`/`Starred`| ☆ / ★ |
| Mark Unread       | `Mark Unread`         | `Unread`        | ↺ |
| Fetch Full Content| `Fetch Full Content`  | `Full content`  | ⤓ |
| Show Original     | `Show Original`       | `Original`      | ↩ |
| Summarize         | `Summarize`           | `Summarize`     | ✦ |
| Save              | `Save`                | `Save`          | ⬇ |

(Glyphs are unicode, consistent with the entry-row `.action-icon` set; they may
be swapped during implementation but the table is the default.)

**Desktop layout** (`.reading-pane-actions`, in normal flow under the meta):
single row, `display: flex; flex-wrap: wrap; gap`. Each `.rp-action` is
`inline-flex` (icon left of label), `white-space: nowrap` so a label never
breaks mid-word, `flex: none` so buttons keep natural width. All five are the
**same visual level — no grouping / no separators**. `flex-wrap: wrap` is kept
only as a safety net for extreme narrow desktop widths (whole buttons wrap, not
text).

**Mobile layout** (≤1024px): `.reading-pane-actions` becomes a **fixed bottom
bar**:

- `position: fixed; left: 0; right: 0; bottom: 0; z-index: 101` (above the pane
  overlay's content; the pane is `position: fixed` with no transform, so a fixed
  child is viewport-positioned and not clipped).
- `display: flex; justify-content: space-around;` with each form / control as a
  `flex: 1` child so 2–5 actions always spread evenly across the full width.
- Each `.rp-action` is **stacked** (icon over small label), `min-height:
  var(--touch-min)` (≥44px; bar renders ~48–56px tall with padding), centered.
- The forms wrap the buttons, so the **form** is the flex child (`form { flex: 1
  }`) and its button stretches — same pattern the entry row uses.
- The pane content gets `padding-bottom` equal to the bar height so the last
  paragraph is not hidden behind the fixed bar.

**Conditional/variable count:** the action set is already conditionally rendered
(`Fetch/Show`, `Summarize`, `Save` only inside `if link`, gated further by
`has_kagi` / `summary_in_flight` / `has_save`). With `flex: 1 + space-around`,
2, 3, 4, or 5 buttons all spread evenly with no left-bias or gaps. `Summarize`
keeps its existing `disabled` state while `summary_in_flight`.

### 3. Toolbar (`.reading-pane-back`)

- **Desktop:** unchanged placement — prev/next nav right-aligned, no Back.
- **Mobile:** Back affordance left (accent text, ≥44px hit box, as today),
  prev/next on the right rendered **icon-only** (`‹` / `›`) to save width. The
  word labels (`Previous` / `Next`) move into a `.nav-label` span hidden on
  mobile; the buttons gain `aria-label="Previous"` / `"Next"` so the accessible
  name survives. (e2e targets these by `data-testid`, not by name.)

### 4. Summary box controls

The summary box (`.summary-box`) keeps its structure (header title + link,
blockquote body). Its `Copy` / `Dismiss` controls adopt the same `.rp-action`
style (icon + label, touch-sized) so they match the new action vocabulary.
Glyphs: `⧉ Copy`, `✕ Dismiss`. Their `data-summary-copy` / `data-summary-dismiss`
hooks and behavior are unchanged.

## Rust changes (favicon data)

`ReadingPaneView` (`src/handlers/pages/mod.rs`) currently lacks the feed
identity needed for the chip. Add:

- Fields: `feed_id: i64`, `feed_has_icon: bool`.
- Methods `feed_initial(&self) -> String` and `feed_color_index(&self) -> u8`
  with the **same logic** as `EntryRowView`. To stay DRY, extract that logic
  into two free functions (`feed_initial(title: &str)` and
  `feed_color_index(feed_id: i64)`) in `pages/mod.rs` and have both
  `EntryRowView` and `ReadingPaneView` delegate to them.
- Populate the new fields in `build_reading_pane_view`
  (`src/handlers/entries.rs`) from `ewf.entry.feed_id` and `ewf.feed_has_icon`.

## Files Touched

- `templates/_reading_pane.html` — favicon chip in meta; `.rp-action` controls
  with icon/label spans + aria-labels; nav icon-only + `.nav-label`; summary-box
  Copy/Dismiss restyle.
- `static/css/app.css` — title size tweak; meta favicon chip (reuse entry-row
  rules); `.rp-action` base + desktop row; `.nav-label`; mobile `@media
  (max-width:1024px)` block: fixed bottom action bar, stacked controls, content
  `padding-bottom`, icon-only nav.
- `src/handlers/pages/mod.rs` — `ReadingPaneView` fields + helpers; extract
  shared `feed_initial` / `feed_color_index` free functions; unit tests.
- `src/handlers/entries.rs` — populate new fields in `build_reading_pane_view`.
- `tests/handlers_test.rs` — adjust/extend reading-pane render assertions if any
  reference the old button classes/markup; add coverage for the favicon chip in
  the pane.
- `e2e/features/reading.feature` + a step or two — add a `@mobile` scenario
  asserting the bottom action bar (controls ≥44px, full-width spread); existing
  `Fetch Full Content` / `Summarize` clicks must keep passing (accessible names
  preserved).

## Testing

- **Rust (`cargo nextest run`, with `RDRS_FAST_HASH=1`):** unit tests for the
  extracted `feed_initial` / `feed_color_index` (uppercase, empty→`?`, unicode,
  modulo bounds) and a handler test asserting the pane renders the favicon chip
  (or `/api/feeds/{id}/icon` img) and the `.rp-action` controls with their
  aria-labels. Existing pane render tests stay green.
- **e2e (`cd e2e && npx playwright test`):** re-source `/tmp/rdrs-env.sh`;
  **`cargo build` first** (CSS is `include_str!`'d into the binary, the e2e
  harness does not rebuild on CSS change). New `@mobile` scenario:
  - open an entry → reading pane visible on mobile,
  - the action controls live in a bottom bar, each `.rp-action` ≥44px tall,
  - the bar spans the viewport width.
  Existing `reading.feature` (Fetch Full Content desktop + mobile nav) and
  `triage.feature` (Summarize) scenarios must continue to pass.

## Risks & Mitigations

- **Fixed bar clipped/scrolling with content** — the pane overlay is
  `position: fixed` with no `transform`/`filter`, so a `position: fixed` child is
  viewport-anchored and not clipped. Verified pattern; the mobile e2e scenario
  guards it.
- **Content hidden behind the bar** — add `padding-bottom` on the pane content
  equal to the bar height on mobile.
- **Accessible-name drift breaking e2e** — every control keeps an explicit
  `aria-label` equal to its canonical full name; visible short labels do not
  affect the name. The Rust/e2e tests assert the names.
- **Desktop label wrap (the `Full content` issue seen in the mockup)** —
  `white-space: nowrap` on `.rp-action` plus `flex: none`; the row only wraps
  whole buttons, never mid-label.
- **Desktop regression from mobile rules** — every mobile rule stays inside the
  existing `@media (max-width: 1024px)` block.
