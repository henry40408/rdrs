# Entry-Row Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the entry-list row to restore the mark-read and open-original controls, fix the star padding bug, drop the category link, and apply the reviewed layout (2-row grid, dot toggle, overlay actions, summary-in-timerow).

**Architecture:** SSR Askama template (`_entry_row.html`) + embedded CSS (`app.css`). No Rust/route/JS changes — existing `/entries/{id}/{read,unread,star,unstar}` endpoints and `_entry_actions_multi.html` swap response are reused. Assets are `include_str!`-embedded, so a `cargo build` is mandatory before e2e/screenshots.

**Tech Stack:** Rust/Axum, Askama, vanilla CSS (CSS grid, custom properties), Playwright BDD e2e.

## Global Constraints

- Design source of truth: `docs/superpowers/specs/2026-07-04-entry-row-redesign-design.md`.
- Preserve these hooks verbatim: `#entry-row-{id}`, `data-entry-row`, `data-entry-id`, `data-entry-link`, `data-testid="entry-item"`, title link `data-swap="#reading-pane"` + `data-testid="entry-title-link"`, star button `data-testid="entry-star-action"`, classes `.entry-item/.entry-read/.selected/.entry-item-title/.entry-item-meta/.entry-favicon/.entry-status/.entry-star-form/.entry-star`, and `.entry-status:empty{display:none}`.
- Pointer control size 24px; touch 36px under `@media (any-pointer: coarse)`. Do NOT change the `--touch-min` (44px) token.
- Keep the feed link `<a href="/feeds/{id}/entries">`; remove only the category link + `.meta-sep`.
- GPG-sign commits; stage files explicitly (no `git add -A`). Run `cargo fmt` before committing Rust-adjacent changes (none here, but fmt-check must pass).

---

### Task 1: Rewrite `templates/_entry_row.html`

**Files:**
- Modify: `templates/_entry_row.html` (full rewrite of the row body)

**Interfaces:**
- Consumes (already on `RowView`, unchanged): `r.id`, `r.is_read`, `r.is_starred`, `r.link`, `r.title`, `r.feed_id`, `r.feed_title`, `r.feed_has_icon`, `r.feed_color_index()`, `r.feed_initial()`, `r.published_at_iso`, `r.published_relative`, `r.summary_status_str()`. Icons `icons::star(filled)`, `icons::summary(filled)`, `icons::external()`.
- Produces: DOM the CSS in Tasks 2–3 targets — `.entry-marker > .unread-toggle` (form), `.entry-item-title`, `.entry-timerow > .entry-status + .entry-time`, `.entry-item-meta > .entry-favicon + .entry-meta-text > .entry-feed`, `.rail-actions > .entry-star-form > .entry-star` + `.entry-open-ext`.

- [ ] **Step 1: Replace the row template body**

```html
{%- import "_icons.html" as icons -%}
{# Single entry row — "wire log" v2. Grid: marker(dot=read toggle) | content
   (title + feed meta) | timerow(summary + relative time). The star + open-
   original actions are an absolute overlay (bottom-right, centred on the meta
   line) so they don't reserve column width — keeps the title wide on touch.
   Restored vs 0.55: mark-read (dot form) and open-original (↗). Category link
   removed from meta. Preserved for app.js + e2e: #entry-row-{id}, data-entry-row,
   data-entry-id, data-entry-link, data-testid="entry-item", title link
   (data-swap="#reading-pane", data-testid="entry-title-link"), the feed meta
   link, the star form (data-testid="entry-star-action"), the .entry-status
   summary cluster (SSE renderSummaryBadge), and .entry-read/.selected hooks. #}
<div id="entry-row-{{ r.id }}" class="entry-item{% if r.is_read %} entry-read{% endif %}" data-entry-row data-entry-id="{{ r.id }}"{% if let Some(link) = r.link.as_ref() %} data-entry-link="{{ link }}"{% endif %} data-testid="entry-item">
    {# Marker: dot = idempotent read/unread toggle (mirrors star form). #}
    <form class="entry-marker" method="post" action="/entries/{{ r.id }}/{% if r.is_read %}unread{% else %}read{% endif %}" data-swap="#entry-row-{{ r.id }}">
        <button type="submit" class="unread-toggle" data-testid="entry-read-toggle" aria-label="{% if r.is_read %}Mark unread{% else %}Mark read{% endif %}" title="{% if r.is_read %}Mark unread{% else %}Mark read{% endif %} (m)"></button>
    </form>

    <a href="/entries/{{ r.id }}/fragment" data-swap="#reading-pane" title="{{ r.title }}" class="entry-item-title {% if r.is_read %}entry-title-normal{% else %}entry-title-bold{% endif %}" data-testid="entry-title-link">{{ r.title }}</a>

    <span class="entry-timerow">
        {# Summary-status cluster relocated here (was meta tail). Empty span when
           no summary so .entry-status:empty hides it; SSE swaps this node. #}
        <span class="entry-status">
            {%- match r.summary_status_str() -%}
            {%- when Some("completed") -%}
            <span title="Has Summary" class="summary-badge" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
            {%- when Some("pending") -%}
            <span title="Pending" class="summary-badge-pending" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
            {%- when Some("processing") -%}
            <span title="Processing" class="summary-badge-processing" aria-hidden="true">{% call icons::summary(false) %}{% endcall %}</span>
            {%- when Some("failed") -%}
            <span title="Failed" class="summary-badge-failed" aria-hidden="true">{% call icons::summary(true) %}{% endcall %}</span>
            {%- when _ -%}
            {%- endmatch -%}
        </span>
        <time class="entry-time" datetime="{{ r.published_at_iso }}">{{ r.published_relative }}</time>
    </span>

    <div class="entry-item-meta">
        {% if r.feed_has_icon %}
        <img class="entry-favicon" src="/api/feeds/{{ r.feed_id }}/icon" alt="" loading="lazy" width="15" height="15">
        {% else %}
        <span class="entry-favicon entry-favicon-chip fav-c{{ r.feed_color_index() }}" aria-hidden="true">{{ r.feed_initial() }}</span>
        {% endif %}
        <span class="entry-meta-text"><a class="entry-feed" href="/feeds/{{ r.feed_id }}/entries">{{ r.feed_title }}</a></span>
    </div>

    <div class="rail-actions">
        <form class="entry-star-form" method="post" action="/entries/{{ r.id }}/{% if r.is_starred %}unstar{% else %}star{% endif %}" data-swap="#entry-row-{{ r.id }}">
            <button type="submit" class="entry-star{% if r.is_starred %} starred{% endif %}" data-testid="entry-star-action" aria-label="{% if r.is_starred %}Unstar{% else %}Star{% endif %}" title="{% if r.is_starred %}Unstar{% else %}Star{% endif %} (f)">{% call icons::star(r.is_starred) %}{% endcall %}</button>
        </form>
        {% if let Some(link) = r.link.as_ref() %}
        <a class="entry-open-ext" href="{{ link }}" target="_blank" rel="noopener noreferrer" data-testid="entry-open-original" aria-label="Open original" title="Open original (v)">{% call icons::external() %}{% endcall %}</a>
        {% endif %}
    </div>
</div>
```

- [ ] **Step 2: Verify template compiles**

Run: `cargo build`
Expected: builds clean (Askama compiles the template). If it fails on a missing method/field, cross-check the name against `RowView` in `src/handlers/entries.rs` and fix the template.

- [ ] **Step 3: Commit**

```bash
git add templates/_entry_row.html
git commit -S -m "feat(ui): restore read/open-original row controls, drop category from meta"
```

---

### Task 2: Rewrite the base `.entry-item` CSS block

**Files:**
- Modify: `static/css/app.css` — the entry-list block (currently ~lines 1346–1515)

**Interfaces:**
- Consumes: tokens `--space-2`, `--radius-md`, `--font-ui`, `--font-mono`, `--color-*`. DOM from Task 1.
- Produces: the pointer (24px) layout; Task 3 layers touch (36px) on top.

- [ ] **Step 1: Replace the block from `.entry-item {` through the `.entry-star.starred .ico` rule** (the `/* ===== Entry list — the "wire log" ===== */` section) with:

```css
/* ===== Entry list — the "wire log" v2 =====
   Grid rows: [marker | title | timerow] / [ (marker) | meta | (timerow) ].
   marker = dot(read toggle); timerow = summary + relative time; star + open-
   original are an absolute overlay centred on the meta row (they do not reserve
   column width, so the title stays wide — esp. on touch). Sizes via --sz/--mk;
   touch bumps them under @media (any-pointer: coarse). Hooks (.entry-item,
   .selected, .entry-read, .entry-item-title, .entry-item-meta, .entry-favicon,
   .entry-status, .entry-star-form) preserved for app.js + e2e. */
.entry-item {
    --sz: 24px;                 /* pointer control hit box (== WCAG AA floor) */
    --mk: 24px;                 /* marker column width */
    --dot-shift: -1px;          /* dot vs title first-line centre nudge */
    --meta-pad: 48px;           /* clear the absolute actions overlay */
    --act-bottom: 10.5px;       /* centre 24px actions on the meta line */
    position: relative;
    display: grid;
    grid-template-columns: var(--mk) minmax(0, 1fr) auto;
    grid-template-rows: auto auto;
    column-gap: var(--space-2);
    row-gap: 4px;
    align-items: start;
    padding: 12px 14px 12px 12px;
    border-bottom: 1px solid var(--color-border-light);
    cursor: pointer;
    transition: background 0.1s;
}
.entry-item:hover { background: var(--color-bg-secondary); }
.entry-item.selected { background: var(--color-accent-subtle); }
.entry-item.selected::before {
    content: "";
    position: absolute;
    left: 0; top: 0; bottom: -1px;
    width: 2px;
    background: var(--color-accent);
}

/* Marker: dot = read/unread toggle, aligned to the title's first-line centre. */
.entry-marker { grid-column: 1; grid-row: 1; justify-self: center; align-self: start; margin: 0; padding: 0; display: flex; }
.unread-toggle {
    width: var(--sz); height: var(--sz);
    margin-top: var(--dot-shift);
    display: grid; place-items: center;
    border: none; background: none; padding: 0;
    border-radius: 50%;
    cursor: pointer;
    transition: background 0.1s;
}
.unread-toggle::before {
    content: "";
    width: 9px; height: 9px;
    border-radius: 50%;
    background: var(--color-accent);
    transition: background 0.1s, box-shadow 0.1s;
}
.entry-item.entry-read .unread-toggle::before {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--color-text-muted);
}
.unread-toggle:hover { background: var(--color-accent-subtle); }
.entry-item.entry-read .unread-toggle:hover::before { box-shadow: inset 0 0 0 1.5px var(--color-accent); }

/* Title (col2 row1) — 3-line clamp; hover affordance restored. */
.entry-item-title {
    grid-column: 2; grid-row: 1;
    min-width: 0;
    font-family: var(--font-ui);
    font-size: 16px; font-weight: 600;
    line-height: 1.35; letter-spacing: -0.005em;
    color: var(--color-text);
    text-decoration: none;
    overflow-wrap: break-word; word-break: break-word;
    display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3;
    overflow: hidden;
    transition: color 0.1s;
}
.entry-item.entry-read .entry-item-title,
.entry-title-normal { font-weight: 500; color: var(--color-text-secondary); }
.entry-title-bold { font-weight: 600; }
.entry-item:hover .entry-item-title { color: var(--color-accent); }

/* Timerow (col3 row1) — summary badge + relative time, on the title's first line. */
.entry-timerow {
    grid-column: 3; grid-row: 1;
    justify-self: end; align-self: start;
    padding-top: 2px;
    display: inline-flex; align-items: center; gap: 5px;
    white-space: nowrap;
}
.entry-time { font-family: var(--font-mono); font-size: 13px; line-height: 1.3; color: var(--color-text-muted); white-space: nowrap; }

/* Meta (col2 row2) — favicon + feed only, single-line ellipsis, centred on row2. */
.entry-item-meta {
    grid-column: 2; grid-row: 2;
    align-self: center;
    display: flex; align-items: center;
    padding-right: var(--meta-pad);
    font-family: var(--font-ui); font-size: 14px; line-height: 1.5;
    color: var(--color-text-muted);
    min-width: 0;
}
.entry-item-meta a { color: var(--color-text-secondary); font-weight: 500; text-decoration: none; }
.entry-item-meta a:hover { color: var(--color-accent); }
.entry-item.entry-read .entry-item-meta a { color: var(--color-text-muted); font-weight: 400; }
.entry-item.entry-read .entry-item-meta a:hover { color: var(--color-accent); }
.entry-meta-text { min-width: 0; flex: 1; overflow: hidden; }
.entry-feed { display: block; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }

/* Favicon (unchanged look). */
.entry-favicon {
    width: 15px; height: 15px;
    border-radius: 3px; margin-right: 5px;
    flex: none; object-fit: cover;
    background: #ffffff;
    box-shadow: inset 0 0 0 1px var(--color-border-light);
}
.entry-favicon-chip {
    display: inline-flex; align-items: center; justify-content: center;
    color: #fff; font-family: var(--font-mono); font-size: 8.5px; font-weight: 600;
    box-shadow: none;
}
.fav-c0 { background: #7C6F9B; }
.fav-c1 { background: #B05B3B; }
.fav-c2 { background: #3B7A6B; }
.fav-c3 { background: #4A6FA5; }
.fav-c4 { background: #A6563E; }
.fav-c5 { background: #6E8B3D; }

/* Summary-status cluster (relocated to timerow). completed/failed filled;
   pending/processing hollow muted. Kept small; SSE renderSummaryBadge target. */
.entry-status { flex: none; display: inline-flex; align-items: center; }
.entry-status:empty { display: none; }
.entry-status .ico { width: 13px; height: 13px; }

/* Actions overlay — star + open-original, bottom-right, centred on the meta row.
   Absolute so they don't reserve column width. No hover-reveal. */
.rail-actions {
    position: absolute;
    right: 14px; bottom: var(--act-bottom);
    display: flex; align-items: center; gap: 2px;
}
.entry-star-form { margin: 0; padding: 0; display: flex; }
.entry-star,
.entry-open-ext {
    width: var(--sz); height: var(--sz);
    display: grid; place-items: center;
    border: none; background: none; padding: 0; margin: 0;
    border-radius: var(--radius-md);
    color: var(--color-text-muted);
    text-decoration: none;
    cursor: pointer;
    transition: color 0.1s, background 0.1s;
}
.entry-star:hover,
.entry-open-ext:hover { color: var(--color-accent); background: var(--color-accent-subtle); }
.entry-star.starred { color: var(--color-accent); }
.entry-star .ico { width: 14px; height: 14px; }
.entry-open-ext .ico { width: 13px; height: 13px; }
.entry-star.starred .ico { fill: currentColor; stroke: currentColor; }
```

Notes for the implementer:
- This **deletes** the old `.entry-time { flex-direction:column … }` gutter rules, the `.unread-dot` rules, and the `.entry-star { margin:-3px -4px 0 0 }` block (the padding bug). Remove them entirely — do not leave orphans.
- The `.summary-badge*` colour rules (elsewhere in the file, ~lines 1920-1934) already set completed=accent, pending=muted, processing, failed=error. Leave them; they now render in the timerow. Verify `.summary-badge` (completed) uses a filled icon — it calls `icons::summary(true)` in the template, which is filled. Good.

- [ ] **Step 2: Build and eyeball**

Run: `cargo build` then start the app or open a rendered list. Confirm: dot aligns to the title's first line, single-line rows have no trailing gap, `[★ ↗]` sit bottom-right aligned to the feed line, summary sparkle (completed) is filled next to the time.

- [ ] **Step 3: Commit**

```bash
git add static/css/app.css
git commit -S -m "feat(ui): entry-row 2-row grid, dot toggle, overlay actions, summary in timerow"
```

---

### Task 3: Touch sizing (`any-pointer: coarse`) + clean up stale mobile/touch overrides

**Files:**
- Modify: `static/css/app.css` — the mobile width block (~lines 2314-2346) and the `@media (hover: none)` block (~lines 3129-3140), plus a new `@media (any-pointer: coarse)` block.

**Interfaces:**
- Consumes: the base block from Task 2 (`--sz`, `--mk`, `--dot-shift`, `--meta-pad`, `--act-bottom`).
- Produces: 36px entry-row controls wherever touch is available.

- [ ] **Step 1: In the mobile width block, remove the stale entry-row overrides.** Delete the `grid-template-columns: 36px …` override (the base rule + `--mk` handle columns now) and the `.entry-star { width:44px; height:44px; margin:-10px -10px 0 0 }` block and its `.entry-star.starred` line. Keep `.entry-item { padding: 14px 12px 14px 14px }`, `.entry-item-title { font-size: var(--font-base) }`, `.entry-item-meta { margin-top: 4px }` (harmless with row-gap), and the `.entry-item-meta a { display:inline-block; padding:.35rem 0 }` taller-hit-box rule.

- [ ] **Step 2: In the `@media (hover: none)` block, remove the `.entry-star { width:44px; height:44px; margin:-10px -12px 0 0 }` override and its `.entry-star.starred` line.** Leave everything else in that block (kbd hiding, the generic `button … { min-height: var(--touch-min) }` list, `.sidebar-item`, etc.) untouched.

- [ ] **Step 3: Add a new touch block for the entry-row controls.** Place it AFTER the width-based media blocks (same source-order-wins reasoning as the existing comment). Use explicit `min-width/min-height` so it beats the generic `button { min-height: 44px }` (element selector) that also matches on pure-touch devices:

```css
/* ===== Entry-row touch targets =====
   Any device with a coarse pointer (phones, tablets, and hybrids like touch
   laptops / iPad+trackpad) gets 36px row controls — comfortably above WCAG AA
   (24px) without the density cost of 44px. Pure-mouse desktops keep the compact
   24px base. Explicit min-width/height out-specifies the generic touch
   button{min-height:44px}. --touch-min (44px) is intentionally NOT reused here. */
@media (any-pointer: coarse) {
    .entry-item {
        --sz: 36px;
        --mk: 36px;
        --dot-shift: -7px;
        --meta-pad: 80px;
        --act-bottom: 4.5px;
    }
    .unread-toggle,
    .entry-star,
    .entry-open-ext {
        min-width: 36px; min-height: 36px;
    }
}
```

- [ ] **Step 4: Build + fmt-check + clippy**

Run:
```bash
cargo build
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```
Expected: all clean (no Rust changed, but CI gates must pass). CSS has no linter in-repo.

- [ ] **Step 5: Commit**

```bash
git add static/css/app.css
git commit -S -m "feat(ui): entry-row 36px touch targets via any-pointer:coarse"
```

---

### Task 4: e2e, screenshots, and manual verification

**Files:**
- Possibly modify: `e2e/features/*.feature` or step defs IF a scenario targeted the removed category link or the old star position.
- Regenerate: `screenshots/*.png` (4 images referenced by `README.md`).

**Interfaces:**
- Consumes: the rebuilt binary (assets embedded at compile time).

- [ ] **Step 1: Rebuild so e2e/screenshots see the new assets**

Run: `cargo build`
(The e2e global-setup skips the build if a binary exists, so this is mandatory after CSS/template edits.)

- [ ] **Step 2: Run the affected e2e features**

Run: `cd e2e && npm ci` (first time) then `npx playwright test --grep "entr|star|read|unread|summary|sse" -x`
Expected: green. If a scenario fails because it asserted the category link text or the old star DOM position, update the scenario/step to the new structure (feed-only meta; star now in `.rail-actions`; new `data-testid`s `entry-read-toggle` / `entry-open-original` available). Do NOT loosen assertions beyond what the redesign changed.

- [ ] **Step 3: Add coverage for the restored controls (only if a feature file already covers row actions).** If `e2e/features` has an entry-actions feature, add steps: clicking `[data-testid=entry-read-toggle]` toggles `.entry-read` on the row; `[data-testid=entry-open-original]` has `href` == the entry link. Keep it minimal; follow existing BDD style.

- [ ] **Step 4: Regenerate screenshots**

Run: `cargo build && cd e2e && npm run screenshots`
Then `git status screenshots/`. Inspect the diffs. If any image differs by only a few dozen bytes (favicon-timing noise, a known caveat), re-run once to confirm it's noise and `git checkout -- <that file>`; commit only images with real visual changes.

- [ ] **Step 5: Manual pass in the browser**

Verify against the spec: dot toggles read state (row gains/loses `.entry-read`, sidebar count updates via the multi-swap); star toggles; `↗` opens the source in a new tab; summary(completed) filled next to time; single-line row has no bottom gap; resize / device-emulate a coarse pointer to confirm 36px controls and that the title does not wrap more aggressively than on pointer.

- [ ] **Step 6: Commit e2e + screenshots**

```bash
git add screenshots/  # only the changed images, explicitly
# plus any e2e files touched, staged by name
git commit -S -m "test(e2e): cover entry-row read/open controls; refresh screenshots"
```

- [ ] **Step 7: Finish the branch**

REQUIRED SUB-SKILL: Use superpowers:finishing-a-development-branch (verify tests → present options → PR/merge per user choice).

## Self-Review

- **Spec coverage:** dot toggle (T1/T2), open-original (T1/T2), category removal (T1/T2 meta), summary relocation+filled (T1 template + base colours), star padding fix (T2 deletes margin), 2-row alignment (T2 grid + align-self), overlay actions + touch title width (T2 rail-actions absolute), dot/-1px + time/2px + actions/bottom alignment (T2/T3 vars), 24/36px + any-pointer:coarse (T3), preserved hooks (Global Constraints + T1), no-hover-reveal (T2 always-visible), e2e/screenshots (T4). All covered.
- **Type consistency:** class/testid names match between template (T1) and CSS (T2/T3) and the preserved-hooks list. `--sz/--mk/--dot-shift/--meta-pad/--act-bottom` defined in T2 base, overridden in T3 touch — names identical.
- **Placeholder scan:** none.
