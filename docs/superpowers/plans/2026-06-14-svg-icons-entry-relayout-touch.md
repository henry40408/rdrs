# SVG Icons, Entry-Row Relayout & Touch-Target Audit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all remaining emoji-as-icon usage with the inline SVG icon set, relayout the entry-list row to echo the reading pane (inline favicon + right-aligned status cluster), swap the reading-pane back arrow for a bordered chevron, and close every measured sub-44px touch-target gap plus the desktop "Mark Above as Read" sizing.

**Architecture:** Askama SSR templates + a macro icon set in `templates/_icons.html`; the sidebar is a JS custom element (`rdrs-sidebar.js`) that builds `innerHTML`, so its icons are inlined in JS (path-identical to the macros). All styling lives in the single `static/css/app.css`. CSS is `include_str!`'d into the binary, so a `cargo build` is required before e2e after any CSS/template edit. Tests are Playwright-BDD under `e2e/`.

**Tech Stack:** Rust + Askama, vanilla ES-module JS, hand-written CSS, Playwright-BDD (`@playwright/test`, `playwright-bdd`), `better-sqlite3` for seeding.

**Spec:** `docs/superpowers/specs/2026-06-14-svg-icons-entry-relayout-touch-design.md`

**Environment (this box):** `source /tmp/rdrs-env.sh` before every cargo/e2e command (OpenSSL). Tests run with `RDRS_FAST_HASH=1`. After `.feature` edits run `npx bddgen`. Already on branch `feat/entry-svg-icons-relayout-touch`.

---

## File Structure

- **`templates/_icons.html`** — add macros: `summary(filled)`, `inbox`, `list`, `rss`, `folder`, `search`, `barchart`, `user`, `cog`, `shield`, `menu`. (Existing `star`, `sparkle`, `close`, `chevron_left/right` reused.)
- **`templates/_entry_row.html`** — relayout into head / meta / actions; summary emoji → `summary()`; add starred status glyph; inline favicon in meta.
- **`templates/_reading_pane.html`** — back button: `←` text → `chevron_left()` + "Back".
- **`templates/macros.html`** — flash dismiss `&times;` → `close()`.
- **`static/js/components/rdrs-sidebar.js`** — emoji entities → inline SVG strings (icon map); hamburger ☰ → `menu` SVG; close × → `close` SVG.
- **`static/css/app.css`** — summary-badge classes; entry-row layout; `.entry-status` cluster; reading-pane back button border; generalize `.ico`/`.is-filled`; `.sidebar-item-icon` SVG sizing + remove dark opacity hack; `.sidebar-toggle`/`.sidebar-close` SVG sizing; touch fixes; Mark-Above desktop size.
- **`e2e/scripts/touch-audit.spec.js`, `e2e/scripts/audit.config.js`** — already written; commit as a retained regression tool.
- **`e2e/features/responsive.feature` + `e2e/steps/responsive.steps.js`** — touch-target assertions.
- **`e2e/features/*.feature`** — update any assertion matching old "Back to list" text / emoji.
- **`.gitignore`** — ignore `e2e/scripts/touch-audit-report.json`.

---

## Task 1: Summary-status icon macro

**Files:**
- Modify: `templates/_icons.html`

- [ ] **Step 1: Add the `summary(filled)` macro**

Append after the existing `chevron_right` macro in `templates/_icons.html`:

```
{% macro summary(filled) %}<svg class="ico{% if filled %} is-filled{% endif %}" viewBox="0 0 24 24" aria-hidden="true">{% if filled %}<path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/>{% else %}<g transform="translate(1.2 1.2) scale(0.9)"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></g>{% endif %}</svg>{% endmacro %}
```

Rationale (from spec): filled = radius-9 sparkle centred at 12,12 (matches `star()`); outline = same path scaled 0.9 so path+stroke lands on the same footprint.

- [ ] **Step 2: Verify template compiles**

Run: `source /tmp/rdrs-env.sh && cargo build`
Expected: builds clean (Askama compiles templates at build time; a macro typo fails the build).

- [ ] **Step 3: Commit**

```bash
git add templates/_icons.html
git commit -S -m "feat(icons): add summary() sparkle macro for status badges"
```

---

## Task 2: Swap summary emoji → SVG in the entry row

**Files:**
- Modify: `templates/_entry_row.html:17-23`
- Modify: `static/css/app.css` (`.summary-badge*` rules ~1585-1609)

- [ ] **Step 1: Replace the emoji `match` block**

In `templates/_entry_row.html`, replace lines 17-23 (the `.entry-item-badges` span) with:

```
<span class="entry-item-badges">{% match r.summary_status_str() %}
    {% when Some("completed") %}<span title="Has Summary" class="summary-badge" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
    {% when Some("pending") %}<span title="Pending" class="summary-badge-pending" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
    {% when Some("processing") %}<span title="Processing" class="summary-badge-processing" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
    {% when Some("failed") %}<span title="Failed" class="summary-badge-failed" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
    {% when _ %}
{% endmatch %}</span>
```

(`{%- import "_icons.html" as icons -%}` is already at the top of the file.)

- [ ] **Step 2: Update `.summary-badge*` CSS**

In `static/css/app.css`, replace the `.summary-badge*` block (~1585-1609) with:

```css
/* Summary-status badges — one sparkle glyph, colour + fill carry meaning.
   Icon sizes via 1.15em of the status-cluster font (set on .entry-status). */
.summary-badge,
.summary-badge-pending,
.summary-badge-processing,
.summary-badge-failed {
    display: inline-flex;
    align-items: center;
}
.summary-badge { color: var(--color-accent); }            /* completed, filled */
.summary-badge-pending { color: var(--color-text-muted); } /* outline */
.summary-badge-processing {
    color: var(--color-accent);                            /* outline, pulsing */
    animation: summary-pulse 1s ease-in-out infinite;
}
.summary-badge-failed { color: var(--color-error); }       /* failed, filled */

@keyframes summary-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.3; }
}
```

(If a `summary-pulse`/blink keyframe already exists, keep one definition — remove the old emoji-era one.)

- [ ] **Step 3: Build**

Run: `source /tmp/rdrs-env.sh && cargo build`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add templates/_entry_row.html static/css/app.css
git commit -S -m "feat(entries): summary status emoji -> sparkle SVG badge"
```

(Icon sizing within the status cluster is set in Task 3 alongside the relayout.)

---

## Task 3: Entry-row relayout (inline favicon + status cluster)

**Files:**
- Modify: `templates/_entry_row.html` (whole body)
- Modify: `static/css/app.css` (`.entry-item` grid block ~1055-1180; mobile ~1920-1958)
- Test: `e2e/features/responsive.feature` (+ steps) for entry-title ≥44px

- [ ] **Step 1: Add a failing e2e assertion for the entry-title tap height**

In `e2e/features/responsive.feature`, under the existing mobile scenario that seeds entries (the one asserting `.filter-bar select` ≥44px), add a step:

```gherkin
    Then the "[data-testid=entry-title-link]" control is at least 44px tall
```

If a generic "control is at least {int}px tall" step does not already exist, add to `e2e/steps/responsive.steps.js`:

```js
Then(
  'the {string} control is at least {int}px tall',
  async ({ page }, selector, min) => {
    const box = await page.locator(selector).first().boundingBox();
    expect(box.height).toBeGreaterThanOrEqual(min);
  }
);
```

- [ ] **Step 2: Regenerate BDD + run, verify it fails**

Run: `cd e2e && npx bddgen && source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 npx playwright test responsive -g "44px tall" --project=chromium`
Expected: FAIL (entry title currently ~21px tall).

- [ ] **Step 3: Rewrite `_entry_row.html` body**

Replace the body of `templates/_entry_row.html` (keep the leading `{%- import ... -%}` and the doc comment) with:

```
<div id="entry-row-{{ r.id }}" class="entry-item{% if r.is_read %} entry-read{% endif %}" data-entry-row data-entry-id="{{ r.id }}" data-testid="entry-item">
    <div class="entry-head">
        <a href="/entries/{{ r.id }}/fragment" data-swap="#reading-pane" class="entry-item-title {% if r.is_read %}entry-title-normal{% else %}entry-title-bold{% endif %}" data-testid="entry-title-link">{{ r.title }}</a>
        <span class="entry-status">
            {% if r.is_starred %}<span class="entry-status-star" title="Starred" aria-hidden="true">{% call icons::star(true) %}{% endcall %}</span>{% endif %}
            {% match r.summary_status_str() %}
            {% when Some("completed") %}<span title="Has Summary" class="summary-badge" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
            {% when Some("pending") %}<span title="Pending" class="summary-badge-pending" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
            {% when Some("processing") %}<span title="Processing" class="summary-badge-processing" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
            {% when Some("failed") %}<span title="Failed" class="summary-badge-failed" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
            {% when _ %}
            {% endmatch %}
            <time class="entry-time" datetime="{{ r.published_at_iso }}">{{ r.published_relative }}</time>
        </span>
    </div>

    <div class="muted entry-item-meta">
        {% if r.feed_has_icon %}
        <img class="entry-favicon" src="/api/feeds/{{ r.feed_id }}/icon" alt="" loading="lazy" width="24" height="24">
        {% else %}
        <span class="entry-favicon entry-favicon-chip fav-c{{ r.feed_color_index() }}" aria-hidden="true">{{ r.feed_initial() }}</span>
        {% endif %}
        <a href="/feeds/{{ r.feed_id }}/entries">{{ r.feed_title }}</a>
        <span class="meta-sep">·</span>
        <a href="/categories/{{ r.category_id }}/entries">{{ r.category_name }}</a>
    </div>

    <div class="entry-item-actions">
        <form method="post" action="/entries/{{ r.id }}/{% if r.is_read %}unread{% else %}read{% endif %}" data-swap="#entry-row-{{ r.id }}">
            <button type="submit" class="entry-action-btn" data-testid="entry-read-action" aria-label="{% if r.is_read %}Mark unread{% else %}Mark read{% endif %}"><span class="action-icon" aria-hidden="true">{% if r.is_read %}{% call icons::unread() %}{% endcall %}{% else %}{% call icons::check() %}{% endcall %}{% endif %}</span><span class="action-label">{% if r.is_read %}unread{% else %}read{% endif %}</span></button>
        </form>
        <form method="post" action="/entries/{{ r.id }}/{% if r.is_starred %}unstar{% else %}star{% endif %}" data-swap="#entry-row-{{ r.id }}">
            <button type="submit" class="entry-action-btn" data-testid="entry-star-action" aria-label="{% if r.is_starred %}Unstar{% else %}Star{% endif %}"><span class="action-icon" aria-hidden="true">{% if r.is_starred %}{% call icons::star(true) %}{% endcall %}{% else %}{% call icons::star(false) %}{% endcall %}{% endif %}</span><span class="action-label">{% if r.is_starred %}starred{% else %}star{% endif %}</span></button>
        </form>
        {% if let Some(link) = r.link.as_ref() %}<a href="{{ link }}" target="_blank" rel="noopener noreferrer" data-testid="entry-original-link" aria-label="Open original"><span class="action-icon" aria-hidden="true">{% call icons::external() %}{% endcall %}</span><span class="action-label">original</span></a>{% endif %}
    </div>
</div>
```

- [ ] **Step 4: Replace the `.entry-item` layout CSS**

In `static/css/app.css`, replace the `.entry-item` grid declaration and the favicon grid-area rules with block flow + the new cluster. Set `.entry-item`:

```css
.entry-item {
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--color-border-light);
    cursor: pointer;
    transition: background 0.1s;
}
```

Remove the `grid-template-columns`/`grid-template-areas`/`column-gap` lines and the `grid-area: fav|head|meta|foot` assignments on `.entry-favicon`, `.entry-head`, `.entry-item-meta`, `.entry-item-actions`. Then add/replace:

```css
/* Head row: title + right-pinned status cluster */
.entry-head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
}
.entry-item-title {
    flex: 1;
    min-width: 0;
    font-family: var(--font-ui);
    font-size: var(--font-sm);          /* 14px desktop */
    font-weight: 600;
    line-height: 1.32;
    color: var(--color-text);
    overflow-wrap: break-word;
    word-break: break-word;
}
.entry-item-title:hover { color: var(--color-accent); }
.entry-title-bold { font-weight: 600; }
.entry-title-normal { font-weight: 400; }

.entry-status {
    flex: none;
    align-self: flex-start;
    height: 1.32em;                     /* == title first line */
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--font-sm);          /* sizes the 1.15em icons (16px) */
}
.entry-status .ico { width: 1.15em; height: 1.15em; }
.entry-status-star { color: var(--color-accent); display: inline-flex; }
.entry-time {
    flex: none;
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
}

/* Meta row: inline favicon (reading-pane style) + feed · category */
.entry-item-meta {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-1);
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    line-height: 1.5;
    color: var(--color-text-secondary);
}
.entry-item-meta a { color: var(--color-text-secondary); }
.entry-item-meta a:hover { color: var(--color-accent); }
.entry-item-meta .meta-sep { color: var(--color-text-muted); }

/* Favicon now inline in the meta row (was a grid column) */
.entry-favicon { width: 24px; height: 24px; margin-top: 0; border-radius: var(--radius-control); flex: none; object-fit: cover; }

.entry-item-actions {
    display: flex;
    gap: var(--space-5);
    margin-top: var(--space-2);
    font-family: var(--font-ui);
    font-size: var(--font-sm);          /* 14px -> action icons 1.15em = 16px, matches reading pane */
    opacity: 0.72;
    transition: opacity 0.1s;
}
```

Keep the existing `.entry-item:hover`, `.entry-item.selected`, `.entry-item.entry-read`, `.entry-favicon-chip`, `.fav-c*`, `.entry-action-btn`, and hover-opacity rules. (`.entry-item-badges` rule, if any, can be dropped — replaced by `.entry-status`.)

- [ ] **Step 5: Mobile relayout adjustments**

In the `@media (max-width: 1024px)` block, within the entry-item rules (~1920-1958): set the title to 16px and the status cluster to size mobile icons at 18px, and pad the title to a 44px tap target (spec §5 option A):

```css
    .entry-item-title {
        font-size: var(--font-base);     /* 16px */
        display: flex;
        align-items: center;
        min-height: var(--touch-min);    /* 44px primary tap (touch fix) */
    }
    .entry-status { font-size: var(--font-base); }   /* icons 1.15em = 18px */
```

Keep the existing mobile `.entry-item-actions` full-width equal-thirds rules and `.action-label { display:none }`. The mobile `.entry-item-actions` font-size stays `--font-xl` (20px → 23px icons, matches reading-pane bottom bar); verify that rule is present, add `font-size: var(--font-xl);` if not.

- [ ] **Step 6: Build + run the title-height test**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && RDRS_FAST_HASH=1 npx playwright test responsive -g "44px tall" --project=chromium`
Expected: PASS.

- [ ] **Step 7: Run triage + entries e2e (no regressions from relayout)**

Run: `cd e2e && source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 npx playwright test triage entries --project=chromium`
Expected: PASS (data-testids and POST targets unchanged). Fix any assertion that referenced the old favicon-as-grid-column or `.entry-item-badges`.

- [ ] **Step 8: Commit**

```bash
git add templates/_entry_row.html static/css/app.css e2e/features/responsive.feature e2e/steps/responsive.steps.js
git commit -S -m "feat(entries): relayout row with inline favicon + right-aligned status cluster"
```

---

## Task 4: Reading-pane back button → bordered chevron

**Files:**
- Modify: `templates/_reading_pane.html:15`
- Modify: `static/css/app.css` (`.reading-pane-back-link` ~478-491; mobile ~1851)
- Test: `e2e/features/reading-pane*.feature` (text assertion)

- [ ] **Step 1: Update any e2e assertion of the old label**

Run: `rg -n "Back to list" e2e`
For each hit asserting the visible text, change the expected text to `Back`. (The `data-testid="reading-pane-back"` is unchanged, so locator-based steps need no change.)

- [ ] **Step 2: Replace the back-button markup**

`templates/_reading_pane.html:15` becomes:

```
        <button type="button" class="reading-pane-back-link" data-pane-back data-testid="reading-pane-back"><span class="action-icon" aria-hidden="true">{% call icons::chevron_left() %}{% endcall %}</span><span>Back</span></button>
```

- [ ] **Step 3: Add the bordered style**

Replace the `.reading-pane-back-link` rule (~478-491) with:

```css
.reading-pane-back-link {
    display: none;                       /* shown in the ≤1024px block */
    margin-right: auto;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--touch-min);
    padding: 0 var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: none;
    font: inherit;
    color: var(--color-accent);
    cursor: pointer;
}
.reading-pane-back-link .action-icon .ico { width: 18px; height: 18px; }
.reading-pane-back-link:hover {
    color: var(--color-accent-hover);
    border-color: var(--color-accent);
}
```

In the `@media (max-width: 1024px)` block, where `.reading-pane-back-link` is revealed (~1851), change `display: inline-flex;` (it currently sets display to show it). Ensure it reads `display: inline-flex;` so the flex gap/centring applies.

- [ ] **Step 4: Build + test**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && npx bddgen && RDRS_FAST_HASH=1 npx playwright test reading-pane --project=chromium`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add templates/_reading_pane.html static/css/app.css e2e/features
git commit -S -m "feat(reading-pane): back button uses bordered chevron + 'Back'"
```

---

## Task 5: Flash dismiss → close() SVG

**Files:**
- Modify: `templates/macros.html:9`

- [ ] **Step 1: Replace the `&times;`**

In `templates/macros.html` line 9, replace the dismiss button content `&times;` with the icon call (keep the `aria-label` and `onclick`):

```
<button type="button" class="banner-dismiss" aria-label="Dismiss notification" data-testid="flash-close" onclick="this.closest('.banner').remove();">{% call icons::close() %}{% endcall %}</button>
```

Ensure `macros.html` imports the icon macros at the top: if not present, add `{%- import "_icons.html" as icons -%}` as the first line.

- [ ] **Step 2: Size the icon**

In `static/css/app.css`, add (near `.banner-dismiss`):

```css
.banner-dismiss .ico { width: 18px; height: 18px; }
```

- [ ] **Step 3: Build + test**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && RDRS_FAST_HASH=1 npx playwright test -g "flash" --project=chromium`
Expected: PASS (dismiss still works via `data-testid="flash-close"`).

- [ ] **Step 4: Commit**

```bash
git add templates/macros.html static/css/app.css
git commit -S -m "feat(flash): dismiss button uses close() SVG"
```

---

## Task 6: Generalize `.ico` base styling

**Files:**
- Modify: `static/css/app.css` (~612-629)

- [ ] **Step 1: Promote `.ico`/`.is-filled` to base rules**

Replace the scoped block:

```css
.action-icon .ico,
.reading-pane-nav-btn .ico {
    width: 1.15em;
    height: 1.15em;
    display: block;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
}
.action-icon .ico.is-filled {
    fill: var(--color-accent);
    stroke: none;
}
```

with a base + sizing split (so `.ico` works in the sidebar too):

```css
/* Base inline-icon styling (stroke set; size per context). */
.ico {
    display: block;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
}
.ico.is-filled { fill: currentColor; stroke: none; }
.action-icon .ico,
.reading-pane-nav-btn .ico { width: 1.15em; height: 1.15em; }
.action-icon .ico.is-filled { fill: var(--color-accent); }
```

(`.is-filled` now fills with `currentColor` by default; the reading-pane action star keeps its explicit accent fill. The entry-status star fills with accent because `.entry-status-star { color: var(--color-accent) }`.)

- [ ] **Step 2: Build + smoke-test existing icons**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && RDRS_FAST_HASH=1 npx playwright test reading-pane triage --project=chromium`
Expected: PASS (existing action/star icons unchanged visually).

- [ ] **Step 3: Commit**

```bash
git add static/css/app.css
git commit -S -m "refactor(css): promote .ico/.is-filled to reusable base rules"
```

---

## Task 7: Sidebar icons (JS inline SVGs)

**Files:**
- Modify: `static/js/components/rdrs-sidebar.js` (icon spans ~167-228, toggle 172, close 178)
- Modify: `static/css/app.css` (`.sidebar-item-icon` ~290-306; `.sidebar-toggle`/`.sidebar-close`)
- Test: `e2e/features/responsive.feature` or a sidebar feature (icons are `<svg>`, no emoji)

- [ ] **Step 1: Add an icon map near the top of `rdrs-sidebar.js`**

After the imports/constants, add a frozen map of SVG strings (path-identical to `_icons.html`):

```js
const ICON = {
  inbox: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.4 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.4-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.8 1.1z"/></svg>',
  star: '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l2.7 5.5 6 .9-4.3 4.2 1 6L12 17.3 6.6 20l1-6L3.3 9.9l6-.9z"/></svg>',
  sparkle: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l1.7 4.6L18 9.3l-4.3 1.7L12 16l-1.7-4.9L6 9.3l4.3-1.7z"/><path d="M18 15l.7 1.8L20.5 17.5l-1.8.7L18 20l-.7-1.8L15.5 17.5l1.8-.7z"/></svg>',
  list: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 6h13M8 12h13M8 18h13"/><path d="M3.5 6h.01M3.5 12h.01M3.5 18h.01"/></svg>',
  rss: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 11a9 9 0 0 1 9 9"/><path d="M4 4a16 16 0 0 1 16 16"/><circle cx="5" cy="19" r="1.6" fill="currentColor" stroke="none"/></svg>',
  folder: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  search: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>',
  barchart: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 20v-6M12 20V4M18 20v-9"/><path d="M4 20h16"/></svg>',
  user: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8" r="4"/><path d="M4.5 21a7.5 7.5 0 0 1 15 0"/></svg>',
  cog: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
  shield: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l8 3v5c0 5-3.5 8-8 9.5C7.5 19 4 16 4 11V6z"/></svg>',
  menu: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 6h18M3 12h18M3 18h18"/></svg>',
  close: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>',
};
```

- [ ] **Step 2: Replace each emoji span with `ICON.<name>`**

In the `innerHTML` template, swap the entity contents of each `<span class="sidebar-item-icon">…</span>`:

| Item | Old | New |
| --- | --- | --- |
| Admin (167) | `&#x1F6E1;&#xFE0F;` | `${ICON.shield}` |
| Unread (183) | `&#x1F4EC;&#xFE0F;` | `${ICON.inbox}` |
| Starred (188) | `&#x2B50;&#xFE0F;` | `${ICON.star}` |
| Summarized (192) | `&#x2728;&#xFE0F;` | `${ICON.sparkle}` |
| All Entries (197) | `&#x1F4F0;&#xFE0F;` | `${ICON.list}` |
| Feeds (204) | `&#x1F4E1;&#xFE0F;` | `${ICON.rss}` |
| Categories (208) | `&#x1F5C2;&#xFE0F;` | `${ICON.folder}` |
| Search (214) | `&#x1F50D;&#xFE0F;` | `${ICON.search}` |
| Statistics (218) | `&#x1F4CA;&#xFE0F;` | `${ICON.barchart}` |
| Settings (222) | `&#x1F464;&#xFE0F;` | `${ICON.user}` |
| App (226) | `&#x2699;&#xFE0F;` | `${ICON.cog}` |

And the chrome:
- Toggle (172): `>&#9776;<` → `>${ICON.menu}<`
- Close (178): `>&times;<` → `>${ICON.close}<`

- [ ] **Step 3: Update `.sidebar-item-icon` + toggle/close CSS, remove dark hack**

In `static/css/app.css`, replace `.sidebar-item-icon` and the two dark-mode opacity rules (~290-306) with:

```css
.sidebar-item-icon {
    width: var(--icon-md);
    height: var(--icon-md);
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
}
.sidebar-item-icon .ico { width: var(--icon-md); height: var(--icon-md); }
```

(Delete the `[data-theme="dark"] .sidebar-item-icon { opacity: 0.85 }` and the `prefers-color-scheme: dark` variant — the emoji-rendering hack.) For the chrome buttons, add:

```css
.sidebar-toggle .ico { width: 22px; height: 22px; }
.sidebar-close .ico { width: 20px; height: 20px; }
```

and remove any `font-size` on `.sidebar-toggle`/`.sidebar-close` that only sized the old glyph (keep their padding/border and the 44px mobile square sizing).

- [ ] **Step 4: Add a failing-then-passing e2e assertion (icons are SVG)**

In a sidebar-covering feature (e.g. add to `responsive.feature` a desktop scenario, or `auth.feature` after sign-in), add a step asserting the nav icon is an SVG:

```gherkin
    Then the "[data-testid=nav-feeds] .sidebar-item-icon svg" element is visible
```

Reuse or add a generic visibility step in steps if needed:

```js
Then('the {string} element is visible', async ({ page }, sel) => {
  await expect(page.locator(sel).first()).toBeVisible();
});
```

- [ ] **Step 5: Build + test**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && npx bddgen && RDRS_FAST_HASH=1 npx playwright test responsive auth --project=chromium`
Expected: PASS. Manually confirm (or via the audit) no emoji remain in the sidebar.

- [ ] **Step 6: Commit**

```bash
git add static/js/components/rdrs-sidebar.js static/css/app.css e2e/features e2e/steps
git commit -S -m "feat(sidebar): replace emoji nav icons + hamburger/close with SVGs"
```

---

## Task 8: Touch-target fixes (CSS)

**Files:**
- Modify: `static/css/app.css` (mobile `@media (max-width: 1024px)` touch block ~1997-2072)
- Test: `e2e/features/responsive.feature` (+ steps)

- [ ] **Step 1: Add failing assertions for each fixed control**

In `e2e/features/responsive.feature`, in the mobile scenario(s) that visit feeds/feed-edit/import, add (using the height/width steps; add a width step if missing):

```gherkin
    Then the "[data-testid=mark-above-btn]" control is at least 44px tall
    # on feed-edit page:
    Then the "input[type=checkbox]" control is at least 44px wide
    Then the "input[type=file]" control is at least 44px tall
    Then the "summary" control is at least 44px tall
```

Add to `e2e/steps/responsive.steps.js` if absent:

```js
Then(
  'the {string} control is at least {int}px wide',
  async ({ page }, selector, min) => {
    const box = await page.locator(selector).first().boundingBox();
    expect(box.width).toBeGreaterThanOrEqual(min);
  }
);
```

- [ ] **Step 2: Run, verify failures**

Run: `cd e2e && npx bddgen && source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 npx playwright test responsive --project=chromium`
Expected: FAIL on the new assertions.

- [ ] **Step 3: Add the touch rules**

In the mobile touch-baseline block (~1997-2072), append:

```css
    /* Sidebar chrome links */
    .sidebar-logo,
    .sidebar-footer a {
        display: inline-flex;
        align-items: center;
        min-height: var(--touch-min);
    }

    /* Horizontal pills: tabs, feed filter, stats period, feed-row edit link */
    .tab-bar a,
    .feed-filter-link,
    .stats-period-btn,
    .feed-actions a {
        min-width: var(--touch-min);
        justify-content: center;
    }

    /* Checkbox + its wrapping label become a 44px tappable row */
    label:has(> input[type="checkbox"]),
    label:has(> input[type="radio"]) {
        display: inline-flex;
        align-items: center;
        gap: var(--space-2);
        min-height: var(--touch-min);
    }
    input[type="checkbox"],
    input[type="radio"] {
        width: 1.25rem;
        height: 1.25rem;
        accent-color: var(--color-accent);
    }

    /* File input + disclosure summary */
    input[type="file"] { min-height: var(--touch-min); }
    summary {
        min-height: var(--touch-min);
        padding: var(--space-3) 0;   /* keeps native ▸/▾ marker (no display:flex) */
        box-sizing: border-box;
    }
```

Note the feed-row "edit" link selector: confirm the actual class with `rg -n "edit" templates/feeds.html` and adjust `.feed-actions a` to match (e.g. it may be `.feed-row a` or a `.btn-sm`). Use the real selector.

- [ ] **Step 4: Build + run**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && RDRS_FAST_HASH=1 npx playwright test responsive --project=chromium`
Expected: PASS.

- [ ] **Step 5: Re-run the audit to confirm zero non-exempt gaps**

Run: `cd e2e && source /tmp/rdrs-env.sh && npx playwright test --config=scripts/audit.config.js`
Expected: the console summary reports only inline-text exemptions remaining.

- [ ] **Step 6: Commit**

```bash
git add static/css/app.css e2e/features e2e/steps
git commit -S -m "fix(a11y): 44px touch targets for sidebar chrome, pills, checkbox, file, summary"
```

---

## Task 9: "Mark Above as Read" desktop sizing

**Files:**
- Modify: `static/css/app.css`
- Test: `e2e/features/responsive.feature` (desktop viewport assertion)

- [ ] **Step 1: Add a failing desktop assertion**

In `responsive.feature`, add a desktop-viewport scenario (or reuse one at default size) visiting a feed page with entries:

```gherkin
    Then the "[data-testid=mark-above-btn]" control is at least 34px tall
```

(at the default/desktop viewport — do not set 375px for this one).

- [ ] **Step 2: Run, verify it fails**

Run: `cd e2e && npx bddgen && source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 npx playwright test responsive -g "Mark Above" --project=chromium`
Expected: FAIL (~24px tall).

- [ ] **Step 3: Add the desktop size rule**

In `static/css/app.css`, near `.mark-above-form`, add a scoped override (the button keeps `.btn-sm` markup but is sized comfortably on desktop):

```css
/* "Mark Above as Read" reads cramped at btn-sm on desktop; give it a
   comfortable size. Mobile still gets 44px from the touch baseline. */
#mark-above-read {
    font-size: var(--font-sm);
    padding: var(--space-2) var(--space-4);
}
```

- [ ] **Step 4: Build + run**

Run: `source /tmp/rdrs-env.sh && cargo build && cd e2e && RDRS_FAST_HASH=1 npx playwright test responsive -g "Mark Above" --project=chromium`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add static/css/app.css e2e/features
git commit -S -m "fix(ui): enlarge 'Mark Above as Read' on desktop"
```

---

## Task 10: Retain the audit tool + ignore its report

**Files:**
- Create: `e2e/scripts/touch-audit.spec.js` (already on disk)
- Create: `e2e/scripts/audit.config.js` (already on disk)
- Modify: `.gitignore`

- [ ] **Step 1: Ignore the generated report**

Add to the repo `.gitignore`:

```
e2e/scripts/touch-audit-report.json
```

- [ ] **Step 2: Commit the audit tool**

```bash
git add e2e/scripts/touch-audit.spec.js e2e/scripts/audit.config.js .gitignore
git commit -S -m "test(e2e): retain 375px touch-target audit as a regression tool"
```

---

## Task 11: Full verification sweep

- [ ] **Step 1: fmt + clippy + build**

Run: `source /tmp/rdrs-env.sh && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo build`
Expected: clean.

- [ ] **Step 2: Rust unit/integration tests**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run`
Expected: PASS.

- [ ] **Step 3: Full e2e suite**

Run: `cd e2e && npx bddgen && source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 npx playwright test --project=chromium`
Expected: all green; 0 `@skip`.

- [ ] **Step 4: Final emoji sweep (entities + literals)**

Run:
```bash
rg -nP '&#x?1F[0-9A-Fa-f]{3};|&#x?2[0-9A-Fa-f]{3};|[\x{1F000}-\x{1FAFF}\x{2600}-\x{27BF}\x{2B00}-\x{2BFF}]' templates static/js | rg -v 'macros.html|_icons.html'
```
Expected: no UI emoji remain (the onboarding `→` in `_entries_layout.html` is the only allowed glyph; `←`/`☰`/emoji gone).

- [ ] **Step 5: Push the branch**

```bash
git push -u origin feat/entry-svg-icons-relayout-touch
```

---

## Self-Review notes

- **Spec coverage:** §1 summary icons → Tasks 1-2; §2 entry relayout → Task 3; §3 back button → Task 4; §4 sidebar (nav + chrome + flash) → Tasks 5, 7; §5 touch audit + all 11 fixes → Tasks 8, 10; entry-title (option A) → Task 3 step 5; Mark Above desktop → Task 9; `.ico` generalisation (needed by sidebar) → Task 6.
- **Ordering:** macro (1) → entry badge (2) → relayout (3) depends on `summary()` + `.entry-status`; `.ico` generalisation (6) precedes sidebar (7) which relies on the base `.ico` rule; touch fixes (8) after layout so measurements are final.
- **Naming consistency:** `entry-status` (cluster), `entry-status-star`, `summary-badge*`, `ICON.<name>` map, `summary(filled)` macro — used identically across tasks.
- **No silent caps:** the audit (Task 8 step 5) re-runs to confirm only inline-text exemptions remain.
