# Reading Pane Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the reading-pane chrome (toolbar, header/meta, action controls, summary-box controls) for desktop and mobile — mobile gets a fixed bottom action bar, desktop a same-level inline action row — reusing the entry-row visual language (favicon chip, icon + label controls, generous tap targets). Article body typography is untouched.

**Architecture:** Presentation-layer change. A small Rust addition gives `ReadingPaneView` the feed identity for the favicon chip (with the `feed_initial` / `feed_color_index` logic extracted to shared free functions so `EntryRowView` and `ReadingPaneView` both delegate). The template swaps the per-button `.btn-secondary .btn-sm` cluster for a shared `.rp-action` control (icon + label spans, stable `aria-label`s), adds the favicon chip to the meta line, and makes the prev/next nav icon-only on mobile. CSS restyles the actions: a fixed bottom bar inside the existing `@media (max-width: 1024px)` block on mobile, a single nowrap inline row on desktop. No new JS.

**Tech Stack:** Rust (Axum + Askama, `src/handlers/`), Askama templates (`templates/`), CSS (`static/css/app.css`), Playwright-BDD e2e (`e2e/`).

---

## Notes for the implementer

- **Environment:** This is a NixOS box; re-source the OpenSSL env before **every**
  cargo/e2e command: `source /tmp/rdrs-env.sh`.
- **`pwd` first** for build/test/git (multi-project workspace rule); expect
  `/home/nixos/Develop/claude/rdrs`.
- **Run Rust tests with `cargo nextest run`** (not `cargo test`), and set
  `RDRS_FAST_HASH=1` to use minimal Argon2 cost (much faster).
- **Run `cargo fmt` before committing Rust.**
- **e2e runs from `e2e/`** and **requires `cargo build` first** — the CSS is
  `include_str!`'d into the binary and the e2e global-setup skips rebuild if the
  binary exists. After any CSS/Rust/template change: `cargo build`, then e2e.
  After `.feature` edits run `npx bddgen` (the test command does this for you,
  but if you run a single generated spec directly, regenerate first).
- **Commits are GPG-signed** (`git commit -S`). End each message with the
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer.
- **Stage files explicitly by name** — never `git add -A` / `git add .`.
- Current branch is `feat/reading-pane-redesign` (already created).

---

## File Structure

- `src/handlers/pages/mod.rs` — extract `feed_initial(&str)` / `feed_color_index(i64)`
  free functions; `EntryRowView` methods delegate to them; add `feed_id` /
  `feed_has_icon` fields + delegating methods to `ReadingPaneView`; unit tests.
- `src/handlers/entries.rs` — populate the two new `ReadingPaneView` fields in
  the single constructor `build_reading_pane_view` (`entries.rs:213`).
- `templates/_reading_pane.html` — favicon chip in meta; `.rp-action` controls
  with icon/label spans + `aria-label`s; nav icon-only + `.nav-label`;
  summary-box Copy/Dismiss restyle.
- `static/css/app.css` — title size; meta favicon override + favicon-safe
  separator rule; `.rp-action` base + desktop row; `.nav-label`; mobile fixed
  bottom bar + content padding-bottom + icon-only nav (inside the existing
  `@media (max-width: 1024px)` block, which ends at the lone `}` after the
  mobile rules).
- `tests/handlers_test.rs` — extend `test_entry_fragment_renders_reading_pane`
  with favicon-chip + `.rp-action`/`aria-label` assertions.
- `e2e/features/reading.feature` — new `@mobile` bottom-bar scenario.
- `e2e/steps/responsive.steps.js` — already has the generic
  `the "{selector}" control is at least {int}px tall/wide` steps (reuse them).

---

## Task 1: Rust — shared favicon helpers + `ReadingPaneView` feed identity

Extract the favicon-chip logic into free functions, have both view-models
delegate, and give `ReadingPaneView` the `feed_id` / `feed_has_icon` it needs.

**Files:**
- Modify: `src/handlers/pages/mod.rs` (`EntryRowView` impl ~48-72; `ReadingPaneView` struct ~80-101; tests module ~2826)
- Modify: `src/handlers/entries.rs:213` (constructor)

- [ ] **Step 1: Write failing unit tests for the free functions**

Append inside the `mod tests { … }` block in `src/handlers/pages/mod.rs`
(near the existing `feed_initial_*` / `feed_color_index_*` tests, ~line 2861):

```rust
    #[test]
    fn feed_initial_fn_uppercases_first_char() {
        assert_eq!(super::feed_initial("daring fireball"), "D");
    }

    #[test]
    fn feed_initial_fn_handles_empty() {
        assert_eq!(super::feed_initial(""), "?");
    }

    #[test]
    fn feed_initial_fn_uppercases_unicode() {
        assert_eq!(super::feed_initial("über"), "Ü");
    }

    #[test]
    fn feed_color_index_fn_is_bounded() {
        assert_eq!(super::feed_color_index(13), 1); // 13 % 6 == 1
        assert_eq!(super::feed_color_index(-1), 5); // (-1).rem_euclid(6) == 5
        assert!(super::feed_color_index(123_456) < 6);
    }
```

- [ ] **Step 2: Run the tests to verify they FAIL (functions undefined)**

```bash
pwd   # /home/nixos/Develop/claude/rdrs
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs feed_initial_fn 2>&1 | tail -20
```

Expected: compile error — `super::feed_initial` / `super::feed_color_index` not
found.

- [ ] **Step 3: Add the free functions and delegate from `EntryRowView`**

In `src/handlers/pages/mod.rs`, add these two free functions just above the
`EntryRowView` struct (~line 30, after the section comment):

```rust
/// Uppercased first character of a feed title, for the favicon letter-chip
/// fallback shown when a feed has no icon. Returns "?" for an empty title.
pub(crate) fn feed_initial(feed_title: &str) -> String {
    feed_title
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// Stable index 0..6 into the favicon fallback colour palette, derived from
/// the feed id so the same feed always gets the same colour.
pub(crate) fn feed_color_index(feed_id: i64) -> u8 {
    feed_id.rem_euclid(6) as u8
}
```

Then replace the bodies of the existing `EntryRowView` methods (~lines 59-71) to
delegate:

```rust
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
    }
```

- [ ] **Step 4: Add fields + delegating methods to `ReadingPaneView`**

In the `ReadingPaneView` struct (`src/handlers/pages/mod.rs`, ~80-101), add two
fields (place them after `feed_title`):

```rust
    pub feed_title: String,
    pub feed_id: i64,
    pub feed_has_icon: bool,
```

Add an `impl` block immediately after the struct's closing `}` (~line 101):

```rust
impl ReadingPaneView {
    /// Uppercased first character of the feed title for the favicon
    /// letter-chip fallback (mirrors `EntryRowView::feed_initial`).
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    /// Stable favicon-palette index derived from the feed id (mirrors
    /// `EntryRowView::feed_color_index`).
    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
    }
}
```

- [ ] **Step 5: Populate the new fields in the constructor**

In `src/handlers/entries.rs`, in `build_reading_pane_view` (the `Ok(ReadingPaneView {`
literal at line 213), add the two fields right after `feed_title`:

```rust
        feed_title: ewf.feed_title.clone().unwrap_or_default(),
        feed_id: ewf.entry.feed_id,
        feed_has_icon: ewf.feed_has_icon,
```

- [ ] **Step 6: Run the tests to verify they PASS**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt
RDRS_FAST_HASH=1 cargo nextest run -p rdrs feed_initial 2>&1 | tail -20
RDRS_FAST_HASH=1 cargo nextest run -p rdrs feed_color_index 2>&1 | tail -20
```

Expected: all `feed_initial_*` / `feed_color_index_*` tests PASS (the new
free-fn tests plus the pre-existing `EntryRowView` delegating tests). Also
confirm the crate builds: `cargo build 2>&1 | tail -5`.

- [ ] **Step 7: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add src/handlers/pages/mod.rs src/handlers/entries.rs
git commit -S -m "refactor(pane): share favicon helpers, add feed identity to ReadingPaneView

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Template — favicon chip, `.rp-action` controls, icon-only nav

Rewrite the reading-pane chrome markup. Keep every swap target, `data-testid`,
form action, and accessible name. The handler test assertions are the test.

**Files:**
- Modify: `templates/_reading_pane.html`
- Modify: `tests/handlers_test.rs` (`test_entry_fragment_renders_reading_pane`, ~3624)

- [ ] **Step 1: Add favicon-chip + `.rp-action` + nav assertions (failing)**

In `tests/handlers_test.rs`, inside `test_entry_fragment_renders_reading_pane`,
after the existing `assert!(html.contains("Body text here") …)` (~line 3630),
add:

```rust
    // Editorial redesign: meta shows the feed favicon chip (feed "Test Feed"
    // has no icon -> coloured initial chip "T").
    assert!(
        html.contains("entry-favicon-chip"),
        "reading pane meta must render the favicon chip fallback"
    );
    // Actions use the shared .rp-action control with stable aria-labels.
    assert!(
        html.contains(r#"class="rp-action""#),
        "reading pane actions must use the .rp-action control"
    );
    assert!(
        html.contains(r#"aria-label="Mark Unread""#),
        "Mark Unread action must keep its accessible name"
    );
```

- [ ] **Step 2: Run to verify it FAILS**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_entry_fragment_renders_reading_pane 2>&1 | tail -20
```

Expected: FAIL — `entry-favicon-chip` / `rp-action` not yet in the template.

- [ ] **Step 3: Rewrite the toolbar nav (icon-only on mobile)**

In `templates/_reading_pane.html`, replace the two `.reading-pane-nav-btn`
buttons (~lines 22-23) with versions that carry an `aria-label` and wrap the
word in a `.nav-label` span (hidden on mobile via CSS):

```html
            <button type="button" class="reading-pane-nav-btn" data-pane-prev data-testid="reading-pane-prev" aria-label="Previous" disabled>‹<span class="nav-label"> Previous</span></button>
            <button type="button" class="reading-pane-nav-btn" data-pane-next data-testid="reading-pane-next" aria-label="Next" disabled><span class="nav-label">Next </span>›</button>
```

- [ ] **Step 4: Add the favicon chip to the meta line**

Replace the `.reading-pane-meta` block (~lines 30-34) with:

```html
        <div class="reading-pane-meta">
            {% if pane.feed_has_icon %}
            <img class="entry-favicon" src="/api/feeds/{{ pane.feed_id }}/icon" alt="" width="24" height="24">
            {% else %}
            <span class="entry-favicon entry-favicon-chip fav-c{{ pane.feed_color_index() }}" aria-hidden="true">{{ pane.feed_initial() }}</span>
            {% endif %}
            <span data-testid="reading-pane-feed-title">{{ pane.feed_title }}</span>
            {% if let Some(author) = pane.author.as_ref() %}<span>{{ author }}</span>{% endif %}
            {% if let Some(ts) = pane.published_at_iso.as_ref() %}<time datetime="{{ ts }}" data-testid="reading-pane-published-at">{{ pane.published_relative }}</time>{% endif %}
        </div>
```

(The mid-dot separator is handled in CSS Task 3 Step 2 — the new rule is
favicon-safe and only dots when an author is present, matching today.)

- [ ] **Step 5: Rewrite the actions block to `.rp-action` controls**

Replace the entire `.reading-pane-actions` block (~lines 35-69) with the
following. Each control is `icon span (aria-hidden) + label span`; the
`aria-label` carries the canonical name the e2e suite relies on:

```html
        <div class="reading-pane-actions">
            <form id="reading-pane-star-form-{{ pane.id }}" method="post" action="/entries/{{ pane.id }}/{% if pane.is_starred %}unstar{% else %}star{% endif %}" data-swap="#entry-row-{{ pane.id }}">
                <button type="submit" class="rp-action" aria-label="{% if pane.is_starred %}Unstar{% else %}Star{% endif %}"><span class="action-icon" aria-hidden="true">{% if pane.is_starred %}★{% else %}☆{% endif %}</span><span class="action-label">{% if pane.is_starred %}Starred{% else %}Star{% endif %}</span></button>
            </form>
            <form method="post" action="/entries/{{ pane.id }}/unread" data-swap="#entry-row-{{ pane.id }}">
                <button type="submit" class="rp-action" aria-label="Mark Unread"><span class="action-icon" aria-hidden="true">↺</span><span class="action-label">Unread</span></button>
            </form>
            {% if let Some(link) = pane.link.as_ref() %}
            {% if pane.is_full_content %}
            <a href="/entries/{{ pane.id }}/fragment" data-swap="#reading-pane" class="rp-action" aria-label="Show Original"><span class="action-icon" aria-hidden="true">↩</span><span class="action-label">Original</span></a>
            {% else %}
            <form method="post" action="/entries/{{ pane.id }}/fetch-full-content" data-swap="#reading-pane">
                <button type="submit" class="rp-action" aria-label="Fetch Full Content"><span class="action-icon" aria-hidden="true">⤓</span><span class="action-label">Full content</span></button>
            </form>
            {% endif %}
            {% if pane.has_kagi || pane.summary_in_flight %}
            <form method="post" action="/entries/{{ pane.id }}/summarize" data-swap="#rp-summary-container">
                <button type="submit" class="rp-action" aria-label="Summarize"{% if pane.summary_in_flight %} disabled{% endif %}><span class="action-icon" aria-hidden="true">✦</span><span class="action-label">Summarize</span></button>
            </form>
            {% endif %}
            {% if pane.has_save %}
            <form method="post" action="/entries/{{ pane.id }}/save" data-swap="#reading-pane">
                <button type="submit" class="rp-action" aria-label="Save"><span class="action-icon" aria-hidden="true">⬇</span><span class="action-label">Save</span></button>
            </form>
            {% endif %}
            {% endif %}
        </div>
```

- [ ] **Step 6: Restyle the summary-box Copy / Dismiss controls**

In the same file, replace the two `.summary-actions` buttons (~lines 81-83) with
`.rp-action` controls:

```html
                <div class="summary-actions">
                    <button type="button" class="rp-action" data-summary-copy aria-label="Copy summary"><span class="action-icon" aria-hidden="true">⧉</span><span class="action-label">Copy</span></button>
                    <button type="button" class="rp-action" data-summary-dismiss data-entry-id="{{ pane.id }}" aria-label="Dismiss summary"><span class="action-icon" aria-hidden="true">✕</span><span class="action-label">Dismiss</span></button>
                </div>
```

- [ ] **Step 7: Run the handler test to verify it PASSES**

```bash
pwd
source /tmp/rdrs-env.sh
RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_entry_fragment_renders_reading_pane 2>&1 | tail -20
```

Expected: PASS. Then run the full reading-pane-related Rust suite to catch any
assertion that referenced the old markup:

```bash
RDRS_FAST_HASH=1 cargo nextest run -p rdrs reading 2>&1 | tail -20
RDRS_FAST_HASH=1 cargo nextest run -p rdrs star 2>&1 | tail -20
```

Expected: PASS. If a test asserted the old `btn-sm` / "Star"/"Mark Unread" plain
text, update it to the new markup (the star toggle still exposes
`aria-label="Unstar"` when starred; the swap target `#reading-pane-star-form-…`
is unchanged).

- [ ] **Step 8: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add templates/_reading_pane.html tests/handlers_test.rs
git commit -S -m "feat(pane): favicon chip + .rp-action controls + icon-only nav markup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: CSS — title size, `.rp-action`, desktop row, mobile bottom bar

All new mobile rules go **inside** the existing `@media (max-width: 1024px)`
block. Desktop rules go in the main (top) section near the existing
`.reading-pane-*` rules (~lines 449-571).

**Files:**
- Modify: `static/css/app.css`

- [ ] **Step 1: Shrink the title (desktop)**

Change `.reading-pane-title` `font-size` (line ~532) from `var(--font-3xl)` to:

```css
    font-size: var(--font-2xl);
```

- [ ] **Step 2: Replace the meta separator rule with a favicon-safe one**

The current rule (~lines 552-555) assumes the feed title is the first child; the
favicon now precedes it. Replace that rule with one anchored to the feed-title
span and gated on an author being present (a `<span>` that is neither the
feed-title nor the favicon chip):

```css
/* Mid-dot separators before author/time, only when an author span is present.
   Anchored to the feed-title so the leading favicon (img or chip span) never
   gets a dot. */
.reading-pane-meta:has(> span:not([data-testid]):not(.entry-favicon)) [data-testid="reading-pane-feed-title"] ~ *::before {
    content: "·";
    margin-right: var(--space-2);
}
```

Add, right after it, a small override so the reused entry-row favicon centers in
the flex meta line (its base rule has `margin-top: 2px` for the entry grid):

```css
.reading-pane-meta .entry-favicon {
    margin-top: 0;
}
```

- [ ] **Step 3: Add the `.rp-action` base + desktop action row**

Replace the existing `.reading-pane-actions` rule (~lines 557-564) with the row
container plus the shared control. (Desktop: same-level, nowrap, no grouping.)

```css
.reading-pane-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-bottom: var(--space-8);
    padding-bottom: var(--space-6);
    border-bottom: 1px solid var(--color-border-light);
}

/* Shared pane action control (also used by the summary-box Copy/Dismiss).
   Icon + label, never wraps its label; desktop ~40px tall. */
.rp-action {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 2.5rem;
    padding: 0 var(--space-4);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg);
    color: var(--color-text-secondary);
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    line-height: 1;
    white-space: nowrap;
    cursor: pointer;
    text-decoration: none;
}

.rp-action:hover {
    color: var(--color-accent);
    border-color: var(--color-accent);
}

.rp-action .action-icon {
    color: var(--color-accent);
}

.rp-action:disabled {
    opacity: 0.5;
    cursor: default;
}
```

- [ ] **Step 4: Add the `.nav-label` rule (visible by default)**

Add near the `.reading-pane-nav-btn` rules (~line 510):

```css
/* The prev/next word labels; hidden on mobile (icon-only) in the media block. */
.nav-label {
    display: inline;
}
```

- [ ] **Step 5: Add the mobile rules inside `@media (max-width: 1024px)`**

Find the `@media (max-width: 1024px)` block. Add the following just after the
existing `.reading-pane-back-link { … }` rule (~line 1789), still inside the
media block:

```css
    /* Title is smaller on mobile (no sidebar to size against on desktop). */
    .reading-pane-title {
        font-size: var(--font-xl);
    }

    /* Prev/next become icon-only to save toolbar width; keep a square-ish
       touch target (the generic baseline already sets min-height). */
    .nav-label {
        display: none;
    }

    .reading-pane-nav-btn {
        min-width: var(--touch-min);
        justify-content: center;
    }

    /* Actions move to a fixed bottom bar: thumb-reachable, available while the
       article scrolls. flex:1 + space-around spreads 2-5 controls evenly. */
    .reading-pane-actions {
        position: fixed;
        left: 0;
        right: 0;
        bottom: 0;
        z-index: 101;
        margin: 0;
        padding: var(--space-2);
        gap: var(--space-1);
        justify-content: space-around;
        background: var(--color-bg-secondary);
        border-top: 1px solid var(--color-border);
        border-bottom: none;
    }

    /* Both the form-wrapped buttons and the bare <a> are direct children. */
    .reading-pane-actions > * {
        flex: 1;
    }

    .reading-pane-actions form {
        display: flex;
    }

    /* Stacked icon-over-label, borderless, big icon, ≥48px tall. */
    .reading-pane-actions .rp-action {
        flex: 1;
        flex-direction: column;
        gap: 2px;
        min-height: 3rem;
        padding: var(--space-1);
        border-color: transparent;
        background: transparent;
        font-size: var(--font-xs);
    }

    .reading-pane-actions .rp-action .action-icon {
        font-size: var(--font-xl);
    }

    /* Clear the fixed bar so the last paragraph isn't hidden behind it. */
    .reading-pane-content {
        padding-bottom: calc(3rem + 2 * var(--space-2) + var(--space-4));
    }
```

- [ ] **Step 6: Build so the binary picks up the CSS, then eyeball it**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -5
```

Expected: clean build. (Visual confirmation happens via the e2e scenario in
Task 4; there is no Rust unit test for CSS.)

- [ ] **Step 7: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add static/css/app.css
git commit -S -m "style(pane): editorial chrome — bottom action bar (mobile), inline row (desktop)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: e2e — mobile bottom action bar scenario

Add a `@mobile` scenario asserting the redesigned chrome. Reuse the generic
size step already in `responsive.steps.js`
(`the "{selector}" control is at least {int}px tall`).

**Files:**
- Modify: `e2e/features/reading.feature`

- [ ] **Step 1: Inspect the existing mobile reading scenario for reuse**

Read `e2e/features/reading.feature` around the existing mobile nav scenario
(~line 180, "Reading-pane navigation survives Fetch Full Content on mobile") to
reuse its Given/When steps for opening the pane on mobile (`I am viewing on a
mobile screen`, seeding a feed with entries, opening the inbox, clicking an
entry, `the reading pane is visible on mobile`).

- [ ] **Step 2: Append the new scenario**

Append to `e2e/features/reading.feature`:

```gherkin
  @mobile
  Scenario: Reading-pane actions sit in a touch-sized bottom bar on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane is visible on mobile
    And the ".rp-action" control is at least 44px tall
    And the ".reading-pane-actions" control is at least 300px wide
```

(The 44px height confirms the touch target; the ≥300px width confirms the bar
spans the 375px viewport — i.e. it is the fixed full-width bar, not an inline
content-width cluster.)

- [ ] **Step 3: Confirm the step phrasings already exist**

The steps `Given I am viewing on a mobile screen`, `Given I have a feed with 5
test entries`, `When I open the inbox`, `When I click the entry titled "…"`,
`Then the reading pane is visible on mobile`, and the generic
`the "{selector}" control is at least {int}px tall` / `…wide` all already exist
(used by `responsive.feature` and the existing mobile reading scenarios). No new
step definitions are required. If `grep -rn` shows any phrase missing, stop and
report rather than inventing a step.

```bash
pwd
grep -rn "control is at least" e2e/steps/responsive.steps.js
grep -rn "reading pane is visible on mobile" e2e/steps
```

Expected: both phrasings are found.

- [ ] **Step 4: Build + run the new scenario (TDD: it must pass with Task 3 CSS)**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -5
cd e2e && npx playwright test reading --grep @mobile 2>&1 | tail -30
```

Expected: the new scenario PASSES (bottom bar ≥44px tall, ≥300px wide) along
with the existing mobile reading scenarios.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/features/reading.feature
git commit -S -m "test(e2e): assert mobile reading-pane bottom action bar

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full regression sweep

Confirm nothing regressed across Rust + e2e. Verification only.

**Files:** none

- [ ] **Step 1: Full Rust suite**

```bash
pwd
source /tmp/rdrs-env.sh
cargo fmt --check
RDRS_FAST_HASH=1 cargo nextest run -p rdrs 2>&1 | tail -25
```

Expected: all tests PASS, fmt clean.

- [ ] **Step 2: e2e — the suites that touch the reading pane and responsive chrome**

```bash
pwd
source /tmp/rdrs-env.sh
cargo build 2>&1 | tail -5
cd e2e && npx playwright test reading triage responsive keyboard_shortcuts_mobile 2>&1 | tail -40
```

Expected: all scenarios PASS. Key ones to confirm by name:
- `reading.feature`: Fetch Full Content (desktop) + navigation survives Fetch
  Full Content on mobile + the new bottom-bar scenario.
- `triage.feature`: the Summarize flow (clicks the `Summarize` button by
  accessible name — must still resolve).
- `responsive.feature`: existing `@mobile` tap-target + reading-pane-back
  scenarios.

- [ ] **Step 3: If anything failed, fix and re-run**

Most likely failure modes and fixes:
- A Rust test asserted old pane markup (`btn-sm`, plain "Star"/"Mark Unread")
  → update to the new `.rp-action` + `aria-label` markup.
- e2e `getByRole("button", { name: … })` could not find an action → confirm the
  `aria-label` exactly matches the feature's string (`Summarize`,
  `Fetch Full Content`).
- Desktop label wrapped → confirm `white-space: nowrap` + the `.rp-action` rule
  shipped.

Then re-run the failing suite.

---

## Self-Review

**Spec coverage:**
- Mobile fixed bottom action bar, flex:1 + space-around, 2-5 adaptive → Task 3 Step 5. ✅
- Desktop same-level inline row, nowrap, no grouping → Task 3 Step 3. ✅
- Title serif, smaller (2xl desktop / xl mobile) → Task 3 Steps 1 & 5. ✅
- Favicon chip in meta (img or coloured initial) → Task 2 Step 4 + Task 1. ✅
- Shared `feed_initial` / `feed_color_index` (DRY) → Task 1 Steps 3-4. ✅
- `.rp-action` icon + label, stable `aria-label`s → Task 2 Steps 5-6. ✅
- Icon-only prev/next on mobile, `aria-label` kept → Task 2 Step 3 + Task 3 Step 5. ✅
- Summary-box Copy/Dismiss restyled → Task 2 Step 6. ✅
- Article body untouched → no task edits `.reading-pane-article`. ✅
- Favicon-safe meta separator → Task 3 Step 2. ✅
- Content padding-bottom clears the fixed bar → Task 3 Step 5. ✅
- e2e mobile bottom-bar coverage → Task 4. ✅
- Accessible names preserved for existing e2e (Summarize, Fetch Full Content,
  Star/Unstar) → Task 2 Step 5 + Task 5 Step 2. ✅

**Placeholder scan:** No TBD/TODO; every code/step is concrete. ✅

**Type/name consistency:** `feed_initial(&str)` / `feed_color_index(i64)` free
fns defined in Task 1 Step 3 and delegated in Steps 3-4; `ReadingPaneView.feed_id`
/ `feed_has_icon` defined Task 1 Step 4, populated Step 5; `.rp-action` class
used identically across Task 2 (template), Task 3 (CSS), Task 4 (e2e selector);
`aria-label="Mark Unread"` asserted (Task 2 Step 1) matches the template (Task 2
Step 5). ✅
