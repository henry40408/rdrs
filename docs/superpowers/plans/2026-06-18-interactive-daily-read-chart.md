# Interactive Daily Read Articles Chart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the `/statistics` "Daily Read Articles" pure-CSS bar chart into an
interactive, touch-friendly chart that always shows a numeric Y-axis and reveals
a day's exact count when a bar is tapped/clicked.

**Architecture:** Keep the server-rendered `div`-bar model. The handler computes a
"nice-max" scale + Y-axis ticks; the template renders gridlines, axis labels, and
per-bar `data-*`; a new vanilla `<rdrs-reading-chart>` custom element (light DOM,
Pointer/click events) highlights a tapped bar and positions an info card.

**Tech Stack:** Rust (Axum + Askama), vanilla ES module custom element, CSS custom
properties, Playwright BDD for E2E.

## Global Constraints

- SSR-first; no bundler/transpiler; vanilla ES module served via `include_str!`. (verbatim project rule)
- No charting library; vanilla ES modules + `include_str!` is the ceiling.
- `cargo fmt --all -- --check` and `cargo clippy -- -D warnings` must pass.
- Tests run with `cargo nextest run` (never `cargo test`); local runs use `RDRS_FAST_HASH=1`.
- Rebuild (`cargo build`) before any E2E run, because assets are embedded at compile time.
- All commits GPG-signed; stage files explicitly by name (never `git add -A`/`.`).
- Work happens on branch `feat/interactive-daily-read-chart` (already created).

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `src/handlers/pages/mod.rs` | `nice_step` / `compute_y_axis` helpers, `TickView`, handler wiring, `StatisticsTemplate.y_ticks` | Modify |
| `templates/statistics.html` | Render Y-axis + gridlines + bar `data-*` + info card, wrap in `<rdrs-reading-chart>`, load page script | Modify |
| `static/css/app.css` | Chart wrap/axis/gridline/active-bar/info-card styles | Modify |
| `static/js/components/rdrs-reading-chart.js` | Tap-to-highlight custom element | Create |
| `src/handlers/static_assets.rs` | Register the new JS asset | Modify |
| `e2e/features/statistics.feature` | BDD scenario for tap interaction | Create |
| `e2e/steps/statistics.steps.js` | Step defs for the scenario | Create |

**Note on screenshots:** The four README screenshots (`e2e/scripts/screenshots.js`)
only capture the unread list and the keyboard-help overlay — never `/statistics`.
This change therefore affects **no** README screenshot; do not regenerate them.

---

### Task 1: Y-axis scale computation (handler)

Compute a nice-max ceiling and tick list server-side, scale bars against it, and
expose `y_ticks` to the template.

**Files:**
- Modify: `src/handlers/pages/mod.rs` (add `TickView` near `DailyReadView` ~line 2135; add helpers near `format_db_bytes` ~line 2157; wire handler ~lines 2638-2663 and struct ~line 2196-2215)
- Test: `src/handlers/pages/mod.rs` `#[cfg(test)] mod tests` (~line 2785)

**Interfaces:**
- Produces:
  - `pub struct TickView { pub value: i64, pub percent: f64 }`
  - `fn nice_step(max: i64) -> i64`
  - `fn compute_y_axis(max: i64) -> (i64, Vec<TickView>)` — returns `(nice_max, ticks)`; `max` must be `> 0`.
  - `StatisticsTemplate.y_ticks: Vec<TickView>`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module (and extend its `use super::{…}` import to include
`compute_y_axis, nice_step`):

```rust
    #[test]
    fn nice_step_small_maxes_are_one() {
        assert_eq!(super::nice_step(1), 1);
        assert_eq!(super::nice_step(3), 1);
        assert_eq!(super::nice_step(4), 1);
    }

    #[test]
    fn nice_step_uses_one_two_five_progression() {
        assert_eq!(super::nice_step(8), 2);
        assert_eq!(super::nice_step(11), 5);
        assert_eq!(super::nice_step(50), 20);
    }

    #[test]
    fn compute_y_axis_single_day() {
        let (nice_max, ticks) = super::compute_y_axis(1);
        assert_eq!(nice_max, 1);
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].value, 1);
        assert!((ticks[0].percent - 100.0).abs() < 1e-9);
    }

    #[test]
    fn compute_y_axis_rounds_up_to_nice_max() {
        let (nice_max, ticks) = super::compute_y_axis(11);
        assert_eq!(nice_max, 15);
        let values: Vec<i64> = ticks.iter().map(|t| t.value).collect();
        assert_eq!(values, vec![5, 10, 15]);
        assert!((ticks[1].percent - (10.0 * 100.0 / 15.0)).abs() < 1e-9);
    }

    #[test]
    fn compute_y_axis_exact_small_range() {
        let (nice_max, ticks) = super::compute_y_axis(4);
        assert_eq!(nice_max, 4);
        let values: Vec<i64> = ticks.iter().map(|t| t.value).collect();
        assert_eq!(values, vec![1, 2, 3, 4]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -E 'test(compute_y_axis) or test(nice_step)'`
Expected: FAIL — `nice_step` / `compute_y_axis` not found.

- [ ] **Step 3: Add `TickView`**

Insert after the `DailyReadView` struct (~line 2141):

```rust
/// One Y-axis gridline + label on the daily-read chart. `percent` is the
/// distance from the chart bottom, against the nice-max scale.
pub struct TickView {
    pub value: i64,
    pub percent: f64,
}
```

- [ ] **Step 4: Add the helpers**

Insert just above `fn format_db_bytes` (~line 2157):

```rust
/// Pick a "nice" tick step (1-2-5 × 10ⁿ) targeting ~4 ticks for the given max.
fn nice_step(max: i64) -> i64 {
    if max <= 4 {
        return 1;
    }
    let raw = max as f64 / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    ((nice * mag).round() as i64).max(1)
}

/// Nice-max ceiling + Y-axis ticks for a chart whose largest value is `max`
/// (`max > 0`). Bars are scaled against the returned `nice_max` so their tops
/// align with the gridlines.
fn compute_y_axis(max: i64) -> (i64, Vec<TickView>) {
    let step = nice_step(max);
    let nice_max = ((max + step - 1) / step) * step;
    let mut ticks = Vec::new();
    let mut v = step;
    while v <= nice_max {
        ticks.push(TickView {
            value: v,
            percent: (v as f64 * 100.0) / nice_max as f64,
        });
        v += step;
    }
    (nice_max, ticks)
}
```

- [ ] **Step 5: Add the struct field**

In `StatisticsTemplate` (after `pub daily_read_counts: Vec<DailyReadView>,`, ~line 2210):

```rust
    pub y_ticks: Vec<TickView>,
```

- [ ] **Step 6: Wire the handler**

Replace the `let daily_max = …` line (~line 2638) and the `height_percent`
computation inside the `daily_read_counts` map (~lines 2651-2655):

```rust
    let daily_max = daily.iter().map(|d| d.count).max().unwrap_or(0);
    let (daily_scale_max, y_ticks) = if daily_max > 0 {
        compute_y_axis(daily_max)
    } else {
        (0, Vec::new())
    };
```

Change the `height_percent` denominator from `daily_max` to `daily_scale_max`:

```rust
            let height_percent = if daily_scale_max > 0 {
                (d.count as f64 * 100.0) / daily_scale_max as f64
            } else {
                0.0
            };
```

Add `y_ticks,` to the `StatisticsTemplate { … }` literal, right after
`daily_read_counts,` (~line 2727):

```rust
            daily_read_counts,
            y_ticks,
```

- [ ] **Step 7: Run tests + build to verify they pass**

Run: `cargo nextest run -E 'test(compute_y_axis) or test(nice_step)' && cargo build`
Expected: PASS, and the binary builds (Askama still compiles the template — `y_ticks` is rendered in Task 2, an unused field is fine for now).

- [ ] **Step 8: Format + commit**

```bash
cargo fmt
git add src/handlers/pages/mod.rs
git commit -m "feat(stats): compute Y-axis nice-max scale and ticks for daily-read chart"
```

---

### Task 2: Template + CSS rendering

Render the Y-axis labels, gridlines, per-bar `data-*`/a11y attributes, and the
info-card element; restyle bars to a low-key default with an active state.

**Files:**
- Modify: `templates/statistics.html:52-66` (the `Daily Read Articles` section)
- Modify: `static/css/app.css:2388-2426` (chart styles)

**Interfaces:**
- Consumes: `StatisticsTemplate.y_ticks` (Task 1), `daily_read_counts[].short_label`, `.count`, `.date`, `.height_percent`.
- Produces (for Task 3's JS): DOM `rdrs-reading-chart` containing `.stats-bar-col[data-date][data-count]` and a `.stats-chart-card[data-chart-card]`.

- [ ] **Step 1: Replace the chart markup**

Replace `templates/statistics.html` lines 52-66 (the whole `Daily Read Articles`
`stats-section`) with:

```html
                <div class="stats-section">
                    <h2>Daily Read Articles</h2>
                    {% if daily_max == 0 %}
                        <p class="muted">No read activity in this period</p>
                    {% else %}
                        <rdrs-reading-chart class="stats-chart-wrap">
                            <div class="stats-y-axis" aria-hidden="true">
                                {% for t in y_ticks %}
                                    <span class="stats-y-tick" style="bottom: {{ t.percent }}%">{{ t.value }}</span>
                                {% endfor %}
                            </div>
                            <div class="stats-chart">
                                {% for t in y_ticks %}
                                    <div class="stats-gridline" style="bottom: {{ t.percent }}%" aria-hidden="true"></div>
                                {% endfor %}
                                {% for d in daily_read_counts %}
                                    <div class="stats-bar-col" data-date="{{ d.short_label }}" data-count="{{ d.count }}"
                                         title="{{ d.date }}: {{ d.count }}" tabindex="0" role="button"
                                         aria-label="{{ d.date }}: {{ d.count }} read">
                                        <div class="stats-bar" style="height: {{ d.height_percent }}%"></div>
                                        <div class="stats-bar-label">{{ d.short_label }}</div>
                                    </div>
                                {% endfor %}
                            </div>
                            <div class="stats-chart-card" data-chart-card hidden></div>
                        </rdrs-reading-chart>
                    {% endif %}
                </div>
```

- [ ] **Step 2: Replace the chart CSS**

Replace `static/css/app.css` lines 2388-2426 (`.stats-chart` through
`.stats-bar-label`) with:

```css
.stats-chart-wrap {
    position: relative;
    display: flex;
    align-items: stretch;
    gap: var(--space-2);
    margin-bottom: var(--space-6);
    touch-action: manipulation;
}
.stats-y-axis {
    position: relative;
    width: 1.75rem;
    height: 160px;
    flex-shrink: 0;
}
.stats-y-tick {
    position: absolute;
    right: 0;
    transform: translateY(50%);
    font-family: var(--font-ui);
    font-size: 10px;
    color: var(--color-text-muted);
    line-height: 1;
}
.stats-chart {
    flex: 1;
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 160px;
    margin-bottom: var(--space-6);
    position: relative;
    min-width: 0;
}
.stats-gridline {
    position: absolute;
    left: 0;
    right: 0;
    height: 1px;
    background: var(--color-border-light);
    z-index: 0;
}
.stats-bar-col {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-end;
    height: 100%;
    min-width: 0;
    position: relative;
    z-index: 1;
    cursor: pointer;
}
.stats-bar-col:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 2px;
}
.stats-bar {
    width: 100%;
    background: var(--color-accent-subtle);
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    min-height: 2px;
    transition: height 0.2s, background 0.15s;
}
.stats-bar-col.is-active .stats-bar,
.stats-bar-col:hover .stats-bar {
    background: var(--color-accent);
}
.stats-bar-label {
    font-family: var(--font-ui);
    font-size: 10px;
    color: var(--color-text-muted);
    margin-top: var(--space-1);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    text-align: center;
}
.stats-chart-card {
    position: absolute;
    top: 0;
    transform: translate(-50%, -100%);
    background: var(--color-text);
    color: var(--color-bg);
    font-family: var(--font-ui);
    font-size: 11px;
    line-height: 1;
    padding: 0.3rem 0.5rem;
    border-radius: var(--radius-sm);
    white-space: nowrap;
    pointer-events: none;
    z-index: 2;
}
.stats-chart-card[hidden] {
    display: none;
}
```

- [ ] **Step 3: Build to verify the template compiles**

Run: `cargo build`
Expected: PASS — Askama compiles the new `y_ticks` loop and `daily_read_counts` access; no errors.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add templates/statistics.html static/css/app.css
git commit -m "feat(stats): render Y-axis, gridlines, and tap target markup for daily-read chart"
```

---

### Task 3: `<rdrs-reading-chart>` custom element + registration

Add the vanilla custom element that highlights a tapped bar and positions the
info card, register it as a served asset, and load it only on `/statistics`.

**Files:**
- Create: `static/js/components/rdrs-reading-chart.js`
- Modify: `src/handlers/static_assets.rs:8-25` (the `FILES` array)
- Modify: `templates/statistics.html` (add a `{% block page_script %}` at end of the `page` block content)

**Interfaces:**
- Consumes: DOM from Task 2 (`.stats-bar-col[data-date][data-count]`, `.stats-chart-card[data-chart-card]`).
- Produces: registered element `rdrs-reading-chart`; served at `/static/js/components/rdrs-reading-chart.js`.

- [ ] **Step 1: Create the custom element**

Create `static/js/components/rdrs-reading-chart.js`:

```js
// <rdrs-reading-chart> — tap/click a bar to highlight it and show an info card.
// Light DOM: enhances the server-rendered bars on /statistics. Pointer/click
// events cover mouse, touch, and pen with one code path; bars are focusable so
// Enter/Space work too. With JS disabled the static chart + native title remain.

class RdrsReadingChart extends HTMLElement {
    connectedCallback() {
        this.card = this.querySelector('[data-chart-card]');
        this.cols = Array.from(this.querySelectorAll('.stats-bar-col'));
        this._active = null;

        this.cols.forEach((col) => {
            col.addEventListener('click', () => this._toggle(col));
            col.addEventListener('keydown', (e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    this._toggle(col);
                }
            });
        });

        // Tap the chart's empty area to dismiss.
        this.addEventListener('click', (e) => {
            if (e.target === this) this._clear();
        });
        this.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') this._clear();
        });
    }

    _toggle(col) {
        if (this._active === col) {
            this._clear();
        } else {
            this._show(col);
        }
    }

    _show(col) {
        if (this._active) this._active.classList.remove('is-active');
        this._active = col;
        col.classList.add('is-active');

        this.card.textContent = `${col.dataset.date} · ${col.dataset.count}`;
        const wrapRect = this.getBoundingClientRect();
        const colRect = col.getBoundingClientRect();
        const left = colRect.left - wrapRect.left + colRect.width / 2;
        this.card.style.left = `${left}px`;
        this.card.hidden = false;
    }

    _clear() {
        if (this._active) this._active.classList.remove('is-active');
        this._active = null;
        this.card.hidden = true;
    }
}

customElements.define('rdrs-reading-chart', RdrsReadingChart);
```

- [ ] **Step 2: Register the asset**

In `src/handlers/static_assets.rs`, add to the `FILES` array (after the
`rdrs-sidebar.js` entry, ~line 24):

```rust
    (
        "js/components/rdrs-reading-chart.js",
        include_str!("../../static/js/components/rdrs-reading-chart.js"),
    ),
```

- [ ] **Step 3: Load the script on the statistics page**

In `templates/statistics.html`, add a `page_script` block immediately before the
final `{% endblock %}` (the one closing `{% block page %}`, line 164):

```html
    {% block page_script %}
    <link rel="modulepreload" href="/static/js/components/rdrs-reading-chart.js?v={{ layout.git_version }}">
    <script type="module" src="/static/js/components/rdrs-reading-chart.js?v={{ layout.git_version }}"></script>
    {% endblock %}
```

(The base `app_layout.html` already declares `{% block page_script %}{% endblock %}` at line 14.)

- [ ] **Step 4: Build to verify everything compiles + embeds**

Run: `cargo build`
Expected: PASS — `include_str!` finds the new file; template compiles.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add static/js/components/rdrs-reading-chart.js src/handlers/static_assets.rs templates/statistics.html
git commit -m "feat(stats): add rdrs-reading-chart element for tap-to-inspect bars"
```

---

### Task 4: E2E scenario (tap a bar shows its count)

Seed read entries across two days, open `/statistics`, tap a bar, and assert the
info card shows the count.

**Files:**
- Create: `e2e/features/statistics.feature`
- Create: `e2e/steps/statistics.steps.js`

**Interfaces:**
- Consumes: existing fixtures `page`, `api`, `serverUrl`, `currentUser`, `seed`; `seed.getUserId`, `seed.createCategory`, `seed.createFeed`, `seed.insertEntries`, `seed.markRead`.
- Produces: a passing tagged scenario covering the tap interaction.

- [ ] **Step 1: Write the feature**

Create `e2e/features/statistics.feature`:

```gherkin
Feature: Daily Read Articles chart

  Background:
    Given I am signed in
    And I have read articles over several days

  Scenario: Tapping a bar reveals that day's count
    When I open the statistics page
    And I tap the tallest read-activity bar
    Then the chart info card shows a read count
```

- [ ] **Step 2: Write the step definitions**

Create `e2e/steps/statistics.steps.js`. Reuse the existing `I am signed in` and
`I open the statistics page` steps if present; the two new steps are below.
(Confirm the signed-in/open steps already exist in `auth.steps.js` /
`admin.steps.js`; if `I am signed in` is not already defined, use the inline sign-in
shown here.)

```js
import { createBdd } from "playwright-bdd";
import { test, expect } from "../support/fixtures.js";

const { Given, When, Then } = createBdd(test);

Given("I have read articles over several days", async ({ api, currentUser, seed }) => {
  await api.register(currentUser.username, currentUser.password);
  const userId = seed.getUserId(currentUser.username);
  const categoryId = seed.createCategory(userId, "News");
  const feedId = seed.createFeed(categoryId, "https://example.com/feed.xml", "Example");
  const ids = seed.insertEntries(
    Array.from({ length: 5 }, (_, i) => ({
      feedId,
      guid: `stats-${i}`,
      title: `Stats Entry ${i}`,
      link: `https://example.com/e/${i}`,
      content: "<p>x</p>",
    }))
  );
  // Three reads today, one yesterday — today's bar is the tallest.
  seed.markRead(ids[0], "0 seconds");
  seed.markRead(ids[1], "-1 hours");
  seed.markRead(ids[2], "-2 hours");
  seed.markRead(ids[3], "-1 days");
});

When("I tap the tallest read-activity bar", async ({ page }) => {
  // The last column is "today" (chart is ordered oldest → newest), which has the
  // most reads in the seed above.
  const bars = page.locator("rdrs-reading-chart .stats-bar-col");
  await bars.last().click();
});

Then("the chart info card shows a read count", async ({ page }) => {
  const card = page.locator("rdrs-reading-chart .stats-chart-card");
  await expect(card).toBeVisible();
  // Format is "MM/DD · N"; assert it ends with the count 3.
  await expect(card).toContainText(/·\s*3$/);
});
```

If `I am signed in` is **not** already a shared step, prepend this to the file:

```js
Given("I am signed in", async ({ page, api, currentUser, serverUrl }) => {
  await page.goto(`${serverUrl}/login`);
  await page.getByTestId("username-input").fill(currentUser.username);
  await page.getByTestId("password-input").fill(currentUser.password);
  await page.getByTestId("login-submit").click();
  await page.waitForURL(`${serverUrl}/`);
});
```

(Note: `Given("I have read articles over several days")` already calls
`api.register`; if a shared `I am signed in` also registers, drop the register call
from one of them to avoid a duplicate-username error. Verify against the existing
`auth.steps.js` during implementation and keep exactly one `api.register`.)

- [ ] **Step 3: Rebuild the binary so E2E sees the new assets**

Run: `cargo build`
Expected: PASS (E2E global-setup skips building if a binary exists, so build first).

- [ ] **Step 4: Regenerate BDD specs and run the scenario**

Run (from `e2e/`): `npx bddgen && npx playwright test --grep "Daily Read Articles chart"`
Expected: PASS — the info card becomes visible and contains the count `3`.

- [ ] **Step 5: Commit**

```bash
git add e2e/features/statistics.feature e2e/steps/statistics.steps.js
git commit -m "test(e2e): cover tap-to-inspect on the daily-read chart"
```

---

### Task 5: Full verification

Confirm the whole suite, lints, and format are green before handing off.

**Files:** none (verification only).

- [ ] **Step 1: Format check + clippy**

Run: `cargo fmt --all -- --check && cargo clippy -- -D warnings`
Expected: PASS — no diff, no warnings.

- [ ] **Step 2: Full Rust test suite**

Run: `RDRS_FAST_HASH=1 cargo nextest run`
Expected: PASS — including the Task 1 `nice_step`/`compute_y_axis` tests.

- [ ] **Step 3: Confirm no README screenshot is affected**

Run: `rg -n "/statistics" e2e/scripts/screenshots.js`
Expected: NO MATCH — confirming the four README screenshots don't include the chart, so none need regeneration.

- [ ] **Step 4: Manual sanity (optional, recommended)**

Build + run the app, sign in to a user with read history, open `/statistics`,
and verify: Y-axis numbers show, tapping a bar highlights it + shows the card,
tapping empty space / `Esc` dismisses, and it works under a touch emulation.

---

## Self-Review

**Spec coverage:**
- Always-visible numeric Y-axis → Task 1 (ticks) + Task 2 (axis/gridline render). ✓
- Tap a bar → highlight + info card → Task 2 (markup/active style) + Task 3 (JS). ✓
- Touch support (Pointer/click + `touch-action: manipulation`) → Task 2 CSS + Task 3 JS. ✓
- No build tooling / vanilla ES module via `include_str!` → Task 3 (asset registration). ✓
- Info card = date + count only → Task 3 `_show` (`MM/DD · N`). ✓
- No-JS fallback (native `title`) → Task 2 markup keeps `title`. ✓
- Unit tests for nice-max → Task 1. ✓
- E2E tap test → Task 4. ✓
- Screenshots: confirmed not affected → Task 5 Step 3. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✓

**Type consistency:** `compute_y_axis` returns `(i64, Vec<TickView>)`; `TickView { value, percent }` used identically in template (`t.value`, `t.percent`). `data-date`/`data-count` set in Task 2 and read in Task 3 match. ✓
