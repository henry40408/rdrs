# Entry Item Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the entry-list row as one editorial CSS-grid layout (Option C v5) that works on desktop and mobile, with an always-present feed favicon, darker secondary text, and touch-sized actions.

**Architecture:** A single row template (`_entry_row.html`) renders a CSS grid (`"fav head" / "fav meta" / "foot foot"`). The favicon falls back to a deterministic letter chip via two new `EntryRowView` methods. All styling lives in `static/css/app.css`: the desktop `.entry-item*` block is replaced and the mobile `@media (max-width:1024px)` block gets a dedicated entry-row section, while the entry-row selectors are stripped from the generic touch baseline. Existing `data-testid`s and POST endpoints are preserved so triage/responsive e2e keep passing.

**Tech Stack:** Rust (Askama view-model), Askama HTML template, CSS, Playwright-BDD e2e.

---

## Notes for the implementer

- **Environment (NixOS):** `source /tmp/rdrs-env.sh` in the SAME compound command before any `cargo`/e2e command (OpenSSL env).
- **CSS is embedded in the binary via `include_str!`** — after editing CSS or Rust you MUST `cargo build` before running e2e, or stale CSS is served. The e2e harness does not rebuild on its own.
- **e2e runs from `e2e/`.** After adding/removing `.feature` scenarios run `npx bddgen` (or just `npx playwright test`, which triggers it). `pwd` first.
- **Rust:** run tests with `cargo nextest run`; run `cargo fmt` before committing.
- **Commits:** GPG-sign (`git commit -S`); end the message with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer. Stage files explicitly — never `git add -A`/`.`.
- The breakpoint convention: **>1024px = desktop** (base rules), **≤1024px = mobile** (the existing `@media (max-width: 1024px)` block).
- `_entry_row.html` is included from `_entries_fragment.html`, `_entries_layout.html`, `_entry_actions_multi.html`, `_open_entry_multi.html` — one template change covers them all.

---

## Task 1: Favicon fallback view-model methods

**Files:**
- Modify: `src/handlers/pages/mod.rs` (impl `EntryRowView`, after `summary_status_str` ~line 54; tests in `mod tests` ~line 2688)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/handlers/pages/mod.rs` (after the existing tests). Note `row_view_from(&ewf, None)` builds an `EntryRowView`; `ewf_with_title` sets `feed_id = 1`, `feed_title = "Feed"`.

```rust
    #[test]
    fn feed_initial_uppercases_first_char() {
        let ewf = ewf_with_title("anything");
        let row = row_view_from(&ewf, None); // feed_title = "Feed"
        assert_eq!(row.feed_initial(), "F");
    }

    #[test]
    fn feed_initial_handles_empty_title() {
        let mut ewf = ewf_with_title("anything");
        ewf.feed_title = Some(String::new());
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_initial(), "?");
    }

    #[test]
    fn feed_color_index_is_stable_and_bounded() {
        let mut ewf = ewf_with_title("anything");
        ewf.entry.feed_id = 13;
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_color_index(), 1); // 13 % 6 == 1
        assert!(row.feed_color_index() < 6);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo nextest run -E 'test(feed_initial) + test(feed_color_index)' 2>&1 | tail -20
```
Expected: FAIL — `no method named feed_initial`/`feed_color_index`.

- [ ] **Step 3: Implement the methods**

In `src/handlers/pages/mod.rs`, inside `impl EntryRowView`, after `summary_status_str`:

```rust
    /// Uppercased first character of the feed title, for the favicon
    /// letter-chip fallback shown when a feed has no icon. Returns "?" for
    /// an empty title.
    pub fn feed_initial(&self) -> String {
        self.feed_title
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    /// Stable index 0..6 into the favicon fallback colour palette, derived
    /// from the feed id so the same feed always gets the same colour.
    pub fn feed_color_index(&self) -> u8 {
        self.feed_id.rem_euclid(6) as u8
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo nextest run -E 'test(feed_initial) + test(feed_color_index)' 2>&1 | tail -20
```
Expected: 3 passed.

- [ ] **Step 5: Format and commit**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo fmt
git add src/handlers/pages/mod.rs
git commit -S -m "feat(entries): favicon fallback helpers on EntryRowView

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: e2e assertions for the redesigned row (failing)

TDD red: assert the new structure (always-present favicon, full-width mobile action buttons, 24px feed/category) that the current DOM lacks.

**Files:**
- Modify: `e2e/steps/responsive.steps.js` (one new step)
- Modify: `e2e/features/responsive.feature` (one new `@mobile` scenario)

- [ ] **Step 1: Add a step that checks an element spans (nearly) the full row width**

Append to `e2e/steps/responsive.steps.js`:

```javascript
Then("the {string} controls each span at least {int}% of the row", async ({ page }, selector, pct) => {
  const row = await page.getByTestId("entry-item").first().boundingBox();
  const btns = await page.locator(selector).all();
  expect(btns.length).toBeGreaterThanOrEqual(2);
  for (const b of btns) {
    const box = await b.boundingBox();
    expect(box).not.toBeNull();
    expect(box.width).toBeGreaterThanOrEqual((row.width * pct) / 100);
  }
});
```

- [ ] **Step 2: Add the failing scenario**

Append to `e2e/features/responsive.feature`:

```gherkin
  @mobile
  Scenario: Entry row redesign — favicon, full-width actions, sized meta on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the ".entry-favicon" control is at least 24px wide
    And the ".entry-favicon" control is at least 24px tall
    And the ".entry-item-meta a" control is at least 24px tall
    And the ".entry-item-actions .entry-action-btn" controls each span at least 25% of the row
    And the ".entry-action-btn" control is at least 44px tall
```

- [ ] **Step 3: Run to verify it FAILS**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -1 && cd e2e && npx bddgen && npx playwright test --grep "Entry row redesign" 2>&1 | tail -15
```
Expected: FAIL — current rows have no `.entry-favicon` (seeded feed has no icon) and the action buttons are not full-width thirds.

- [ ] **Step 4: Commit the failing test**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/steps/responsive.steps.js e2e/features/responsive.feature
git commit -S -m "test(e2e): assert entry-row redesign layout (failing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Redesign the row template + CSS

Makes Task 2 green and preserves all existing triage/responsive selectors.

**Files:**
- Modify: `templates/_entry_row.html` (full rewrite)
- Modify: `static/css/app.css` (replace `.entry-item*` desktop block; edit the `@media (max-width:1024px)` block)

- [ ] **Step 1: Rewrite `templates/_entry_row.html`**

Replace the entire file with:

```html
{# Single entry row — editorial CSS-grid layout (see
   docs/superpowers/specs/2026-06-13-entry-item-redesign-design.md).
   Grid areas: "fav head" / "fav meta" / "foot foot". The form-wrapped
   action buttons keep their data-testids and POST targets so triage and
   responsive e2e keep working; on mobile the .action-label spans are
   hidden (CSS) leaving icon-only 44px buttons. #}
<div id="entry-row-{{ r.id }}" class="entry-item{% if r.is_read %} entry-read{% endif %}" data-entry-row data-entry-id="{{ r.id }}" data-testid="entry-item">
    {% if r.feed_has_icon %}
    <img class="entry-favicon" src="/api/feeds/{{ r.feed_id }}/icon" alt="" loading="lazy" width="24" height="24">
    {% else %}
    <span class="entry-favicon entry-favicon-chip fav-c{{ r.feed_color_index() }}" aria-hidden="true">{{ r.feed_initial() }}</span>
    {% endif %}

    <div class="entry-head">
        <a href="/entries/{{ r.id }}/fragment" data-swap="#reading-pane" class="entry-item-title {% if r.is_read %}entry-title-normal{% else %}entry-title-bold{% endif %}" data-testid="entry-title-link">{{ r.title }}</a>
        <span class="entry-item-badges">{% match r.summary_status_str() %}
            {% when Some("completed") %}<span title="Has Summary" class="summary-badge">✅</span>
            {% when Some("pending") %}<span title="Pending" class="summary-badge-pending">⏳</span>
            {% when Some("processing") %}<span title="Processing" class="summary-badge-processing">🔄</span>
            {% when Some("failed") %}<span title="Failed" class="summary-badge-failed">❌</span>
            {% when _ %}
        {% endmatch %}</span>
        <time class="entry-time" datetime="{{ r.published_at_iso }}">{{ r.published_relative }}</time>
    </div>

    <div class="muted entry-item-meta">
        <a href="/feeds/{{ r.feed_id }}/entries">{{ r.feed_title }}</a>
        <span class="meta-sep">·</span>
        <a href="/categories/{{ r.category_id }}/entries">{{ r.category_name }}</a>
    </div>

    <div class="entry-item-actions">
        <form method="post" action="/entries/{{ r.id }}/{% if r.is_read %}unread{% else %}read{% endif %}" data-swap="#entry-row-{{ r.id }}">
            <button type="submit" class="entry-action-btn" data-testid="entry-read-action" aria-label="{% if r.is_read %}Mark unread{% else %}Mark read{% endif %}"><span class="action-icon" aria-hidden="true">{% if r.is_read %}↺{% else %}✓{% endif %}</span><span class="action-label">{% if r.is_read %}unread{% else %}read{% endif %}</span></button>
        </form>
        <form method="post" action="/entries/{{ r.id }}/{% if r.is_starred %}unstar{% else %}star{% endif %}" data-swap="#entry-row-{{ r.id }}">
            <button type="submit" class="entry-action-btn" data-testid="entry-star-action" aria-label="{% if r.is_starred %}Unstar{% else %}Star{% endif %}"><span class="action-icon" aria-hidden="true">{% if r.is_starred %}★{% else %}☆{% endif %}</span><span class="action-label">{% if r.is_starred %}starred{% else %}star{% endif %}</span></button>
        </form>
        {% if let Some(link) = r.link.as_ref() %}<a href="{{ link }}" target="_blank" rel="noopener noreferrer" data-testid="entry-original-link" aria-label="Open original"><span class="action-icon" aria-hidden="true">↗</span><span class="action-label">original</span></a>{% endif %}
    </div>
</div>
```

- [ ] **Step 2: Replace the desktop `.entry-item*` block in `static/css/app.css`**

Find the block starting at `/* ===== Entry Items (List) ===== */` (the `.entry-item { padding: var(--space-4) var(--space-5); ... }` rule) and ending after `.entry-item-meta { ... }` and the `.entry-item-actions` + `.entry-item-meta a/.entry-item-actions a/.breadcrumb a` colour rules (the contiguous `.entry-item*` section, roughly the rule for `.entry-item` through the `.entry-item-meta a:hover, .entry-item-actions a:hover, .breadcrumb a:hover` rule). Replace that whole section with:

```css
/* ===== Entry Items (List) — editorial grid ===== */
.entry-item {
    display: grid;
    grid-template-columns: auto 1fr;
    grid-template-areas:
        "fav  head"
        "fav  meta"
        "foot foot";
    column-gap: var(--space-3);
    padding: var(--space-4) var(--space-5);
    border-bottom: 1px solid var(--color-border-light);
    cursor: pointer;
    transition: background 0.1s;
}

.entry-item:hover { background: var(--color-bg-secondary); }

.entry-item.selected {
    background: var(--color-accent-subtle);
    box-shadow: inset var(--border-accent-width) 0 0 var(--color-accent);
}

.entry-item.entry-read { opacity: 0.62; }
.entry-item.entry-read:hover,
.entry-item.entry-read.selected { opacity: 1; }

/* Favicon spans the title+meta rows only (no tall empty column). */
.entry-favicon {
    grid-area: fav;
    width: 24px;
    height: 24px;
    margin-top: 2px;
    border-radius: var(--radius-control);
    flex: none;
    object-fit: cover;
}
.entry-favicon-chip {
    display: flex;
    align-items: center;
    justify-content: center;
    color: #fff;
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    font-weight: 700;
}
.fav-c0 { background: #7C6F9B; }
.fav-c1 { background: #B05B3B; }
.fav-c2 { background: #3B7A6B; }
.fav-c3 { background: #4A6FA5; }
.fav-c4 { background: #A6563E; }
.fav-c5 { background: #6E8B3D; }

/* Head: serif title + summary badges + right-aligned time. */
.entry-head {
    grid-area: head;
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
}
.entry-item-title {
    font-family: var(--font-display);
    font-size: var(--font-base);
    font-weight: 600;
    line-height: 1.32;
    color: var(--color-text);
    cursor: pointer;
    overflow-wrap: break-word;
    word-break: break-word;
}
.entry-item-title:hover { color: var(--color-accent); }
.entry-title-bold { font-weight: 600; }
.entry-title-normal { font-weight: 400; }
.entry-item-badges { flex: none; }
.entry-time {
    margin-left: auto;
    flex: none;
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
}

/* Meta: feed · category, flush-left with the title. */
.entry-item-meta {
    grid-area: meta;
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin-top: var(--space-1);
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    color: var(--color-text-secondary);
}
.entry-item-meta a { color: var(--color-text-secondary); font-weight: 500; }
.entry-item-meta a:hover { color: var(--color-accent); }
.entry-item-meta .meta-sep { color: var(--color-text-muted); }

/* Actions: full-width strip. Desktop = quiet text links that brighten on
   row hover; mobile (below) = full-width icon buttons. */
.entry-item-actions {
    grid-area: foot;
    display: flex;
    gap: var(--space-5);
    margin-top: var(--space-2);
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    opacity: 0.72;
    transition: opacity 0.1s;
}
.entry-item:hover .entry-item-actions,
.entry-item.selected .entry-item-actions { opacity: 1; }
.entry-action-btn,
.entry-item-actions a {
    display: inline-flex;
    align-items: center;
    gap: 0.35em;
    padding: 0;
    border: none;
    background: none;
    color: var(--color-text-secondary);
    font: inherit;
    cursor: pointer;
}
.entry-action-btn:hover,
.entry-item-actions a:hover { color: var(--color-accent); }
```

- [ ] **Step 3: Strip entry-row selectors from the generic mobile baseline**

In the `@media (max-width: 1024px)` block of `static/css/app.css`, make these edits (the entry row now styles these itself):

1. In the inline-flex touch group (the rule whose selectors include `.action-link, .actions a, .tab-bar a, …`), **delete** these three selector lines: `.entry-action-btn,`, `.entry-item-actions a,`, and `.entry-item-meta a,`.

2. **Delete** the entire rule:
```css
    .entry-item .entry-item-actions .entry-action-btn,
    .entry-item .entry-item-actions a {
        padding: var(--space-1) var(--space-2);
    }
```
(including its preceding comment block about padding:0 / specificity).

3. In the font-size rule `.entry-item-meta, .entry-item-actions, .action-link { font-size: var(--font-sm); }`, **delete** `.entry-item-meta,` and `.entry-item-actions,` so only `.action-link { font-size: var(--font-sm); }` remains.

4. **Delete** the entire rule (comment + selector):
```css
    /* Anti-mis-tap: widen gaps ... */
    .entry-item-actions {
        gap: var(--space-4);
        align-items: center;
    }
```

- [ ] **Step 4: Add the mobile entry-row section**

Still inside the `@media (max-width: 1024px)` block, locate the existing rule `.entry-item { padding: var(--space-4); }` and replace it with the consolidated mobile entry-row rules:

```css
    /* ===== Entry row (redesign) — mobile ===== */
    .entry-item { padding: var(--space-4); }

    /* Feed/category: 24px tap area (AA floor; inline links are exempt from
       the 44px rule), no horizontal padding so they stay flush with title. */
    .entry-item-meta { margin-top: var(--space-2); }
    .entry-item-meta a {
        display: inline-flex;
        align-items: center;
        min-height: 24px;
    }

    /* Actions: full-width equal thirds, 44px tall, big icon, small padding,
       always visible, icon-only (labels hidden). */
    .entry-item-actions {
        gap: var(--space-2);
        margin-top: var(--space-3);
        opacity: 1;
    }
    .entry-item-actions .action-label { display: none; }
    .entry-action-btn,
    .entry-item-actions a {
        flex: 1;
        justify-content: center;
        gap: 0;
        min-height: var(--touch-min);
        padding: var(--space-2);
        border: 1px solid var(--color-border-light);
        border-radius: var(--radius-md);
        font-size: var(--font-xl);
    }
```

- [ ] **Step 5: Rebuild and run the redesign + regression e2e**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -1 && cd e2e && npx bddgen && npx playwright test --grep @mobile 2>&1 | tail -6
```
Expected: all `@mobile` scenarios PASS, including "Entry row redesign…".

- [ ] **Step 6: Run triage + entries scenarios (depend on row testids)**

```bash
cd /home/nixos/Develop/claude/rdrs/e2e && source /tmp/rdrs-env.sh && npx playwright test triage entries 2>&1 | tail -6
```
Expected: PASS (read/star/original actions and title-link still work via preserved testids).

- [ ] **Step 7: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add templates/_entry_row.html static/css/app.css
git commit -S -m "feat(ui): editorial entry-row redesign (desktop + mobile)

Grid layout (fav/head/meta/foot) with always-present favicon (letter-chip
fallback), serif title, darker meta, and a full-width action strip:
quiet hover text links on desktop, 44px icon buttons on mobile. Keeps all
entry-row data-testids and POST targets.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Full regression sweep

**Files:** none (verification only)

- [ ] **Step 1: Rust tests**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -8
```
Expected: all pass.

- [ ] **Step 2: Full e2e suite**

```bash
cd /home/nixos/Develop/claude/rdrs && source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -1 && cd e2e && npx bddgen && npx playwright test 2>&1 | tail -4
```
Expected: all pass (responsive + triage + entries + keyboard + search + the rest).

- [ ] **Step 3: If anything failed, fix and re-run.** Do not leave the suite red.

---

## Self-Review

**Spec coverage:**
- Grid layout `"fav head"/"fav meta"/"foot foot"` → Task 3 Step 2. ✅
- Favicon always present + letter-chip fallback (6 colors, `feed_id % 6`) → Task 1 (methods) + Task 3 Steps 1-2. ✅
- Serif title + right time; meta = feed·category darker, flush-left → Task 3 Step 2/4. ✅
- Actions: desktop hover text links (icon+label), mobile full-width 44px icon-only with aria-label → Task 3 Steps 1-2-4. ✅
- Feed/category 24px no h-padding → Task 3 Step 4. ✅
- Read opacity 0.62, selected gold bar, unread bold → Task 3 Step 2. ✅
- Color darkening (meta → text-secondary) → Task 3 Step 2. ✅
- Preserve data-testids / POST endpoints → Task 3 Step 1 (verified Task 2 Step? triage in Task 3 Step 6). ✅
- Supersede entry-row baseline rules, keep the rest → Task 3 Step 3. ✅
- e2e for new layout → Task 2; regression → Task 4. ✅

**Placeholder scan:** none — all template/CSS/Rust/test code is concrete.

**Consistency:** class names match between template (Task 3 Step 1) and CSS (Steps 2/4): `.entry-favicon`, `.entry-favicon-chip`, `.fav-c{0..5}`, `.entry-head`, `.entry-item-title`, `.entry-item-badges`, `.entry-time`, `.entry-item-meta`, `.meta-sep`, `.entry-item-actions`, `.entry-action-btn`, `.action-icon`, `.action-label`. Method names `feed_initial()` / `feed_color_index()` match Task 1 and the template. e2e selectors in Task 2 (`.entry-favicon`, `.entry-item-meta a`, `.entry-item-actions .entry-action-btn`) exist in the new DOM.
