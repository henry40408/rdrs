# SVG icon migration, entry-row relayout & touch-target audit — Design

Date: 2026-06-14
Status: Approved (visuals confirmed via brainstorming companion)

## Context

Three user-facing goals, shipped as one PR:

1. Replace the **remaining emoji-as-icon** usage with inline SVGs from the existing
   stroke-based icon set (`templates/_icons.html`).
2. Re-lay out the **entry list row** so it echoes the reading-pane editorial
   chrome (inline favicon leading the meta line, right-aligned status cluster).
3. Audit **all interactive components** for adequate touch targets, completing
   the 44px work started for buttons in `54b0008`.

Builds on `2026-06-13-entry-item-redesign-design.md`,
`2026-06-13-mobile-touch-targets-design.md`, and
`2026-06-14-reading-pane-redesign-design.md`.

### Corrected emoji inventory

An initial scan undercounted: the sidebar is rendered by a JS custom element
(`<rdrs-sidebar>`), not Askama, and its emoji are **HTML numeric entities**
(`&#x1F4E1;`), so a codepoint regex over templates missed them. The complete
inventory:

| Surface | Location | Glyphs |
| --- | --- | --- |
| Summary-status badges | `templates/_entry_row.html:18–21` | ✅ ⏳ 🔄 ❌ |
| Sidebar nav (11) | `static/js/components/rdrs-sidebar.js` | 📬 ⭐ ✨ 📰 📡 🗂️ 🔍 📊 👤 ⚙️ 🛡️ |
| Sidebar chrome | `rdrs-sidebar.js` | ☰ (hamburger), × (close) |
| Flash dismiss | `templates/macros.html:9` | × |

Out of scope (kept as typography): the onboarding CTA arrow
`Add your first feed →` (`_entries_layout.html:61`); mid-dots, em-dashes, and
other punctuation.

## Icon design language

All icons follow the existing convention: inline `<svg class="ico"
viewBox="0 0 24 24" aria-hidden="true">`, `fill:none; stroke:currentColor;
stroke-width:2; round caps/joins`. Filled variants use `.is-filled`
(`fill:currentColor; stroke:none`). Icons scale to their container at
`1.15em` (the rule reading-pane actions already use), so every inline icon is
sized consistently against its surrounding text.

## 1. Summary-status icon set

One **4-point sparkle** glyph carries all four states; colour + fill + motion
disambiguate. Ties the badge to the AI-summary identity (same family as the
Summarize action's `sparkle()`).

New macro in `_icons.html`:

```
{% macro summary(filled) %}
  <svg class="ico{% if filled %} is-filled{% endif %}" viewBox="0 0 24 24" aria-hidden="true">
    {% if filled %}<path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/>
    {% else %}<g transform="translate(1.2 1.2) scale(0.9)"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></g>{% endif %}
  </svg>
{% endmacro %}
```

The outline variant is `scale(0.9)` on the same path (keeping `stroke-width:2`)
so that path + stroke lands on the **same footprint as the filled glyph and the
`star()` icon** — without it, a stroked shape reads ~1px larger than a filled
one. The filled path is centred at 12,12 with radius 9 to match `star()`.

State → treatment mapping in `_entry_row.html` (replaces the emoji `match`):

| State | Macro | Colour token |
| --- | --- | --- |
| completed | `summary(true)` | `--color-accent` (gold) |
| pending | `summary(false)` | `--color-text-muted` |
| processing | `summary(false)` + pulse | `--color-accent` |
| failed | `summary(true)` | `--color-error` (red) |

CSS: `.summary-badge*` classes set the colour (applied to the SVG via
`currentColor`); the existing processing **pulse** (opacity animation) is kept;
emoji-specific styling is removed. The badge `.ico` is sized `1.15em` of the
title (→ 16px desktop / 18px mobile).

## 2. Entry-row relayout

Drop the grid left-favicon column. Three stacked rows:

```
[title ........................] [★ ✦ · published-at]   ← head
[favicon] feed · category                               ← meta
[ read ] [ star ] [ original ]                          ← actions
```

`templates/_entry_row.html`:

- **head** — `.entry-head` is `display:flex; align-items:flex-start`. Title is
  `flex:1; min-width:0; overflow-wrap:break-word`. A right-pinned
  `.entry-status` cluster (`flex:none; align-self:flex-start; height:1.32em;
  align-items:center`) holds, in order: a **filled star** status glyph rendered
  only when `is_starred`, the **summary** badge, then `.entry-time`. Fixed
  height = title's first line, so icons centre on the first line and the cluster
  stays put as the title wraps.
- **meta** — `.entry-item-meta` becomes `display:flex; align-items:center;
  gap:8px`, leading with the 24px favicon (img or colour chip, reading-pane
  style, `margin-top:0`) followed by feed · category links.
- **actions** — unchanged markup (read/star/original forms keep their
  `data-testid`s and POST targets, so triage e2e is unaffected).

**Note (intentional redundancy):** the star appears both as a read-only status
glyph in the head (when starred) and as the toggle action in the actions row —
glance vs. act. Approved during design review.

CSS (`app.css`):

- Replace `.entry-item` grid (`grid-template-areas`) with simple block flow of
  the three rows. Remove the `fav` grid area.
- `.entry-status` cluster styles; status icons `1.15em` (16px desktop / 18px
  mobile); star filled `--color-accent`.
- `.entry-item-meta` flex with inline 24px favicon.
- **Action icon sizes aligned to reading pane:** set desktop
  `.entry-item-actions` font-size to `--font-sm` (14px) so action icons render
  16px (`1.15em`), pixel-matching `.rp-action`. Mobile already uses `--font-xl`
  (20px) → 23px icons, matching the reading-pane bottom bar.
- Favicon palette (`fav-c0…5`), `feed_initial()`, `feed_color_index()` reused
  unchanged.

## 3. Reading-pane back button

`templates/_reading_pane.html:15`: replace the `← Back to list` text with
`{% call icons::chevron_left() %}` + label **"Back"**.

CSS `.reading-pane-back-link` (mobile-only): add the nav-button outline — `1px
solid var(--color-border)`, `border-radius:var(--radius-md)`, `min-height:
var(--touch-min)`, padding, `display:inline-flex; align-items:center; gap` — so
it matches `.reading-pane-nav-btn` sitting beside it. Chevron 18px (matches the
prev/next icons; same glyph as the prev button).

## 4. Sidebar icons

Add new macros to `_icons.html` (stroke style, 24×24): `inbox`, `list`, `rss`,
`folder`, `search`, `barchart`, `user`, `cog`, `shield`, `menu`. Reuse
`star()`, `sparkle()`, `close()`.

| Item | Icon | Item | Icon |
| --- | --- | --- | --- |
| Unread | inbox | Statistics | bar chart |
| Starred | `star()` | Settings (`/user-settings`) | user (person) |
| Summarized | `sparkle()` | App (`/settings`) | cog (toothed gear) |
| All Entries | list | Admin | shield |
| Feeds | RSS mark | Menu toggle | hamburger |
| Categories | folder | Close (sidebar + flash) | `close()` |
| Search | magnifier | | |

**Rendering split:** the sidebar is a JS custom element building `innerHTML`
strings — it cannot call Askama macros. The SVG markup is therefore inlined in
`rdrs-sidebar.js` (a small per-item icon map), kept path-identical to the
`_icons.html` macros. This duplication is inherent to the JS-rendered sidebar
and is documented here. The flash dismiss (`macros.html`) is Askama and uses
`{% call icons::close() %}`.

CSS (`app.css`):

- Generalise the icon stroke styling so `.ico` works outside reading-pane
  contexts: a base `.ico { fill:none; stroke:currentColor; stroke-width:2;
  round caps/joins }`, sized per context (`.action-icon .ico { width:1.15em }`,
  `.sidebar-item-icon .ico { width:var(--icon-md); height:var(--icon-md) }`).
- `.sidebar-item-icon`: switch from font glyph (`font-size:1rem; text-align:
  center`) to an 18px SVG box; icon inherits `currentColor` (item text colour,
  accent when `.active`).
- **Remove the dark-mode `opacity:0.85` hack** (`app.css` ~298–305) — it existed
  only to tame emoji rendering and is unnecessary for stroke SVGs.
- `.sidebar-toggle` / `.sidebar-close`: size the inline SVG (~22px) in place of
  the font glyph; keep their 44px square mobile touch sizing and `aria-label`s.

## 5. Touch-target audit (all interactive components)

The 44px baseline (`54b0008`) already covers `button`/`.btn`, text links,
icon-only chrome buttons, `.sidebar-item`, text `input`s, `<select>`,
`textarea`. This task closes the remainder via **measurement, not inspection**.

**Audit:** a Playwright pass at 375px walks every interactive element
(`button, a, select, input, textarea, label:has(input), [role=button]`, custom
elements) across the main pages (entries, feeds, feed-edit, import, categories,
search, settings, user-settings, statistics) and records each rendered bounding
box, flagging anything < 44px in either axis.

The audit is implemented as a reusable spec, `e2e/scripts/touch-audit.spec.js`
(run via `scripts/audit.config.js`), kept in the tree as a regression tool.

**Audit results** — 11 distinct genuine sub-44px targets (caption `<label>`s
that pair with a separate control via `for=`, and inline `.entry-item-meta`
links, are excluded as non-targets/exemptions). Fixes, all `min-height`/
`min-width: 44px` with flex-centring, no background/text-decoration changes:

| Target | Measured | Fix | Breakpoint |
| --- | --- | --- | --- |
| Sidebar logout link | 49×19 | min-height 44px | mobile |
| Sidebar `.sidebar-logo` ("rdrs") | 41×30 | min-height 44px | mobile |
| **Entry title link** (primary tap) | 87×21 | **min-height 44px on `.entry-item-title`** (option A — CSS only; pads short single-line titles, multi-line already exceed it) | mobile |
| Filter tabs (`.tab-bar a`) | 39–42×44 | min-width 44px | mobile |
| Feeds filter (`.feed-filter-link`) | 41×44 | min-width 44px | mobile |
| Stats period (`.stats-period-btn`) | 41×44 | min-width 44px | mobile |
| Feed "edit" link | 39×44 | min-width 44px | mobile |
| Checkbox box (`input[type=checkbox]`) | 13×13 | enlarge to ~20px (`width/height` + `accent-color`) | mobile |
| Checkbox label row ("Disable HTTP/2") | 317×21 | wrapping `<label>` → 44px flex row | mobile |
| File input (`input[type=file]`) | 253×21 | min-height 44px | mobile |
| `<summary>` disclosure ("HTTP Settings") | 317×27 | **min-height 44px via padding** (not `display:flex`, so the native ▸/▾ marker is preserved) | mobile |

- `input[type="radio"]` — none exist; nothing to do.
- `.entry-item-meta a` inline links — intentionally ~28px (WCAG 2.5.8 AA + wrap
  preservation); documented exemption, unchanged.

### Desktop: "Mark Above as Read"

Separately from the mobile audit, `#mark-above-read` (`.btn-sm`,
`_entries_layout.html`) reads too small on **desktop**: `.btn-sm` is `--font-xs`
+ `space-1/space-2` padding (~24px tall), and the 44px floor only applies in the
`≤1024px` block, so desktop has no minimum. Fix: give this button a comfortable
desktop size — `--font-sm` with `space-2`/`space-4` padding (~36px tall) — while
keeping the mobile 44px from the touch baseline. Scoped to this button (not a
global `.btn-sm` change).

## Testing

- **Summary icons / entry row:** existing triage & entries e2e rely on
  `data-testid`s and POST targets, all preserved. Update any assertion that
  matched the old emoji or favicon-as-grid-column structure.
- **Back button:** update reading-pane e2e assertions matching the old
  "Back to list" text (now "Back" + chevron); `data-testid="reading-pane-back"`
  unchanged.
- **Sidebar:** nav `data-testid`s (`nav-unread`, `nav-feeds`, …) and chrome
  `aria-label`s unchanged; existing sidebar e2e should pass. Add an assertion
  that sidebar item icons render as `<svg>` (no emoji).
- **Touch targets:** the audit spec (`e2e/scripts/touch-audit.spec.js`) is
  retained as a regression tool; extend `e2e/features/responsive.feature` with
  per-control ≥44px assertions for each fixed control (entry title, sidebar
  logout/logo, filter/period pills, feed-edit link, checkbox label row, file
  input, `<summary>`). The "Mark Above as Read" fix is **desktop** — assert its
  rendered height at a desktop viewport (not the 44px mobile rule).
- **Build/run notes:** CSS is `include_str!`'d into the binary — `cargo build`
  before e2e after CSS/Rust edits; `npx bddgen` after `.feature` changes; tests
  run with `RDRS_FAST_HASH`; this box needs the OpenSSL env re-sourced before
  cargo/e2e.

## Out of scope

- Onboarding CTA arrow `Add your first feed →` (kept as CTA typography).
- Any non-icon typographic glyphs (`·`, `—`, `&middot;`, etc.).
- Behavioural changes to summary generation, starring, or navigation.
