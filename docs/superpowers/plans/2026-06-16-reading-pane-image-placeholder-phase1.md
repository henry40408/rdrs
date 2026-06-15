# Reading-Pane Image Placeholder — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reserve space for content images, show a loading skeleton, harvest cheap dimension hints, and render a graceful broken-image fallback in the reading pane — with no new infrastructure.

**Architecture:** Server-side sanitize tags every content `<img>` with `data-img-state="loading"` and harvests `width`/`height` from `data-original-width/height` or inline `style` (pre-ammonia, since ammonia strips those). CSS shows a static tinted skeleton while loading and a temporary `16/9` box for still-dimensionless images. A small vanilla-JS pass flips each image to `loaded` on load or replaces it with a dashed-box fallback on error, hooked on initial load and on every `rdrs:swap-complete`.

**Tech Stack:** Rust (lol_html, ammonia, Axum, Askama), vanilla ES modules, Playwright BDD.

**Spec:** `docs/superpowers/specs/2026-06-16-reading-pane-image-placeholder-design.md` (Phase 1 only — Layer 3 cache/worker is a separate later plan).

**Environment:** NixOS — prefix every `cargo`/`npm`/`playwright` command with `source /tmp/rdrs-env.sh &&`. Run Rust tests with `RDRS_FAST_HASH=1`. `cargo fmt` before committing; keep `cargo clippy --all-targets -- -D warnings` clean. Commits GPG-signed (`git commit -S`). Stage files explicitly — never `git add -A`/`.`. Static assets are `include_str!`'d into the binary, so **`cargo build` before any e2e/screenshot run**.

---

## File Structure

- `src/services/sanitize.rs` — add a pre-ammonia `harvest_image_dimensions` pass; have the post-ammonia `img_handler` tag images with `data-img-state="loading"`.
- `static/css/app.css` — skeleton + temporary-ratio + `.rp-broken-image` styles, near the existing `.reading-pane-article img` rule (~line 671).
- `static/js/app.js` — `initPaneImages()` + `markBrokenImage()`, hooked on init and `rdrs:swap-complete`.
- `templates/_icons.html` — a `broken_image` icon macro (for the JS fallback we inline the SVG in JS; the macro is only if a template hook is needed — see Task 4; not required).
- `e2e/features/reading.feature`, `e2e/steps/*.steps.js`, `e2e/support/seed.js` — skeleton/loaded/broken scenarios.

> Phase 1 does NOT change the `sanitize_html` signature (the dimension-cache ctx is Phase 2). All five production callers stay as-is.

---

## Task 1: Characterize Layer 1 (dimensions survive sanitize)

**Files:**
- Test: `src/services/sanitize.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the test**

Add to the tests module:

```rust
    #[test]
    fn test_image_width_height_preserved() {
        let input = r#"<img src="https://example.com/a.jpg" width="640" height="480" alt="x">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"640\""), "width must survive: {output}");
        assert!(output.contains("height=\"480\""), "height must survive: {output}");
    }
```

- [ ] **Step 2: Run it**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_image_width_height_preserved 2>&1 | tail -8`
Expected: PASS (ammonia's default `img` attributes include `width`/`height`). If it FAILS, ammonia is stripping them — then add `.add_tag_attributes("img", &["width","height","alt"])` to the `Builder` in `sanitize_html` and re-run. Report which path was taken.

- [ ] **Step 3: Commit**

```bash
source /tmp/rdrs-env.sh && cargo fmt
git add src/services/sanitize.rs
git commit -S -m "test: assert image dimensions survive sanitization"
```

---

## Task 2: Harvest dimension hints (pre-ammonia)

**Files:**
- Modify: `src/services/sanitize.rs`

- [ ] **Step 1: Write failing tests**

Add to the tests module:

```rust
    #[test]
    fn test_harvest_dims_from_data_original() {
        let input = r#"<img src="https://e.com/a.jpg" data-original-width="800" data-original-height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"800\""), "{output}");
        assert!(output.contains("height=\"600\""), "{output}");
    }

    #[test]
    fn test_harvest_dims_from_style() {
        let input = r#"<img src="https://e.com/a.jpg" style="width:320px;height:240px">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"320\""), "{output}");
        assert!(output.contains("height=\"240\""), "{output}");
    }

    #[test]
    fn test_harvest_skips_when_dims_present() {
        // Existing width/height win; hints are ignored.
        let input = r#"<img src="https://e.com/a.jpg" width="100" height="50" data-original-width="800" data-original-height="600">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("width=\"100\""), "{output}");
        assert!(!output.contains("width=\"800\""), "{output}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run -p rdrs harvest_dims 2>&1 | tail -12`
Expected: FAIL (no width/height injected yet).

- [ ] **Step 3: Implement the harvest pass**

Add this function above `promote_lazy_images` in `src/services/sanitize.rs`:

```rust
/// Parse a `width:NNpx` / `height:NNpx` integer out of an inline `style`.
fn style_dim(style: &str, prop: &str) -> Option<String> {
    for decl in style.split(';') {
        let mut kv = decl.splitn(2, ':');
        let key = kv.next()?.trim();
        if !key.eq_ignore_ascii_case(prop) {
            continue;
        }
        let val = kv.next()?.trim();
        // Accept "320px" or "320"; take leading integer.
        let digits: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    None
}

/// Pre-ammonia pass: for any `<img>` lacking BOTH `width` and `height`, inject
/// them from `data-original-width`/`data-original-height` or an inline
/// `style="width:..px;height:..px"`. Ammonia strips those hint sources, so this
/// must run before it. Only injects when a usable integer PAIR is found.
fn harvest_image_dimensions(html: &str) -> String {
    use lol_html::{element, rewrite_str, RewriteStrSettings};
    let handler = element!("img", |el| {
        if el.get_attribute("width").is_some() || el.get_attribute("height").is_some() {
            return Ok(());
        }
        let style = el.get_attribute("style").unwrap_or_default();
        let w = el
            .get_attribute("data-original-width")
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
            .or_else(|| style_dim(&style, "width"));
        let h = el
            .get_attribute("data-original-height")
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
            .or_else(|| style_dim(&style, "height"));
        if let (Some(w), Some(h)) = (w, h) {
            el.set_attribute("width", &w)?;
            el.set_attribute("height", &h)?;
        }
        Ok(())
    });
    rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![handler],
            ..RewriteStrSettings::default()
        },
    )
    .unwrap_or_else(|_| html.to_string())
}
```

Then in `sanitize_html`, run it right after `promote_lazy_images` and before ammonia. Change:

```rust
    let unlazied = promote_lazy_images(content);
```
to:
```rust
    let unlazied = promote_lazy_images(content);
    let unlazied = harvest_image_dimensions(&unlazied);
```

> Note: `lol_html`'s `element!`/`rewrite_str`/`RewriteStrSettings` are already used by `rewrite_post_ammonia` in this file — reuse the same imports/pattern. If `RewriteStrSettings { element_content_handlers, .. }` struct-literal form differs from the existing usage, match the existing builder form (`RewriteStrSettings::new().append_element_content_handler(handler)`).

- [ ] **Step 4: Run to verify pass**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run -p rdrs harvest_dims 2>&1 | tail -12 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -4`
Expected: 3 harvest tests PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
source /tmp/rdrs-env.sh && cargo fmt
git add src/services/sanitize.rs
git commit -S -m "feat: harvest image dimension hints before sanitization"
```

---

## Task 3: Tag images with loading state (server-side)

**Files:**
- Modify: `src/services/sanitize.rs` (the `img_handler` in `rewrite_post_ammonia`)

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn test_img_tagged_loading_state() {
        let input = r#"<img src="https://e.com/a.jpg" alt="x">"#;
        let output = sanitize_html(input, TEST_SECRET, None, None, None);
        assert!(output.contains("data-img-state=\"loading\""), "{output}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run -p rdrs test_img_tagged_loading_state 2>&1 | tail -8`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `rewrite_post_ammonia`'s `img_handler`, inside the `if let Some(url) = absolute_url {` block where `loading`/`decoding` are already set (after `el.set_attribute("decoding", "async")?;`), add:

```rust
                el.set_attribute("data-img-state", "loading")?;
```

- [ ] **Step 4: Run to verify pass**

Run: `source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run -p rdrs sanitize 2>&1 | tail -10`
Expected: the new test passes and existing sanitize tests still pass.

- [ ] **Step 5: Commit**

```bash
source /tmp/rdrs-env.sh && cargo fmt
git add src/services/sanitize.rs
git commit -S -m "feat: tag proxied content images with data-img-state=loading"
```

---

## Task 4: Skeleton + broken-image CSS

**Files:**
- Modify: `static/css/app.css` (after the `.reading-pane-article img` rule, ~line 677)

- [ ] **Step 1: Add the CSS**

Insert directly after the `.reading-pane-article img { ... }` block:

```css
/* Loading skeleton: a static tinted block until JS flips data-img-state to
   "loaded". Set server-side in sanitize so it shows on first paint. */
.reading-pane-article img[data-img-state="loading"] {
    background: var(--color-bg-secondary);
}

/* Dimensionless images reserve a 16/9 box while loading so the pane doesn't
   collapse; the natural ratio takes over once loaded (the unavoidable
   first-view ratio settle). Article images are display:block already, so this
   never affects inline glyphs. */
.reading-pane-article img[data-img-state="loading"]:not([width]):not([height]) {
    aspect-ratio: 16 / 9;
    width: 100%;
    object-fit: cover;
}

/* Broken-image fallback (JS replaces the <img> with this on error). Dashed box,
   neutral palette, centered glyph + alt caption. */
.rp-broken-image {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    aspect-ratio: 16 / 9;
    margin: var(--space-6) auto;
    padding: var(--space-4);
    text-align: center;
    background: var(--color-bg-secondary);
    border: 1px dashed var(--color-border-light);
    border-radius: var(--radius-md);
}
.rp-broken-image .ico {
    width: 28px;
    height: 28px;
    color: var(--color-text-muted);
    opacity: 0.6;
}
.rp-broken-cap {
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    color: var(--color-text-muted);
}
```

- [ ] **Step 2: Build to embed the CSS**

Run: `source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -2`
Expected: builds.

- [ ] **Step 3: Commit**

```bash
git add static/css/app.css
git commit -S -m "feat: reading-pane image skeleton + broken-image fallback styles"
```

---

## Task 5: Per-image load/error lifecycle (JS)

**Files:**
- Modify: `static/js/app.js`

- [ ] **Step 1: Add the image-init functions**

Add near the other pane helpers (e.g. after `cancelPaneImages`, ~line 111):

```js
// Reading-pane content images: flip the server-set data-img-state="loading"
// skeleton to "loaded" on load, or replace the image with a dashed-box
// fallback on error. Idempotent per image via data-img-init.
function markBrokenImage(img) {
    const box = document.createElement('div');
    box.className = 'rp-broken-image';
    box.innerHTML =
        '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
        'stroke-width="1.5" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/>' +
        '<path d="M3 16l5-5 4 4"/><circle cx="8.5" cy="9" r="1.3"/><path d="M4 4l16 16"/></svg>';
    const cap = document.createElement('span');
    cap.className = 'rp-broken-cap';
    const alt = (img.getAttribute('alt') || '').trim();
    // textContent — never innerHTML — so alt text can't inject markup.
    cap.textContent = alt ? `Image unavailable — ${alt}` : 'Image unavailable';
    box.appendChild(cap);
    // Preserve reserved height for dimensioned images.
    const w = img.getAttribute('width');
    const h = img.getAttribute('height');
    if (w && h) box.style.aspectRatio = `${w} / ${h}`;
    img.replaceWith(box);
}

function initPaneImages() {
    const pane = document.getElementById('reading-pane');
    if (!pane) return;
    for (const img of pane.querySelectorAll('.reading-pane-article img:not([data-img-init])')) {
        img.setAttribute('data-img-init', '');
        // Already settled (e.g. cached) before we attached handlers.
        if (img.complete) {
            if (img.naturalWidth > 0) img.setAttribute('data-img-state', 'loaded');
            else markBrokenImage(img);
            continue;
        }
        img.addEventListener('load', () => img.setAttribute('data-img-state', 'loaded'), { once: true });
        img.addEventListener('error', () => markBrokenImage(img), { once: true });
    }
}
```

- [ ] **Step 2: Hook it on initial load and after swaps**

Find the bootstrap call `installSwap();` (~line 549) and the existing
`document.addEventListener('rdrs:swap-complete', () => applyTimeTooltips());`
(~line 623). Mirror that pattern: add right after the `applyTimeTooltips`
listener:

```js
document.addEventListener('rdrs:swap-complete', () => initPaneImages());
```

And ensure it runs for the SSR-rendered pane on initial load. If `app.js`
already has a DOMContentLoaded/bootstrap block that calls `applyTimeTooltips()`
once, add `initPaneImages();` beside it. If there is no such block, add at the
bottom of the module top-level (the module runs after DOM parse for a deferred
`<script type="module">`):

```js
initPaneImages();
```

> Verify how `applyTimeTooltips()` is invoked on first load and match it exactly
> (same call site / guard), so `initPaneImages()` has identical lifecycle.

- [ ] **Step 3: Build**

Run: `source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -2`
Expected: builds (embeds the new JS).

- [ ] **Step 4: Commit**

```bash
git add static/js/app.js
git commit -S -m "feat: reading-pane image load/error lifecycle (skeleton + broken fallback)"
```

---

## Task 6: E2E coverage

**Files:**
- Modify: `e2e/support/seed.js`, `e2e/steps/entries.steps.js` (or the reading steps file), `e2e/features/reading.feature`

- [ ] **Step 1: Inspect the existing harness**

Read `e2e/support/seed.js` (how `seedTestEntries` / `upsert` sets entry
`content`) and the reading.feature Background. Determine how to seed an entry
whose content contains a chosen `<img>`. If no helper sets arbitrary content,
add one mirroring the existing insert style:

```javascript
  setEntryContent(entryId, html) {
    const db = new Database(this.dbPath);
    try {
      db.prepare("UPDATE entry SET content = ? WHERE id = ?").run(html, entryId);
    } finally {
      db.close();
    }
  }
```

(Match the file's actual DB-access convention — e.g. `this.db` vs
`new Database(this.dbPath)`.)

- [ ] **Step 2: Add steps**

In the reading steps file, add (adapt `this.page`/`expect` to the file style):

```javascript
Given("the entry titled {string} has content with a broken image", async function (title) {
  const userId = this.seed.getUserId(this.currentUser.username);
  const entryId = this.seed.findEntryIdByTitle(userId, title);
  // Unresolvable host → the proxy fetch fails → browser fires img error.
  this.seed.setEntryContent(
    entryId,
    '<p>x</p><img src="https://invalid.example.invalid/missing.jpg" alt="Missing diagram">',
  );
});

Then("the reading pane shows a broken-image fallback", async function () {
  await expect(this.page.locator(".reading-pane-article .rp-broken-image")).toBeVisible();
  await expect(this.page.locator(".rp-broken-cap")).toContainText("Image unavailable");
});

Then("the reading pane image is marked loaded", async function () {
  await expect(
    this.page.locator('.reading-pane-article img[data-img-state="loaded"]').first(),
  ).toBeVisible();
});
```

> Match the exact helper names the sibling steps use to resolve `userId`/`entryId`
> (copy from the existing `... has a summary` step). Use the real World accessor
> (`this.page` vs destructured `{ page }`) consistent with that file.

- [ ] **Step 3: Add scenarios**

In `e2e/features/reading.feature`, add after the existing summary scenarios:

```gherkin
  Scenario: A broken content image shows the dashed-box fallback
    Given the entry titled "Test Entry 3" has content with a broken image
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    Then the reading pane shows a broken-image fallback
```

> Reuse existing steps for open/click (confirm phrasing with
> `rg -n "I open the inbox|I click the entry titled" e2e/steps`). Use a title the
> Background seeds. The "loaded" assertion is harder to make deterministic
> against a live external image; the broken-image path is the reliable e2e — keep
> the loaded-state check as a Rust/JS-level concern or a follow-up if a locally
> served image fixture isn't readily available.

- [ ] **Step 4: Rebuild, regenerate, run**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -2
cd e2e && npx bddgen >/dev/null 2>&1 && source /tmp/rdrs-env.sh && npx playwright test --grep "broken content image" 2>&1 | tail -15
```
Expected: the scenario passes (the unresolvable host triggers `error` → fallback). If the proxy returns a placeholder/200 instead of failing, point the `src` at a path the proxy rejects (e.g. an SSRF-blocked host) so the browser still fires `error`; report what was needed.

- [ ] **Step 5: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/support/seed.js e2e/features/reading.feature e2e/steps
git commit -S -m "test(e2e): reading-pane broken-image fallback"
```

---

## Task 7: Screenshots, docs, full verification

**Files:**
- Possibly `screenshots/*.png`, `ARCHITECTURE.md`

- [ ] **Step 1: Regenerate screenshots, check diff**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -2
cd e2e && source /tmp/rdrs-env.sh && npm run screenshots 2>&1 | tail -4
cd /home/nixos/Develop/claude/rdrs && git status --porcelain screenshots/
```
Expected: likely no change (demo images load fine). If changed, eyeball one with
the Read tool and include the updated images only if correct.

- [ ] **Step 2: Update docs**

If `ARCHITECTURE.md` describes the sanitize/image pipeline, add one concise
clause that content images get dimension-hint harvesting + a loading skeleton +
broken-image fallback. Search: `rg -n -i "sanitize|image proxy|images" ARCHITECTURE.md`. If no relevant section exists, make no change and say so.

- [ ] **Step 3: Full gate**

```bash
source /tmp/rdrs-env.sh && cargo fmt --all -- --check && echo FMT_OK
source /tmp/rdrs-env.sh && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
source /tmp/rdrs-env.sh && RDRS_FAST_HASH=1 cargo nextest run 2>&1 | tail -4
cd e2e && source /tmp/rdrs-env.sh && npx playwright test --grep-invert "@skip" 2>&1 | tail -6
```
Expected: fmt clean, clippy clean, all Rust tests pass, e2e green. Report any failure verbatim.

- [ ] **Step 4: Commit any docs/screenshot changes**

```bash
cd /home/nixos/Develop/claude/rdrs
git add ARCHITECTURE.md   # only if edited
# git add screenshots/<files>  # only if Step 1 produced correct diffs
git commit -S -m "docs: note reading-pane image placeholder handling"
```
If nothing changed in Steps 1-2, skip the commit and say so.

---

## Self-Review Notes

- **Spec coverage (Phase 1):** Layer 1 (Task 1), hint harvest incl. data-original/style (Task 2), server `data-img-state` (Task 3), skeleton + temp ratio + broken box CSS visual decisions A/A (Task 4), JS load/error + broken fallback with alt, XSS-safe via textContent (Task 5), e2e broken-image (Task 6), screenshots/docs/gate (Task 7). Layer 3 (cache + worker + sanitize ctx + miss enqueue) is explicitly deferred to a Phase 2 plan.
- **Placeholder scan:** none — every code step has concrete code; the one judgment call (broken-image e2e trigger) has a stated fallback.
- **Type/name consistency:** `data-img-state` ("loading"/"loaded"/"broken"), `data-img-init`, `.rp-broken-image`, `.rp-broken-cap`, `markBrokenImage`, `initPaneImages` are used identically across Tasks 3-6. CSS targets `:not([width]):not([height])` matching the harvested attrs from Task 2.
- **srcset/privacy:** unchanged in Phase 1 — ammonia continues to strip `srcset` (Task 1's preserved-attrs check is `width`/`height` only); no proxy bypass introduced.
