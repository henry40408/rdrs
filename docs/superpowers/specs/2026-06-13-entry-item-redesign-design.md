# Entry Item Redesign (Option C — Editorial) — Design

**Date:** 2026-06-13
**Branch:** `feat/mobile-touch-targets` (continues the mobile-layout effort; builds on the `--touch-min` token and mobile touch baseline already on this branch)
**Status:** Approved design (v5 mockup), pending implementation plan

## Problem

After the 44px touch-target pass, the entry-item row reads awkwardly on mobile:
the meta links and action controls were inflated piecemeal on top of a layout
that was designed for desktop density, so spacing and alignment look off. The
row needs a coherent redesign that works for **both** desktop and mobile rather
than mobile patches over a desktop layout.

This was validated visually through a browser mockup session (Option C, iterated
v1→v5). The chosen direction is the "editorial" treatment.

## Goals

- One coherent entry-row layout that scales from desktop to a 375px phone.
- Touch-friendly on mobile without looking bulky; tidy and scannable on desktop.
- Darker, more legible secondary text (current muted grey is too pale).
- Keep the existing interaction model: tap the row/title to open; per-row
  star / read-toggle / open-original actions; unread/read/selected states.

## Non-Goals

- No change to the reading pane, sidebar, tables, or other pages (those keep the
  touch-baseline work already on this branch).
- No change to backend endpoints or the swap/data-flow model.
- No new JS behavior beyond what the existing action forms already do.

## Chosen Design (v5)

### Layout — CSS grid, identical structure desktop & mobile

```
grid-template-columns: auto 1fr;
grid-template-areas:
  "fav  head"
  "fav  meta"
  "foot foot";
column-gap: var(--space-3);   /* 12px */
```

- **`fav`** — leading feed favicon (24px, `--radius-control`). Spans only the
  `head`+`meta` rows so there is no tall empty column beside the action strip.
- **`head`** — title + relative time. Title `var(--font-ui)` (DM Sans, the UI
  sans — a serif here read as out of place). Size `var(--font-sm)` (14px) on
  desktop so it matches the visible 14px sidebar/chrome scale, bumped to
  `var(--font-base)` (16px) on mobile where no sidebar is shown for comparison.
  Weight 600 when unread / 400 when read, `color: var(--color-text)`. Time is
  `margin-left:auto`, muted, `white-space:nowrap`.
- **`meta`** — feed link `·` category link. `var(--font-ui)`, ~12.5px desktop /
  13px mobile, `color: var(--color-text-secondary)` (darker than today's muted),
  normal weight, separator muted. Left edge is flush with the title.
- **`foot`** — action strip spanning the full width from the row's left content
  edge (both grid columns).

### Favicon (new: always present, with fallback)

Today the feed icon renders only when `feed_has_icon`. The redesign always shows
a 24px leading favicon:

- If the feed has an icon: `<img src="/api/feeds/{feed_id}/icon" width="24" height="24">`.
- Otherwise: a deterministic colored chip showing the feed title's first letter.
  Color is chosen from a fixed 6-color palette indexed by `feed_id % 6`
  (computed in the template/handler), so the same feed is always the same color.
  No new network/storage — pure render.

### Actions (`foot`)

Same three controls as today, re-presented:

- **Star** — `★` (starred) / `☆` (not). Posts to `…/star` | `…/unstar`.
- **Read toggle** — `✓` (mark read, when unread) / `↺` (mark unread, when read).
  Posts to `…/read` | `…/unread`.
- **Original** — `↗` link to `r.link` (external), only when a link exists.

Presentation differs by breakpoint:

- **Desktop:** quiet inline text links with icon + label
  (`★ starred`, `✓ read`, `↗ original`), `opacity: ~0.72`, raised to full
  opacity on row hover (a "hover footer"). Left-aligned, `gap: var(--space-5)`.
- **Mobile (≤1024px):** three equal full-width icon **buttons**
  (`display:flex; gap:var(--space-2)`, each `flex:1`), `min-height:var(--touch-min)`
  (44px), icon ~22px, small padding (~6px), thin `--color-border-light` border,
  `--radius-md`. Icon-only.

Accessibility: each mobile icon button (and the icon-only desktop link where the
label is hidden) MUST carry an `aria-label` / `title` ("Star", "Mark read",
"Open original"). Desktop keeps the visible text label.

### Touch sizing

- Action buttons (primary controls): **44px** min-height on mobile.
- Feed / category: kept as plain **inline** links so "feed · category" wraps
  naturally as one text run (not two side-by-side flex blocks). Vertical padding
  gives each link a ~28px hit box (≥ the WCAG 2.5.8 AA 24px floor) without
  breaking the wrap; **no horizontal padding** so the first link is flush-left
  with the title. Inline text links are formally exempt from 2.5.5's 44px, so
  this is a deliberate, compliant choice. (min-height/inline-flex is avoided
  here because it would make each link an atomic box and break wrapping.)

### States (unchanged semantics, restyled)

- **Unread:** bold title. (Optional later: leading dot — NOT in this scope.)
- **Read:** `opacity: 0.62` (was 0.55 — slightly less faded for legibility);
  full opacity on hover/selected.
- **Selected:** `--color-accent-subtle` background + inset gold left bar
  (`box-shadow: inset var(--border-accent-width) 0 0 var(--color-accent)`) — kept
  from current.
- Row hover: `--color-bg-secondary` background — kept.

### Color/legibility changes

- Meta text: `--color-text-muted` → `--color-text-secondary` (darker).
- Read opacity: 0.55 → 0.62.

## Affected Units

- **`templates/_entry_row.html`** — restructured to the grid areas; favicon
  always rendered (with letter-chip fallback); actions grouped in `.foot` with
  `aria-label`s. This is the single source of the row DOM.
- **`static/css/app.css`** — replace the `.entry-item*` rule block (desktop) and
  the entry-row parts of the `@media (max-width:1024px)` baseline with the v5
  grid layout + states. Remove now-superseded entry-row touch tweaks added
  earlier on this branch (the `.entry-item-actions` flex/gap/align rules and the
  `.entry-action-btn` padding rule) since the redesign defines the action strip
  wholesale.
- Possibly a tiny helper in the page handler/template for the favicon fallback
  color/letter if it can't be expressed in Askama alone.

## Testing

Extend the existing Playwright-BDD e2e (which already asserts 44px targets):

- Update/keep the entry-row selectors used by existing scenarios. The action
  controls move from text links to icon buttons — the existing
  `.entry-action-btn` test hooks (`data-testid="entry-read-action"`,
  `entry-star-action`, `entry-original-link`) MUST be preserved so triage and
  responsive scenarios keep working.
- Add a `@mobile` assertion that the action buttons span full width (each ≈ a
  third of the row) and are ≥44px tall, and that feed/category links are ≥24px.
- Verify desktop still shows text labels and the hover footer.
- Full responsive + triage + keyboard sweeps stay green.

## Risks & Mitigations

- **Breaking triage/responsive e2e** by changing the action DOM. Mitigation:
  keep all existing `data-testid`s and POST actions identical; only restyle and
  re-wrap.
- **Favicon fallback** adds template logic. Mitigation: keep it a pure
  deterministic render (palette indexed by `feed_id`), no DB/network.
- **Overlap with the touch-baseline already on this branch.** Mitigation: the
  redesign explicitly supersedes the entry-row-specific baseline rules; the
  broader baseline (buttons, inputs, sidebar, tabs, banner, hamburger) stays.
- **Icon-only mobile a11y.** Mitigation: required `aria-label`/`title` on every
  icon control.
