# SSR-first PR-12 — Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the legacy CSR scaffolding now that all 14 logged-in routes are SSR (PRs 1-11). Remove unused JS files, unused JSON endpoints, and CSR-specific e2e specs.

**Architecture:** Pure deletion + comment scrub. No new code, no behavior change. Touches three layers: static JS assets (delete files, remove from `static_assets.rs` allowlist, remove from `app_layout.html`), server (delete handlers + routes + tests), e2e (delete `test.fixme`-quarantined specs).

**Tech Stack:** Rust + Axum (handlers/routes), Playwright (e2e), vanilla-JS custom elements.

---

## Background — what stays vs goes

**Stays (chrome shared by all SSR pages):**
- `static/js/app.js` — the only logged-in page-script (sidebar polling, keyboard, swap helper, etc.)
- `static/js/passkey.js` — WebAuthn JS exception for `/user-settings`
- `static/js/utils.js` — `escapeHtml` is imported by `passkey.js`
- `static/js/components/rdrs-sidebar.js`, `rdrs-flash.js`, `rdrs-kb-help.js`, `rdrs-kb-pending.js`

**Goes:**
- `static/js/keyboard.js`
- `static/js/components/rdrs-entry-list.js`
- `static/js/pages/entries.js` (and its parent dir, which becomes empty)
- `GET /api/entries/{id}` (`handlers::entry::get_entry_detail`) — last consumer was `<rdrs-entry-list>._loadEntryByIdInPane`
- `GET /api/feeds` (`handlers::feeds::list_feeds`) — last consumer was `static/js/pages/entries.js:88` feed-icon column
- All `test.fixme(...)` tests in the e2e suite (37 across 9 files — 8 whole files, plus 8 individual tests in `responsive.spec.ts`)
- Stale code comments referencing the above

**Note:** The spec § Migration plan row 12 says "tighten allowlist to `app.css` + `app.js` + `passkey.js`". That was written before the sidebar/flash/kb custom elements were locked in as the SSR shell chrome. The realistic minimum is: app.css, app.js, passkey.js, utils.js, and the four kept components. Confirmed by reading the four components and verifying `app_layout.html` still mounts each.

---

## File Structure (what touches what)

| File | Action |
|---|---|
| `static/js/keyboard.js` | DELETE |
| `static/js/components/rdrs-entry-list.js` | DELETE |
| `static/js/pages/entries.js` | DELETE (plus rmdir `pages/`) |
| `src/handlers/static_assets.rs` | Drop 3 entries from `FILES` |
| `templates/app_layout.html:10-11` | Drop two `<script>` tags |
| `src/lib.rs:73-76` | Drop `/api/feeds` route + the explanatory comment |
| `src/lib.rs:170-171, 223` | Drop `/api/entries/{id}` route + comment |
| `src/handlers/entry.rs` | Delete `get_entry_detail` fn (around line 382-end-of-fn) |
| `src/handlers/feeds.rs` | Delete `list_feeds` fn + its imports/preamble comments (lines 12-?, ~60+) |
| `tests/handlers_test.rs` | Delete `test_list_feeds_empty`, `test_list_feeds_unauthorized` |
| `tests/entry_handlers_test.rs` | Delete `test_get_entry_detail_owner`, `test_get_entry_detail_not_owner`, `test_get_entry_detail_missing` |
| `static/js/app.js` | Scrub stale comments mentioning keyboard.js / rdrs-entry-list / pages/entries.js |
| `static/js/components/rdrs-sidebar.js` | Scrub stale comments |
| `src/handlers/entries.rs` | Scrub stale comments (lines 68, 488, 545) |
| `src/handlers/feeds.rs` | Scrub stale preamble comment (line 16) |
| `e2e/tests/entry-detail.spec.ts` | DELETE whole file (3/3 fixme) |
| `e2e/tests/entry-navigation.spec.ts` | DELETE whole file (4/4 fixme) |
| `e2e/tests/entry-actions.spec.ts` | DELETE whole file (4/4 fixme) |
| `e2e/tests/global-navigation.spec.ts` | DELETE whole file (3/3 fixme) |
| `e2e/tests/ssr-no-double-render.spec.ts` | DELETE whole file (11/11 fixme — CSR fetch-and-fill anti-regression, no longer applicable) |
| `e2e/tests/sidebar-flicker.spec.ts` | DELETE whole file (1/1 fixme) |
| `e2e/tests/sidebar-active-category.spec.ts` | DELETE whole file (2/2 fixme) |
| `e2e/tests/keyboard-help.spec.ts` | DELETE whole file (1/1 fixme) |
| `e2e/tests/responsive.spec.ts` | Delete only the 8 `test.fixme(...)` blocks, keep the 6 live tests |

---

## Tasks

### Task 1: Delete legacy JS files + their script tags + allowlist entries

**Files:**
- Delete: `static/js/keyboard.js`
- Delete: `static/js/components/rdrs-entry-list.js`
- Delete: `static/js/pages/entries.js`
- Delete: `static/js/pages/` (the now-empty parent dir)
- Modify: `src/handlers/static_assets.rs` (drop 3 entries from the `FILES` array)
- Modify: `templates/app_layout.html` (drop two `<script>` tags at lines 10-11)

- [ ] **Step 1: Run `cargo check` to confirm clean baseline**

```
source /tmp/rdrs-env.sh && cargo check
```
Expected: `Finished` with zero warnings/errors from our crate.

- [ ] **Step 2: Delete the three JS files and the now-empty `pages/` dir**

```
rm static/js/keyboard.js
rm static/js/components/rdrs-entry-list.js
rm static/js/pages/entries.js
rmdir static/js/pages
```

- [ ] **Step 3: Drop the three matching entries from `src/handlers/static_assets.rs` `FILES` array**

Remove these three tuples (and their surrounding `(...)` wrappers):
- `("js/keyboard.js", include_str!("../../static/js/keyboard.js"))`
- `("js/components/rdrs-entry-list.js", include_str!("../../static/js/components/rdrs-entry-list.js"))`
- `("js/pages/entries.js", include_str!("../../static/js/pages/entries.js"))`

After the edit, the `FILES` array should contain exactly these 8 entries (order may be reorganized for readability):
- `css/app.css`
- `js/utils.js`
- `js/app.js`
- `js/passkey.js`
- `js/components/rdrs-flash.js`
- `js/components/rdrs-kb-help.js`
- `js/components/rdrs-kb-pending.js`
- `js/components/rdrs-sidebar.js`

- [ ] **Step 4: Drop the two `<script type="module">` tags from `templates/app_layout.html`**

Lines 10-11 currently:
```html
    <script type="module" src="/static/js/keyboard.js?v={{ layout.git_version }}"></script>
    <script type="module" src="/static/js/components/rdrs-entry-list.js?v={{ layout.git_version }}"></script>
```
Delete both lines.

- [ ] **Step 5: Verify build is still clean**

```
source /tmp/rdrs-env.sh && cargo check
```
Expected: `Finished` with zero errors. (Warnings about unused imports/items in handlers are expected — those land in Tasks 2-3.)

- [ ] **Step 6: Commit**

```
git add static/js src/handlers/static_assets.rs templates/app_layout.html
git commit -S -m "$(cat <<'EOF'
chore(cleanup): delete legacy CSR JS files

Remove keyboard.js, components/rdrs-entry-list.js, pages/entries.js
(the CSR scaffolding for the entries family). Their last consumers
went SSR in PRs 10-11. Drop their script tags from app_layout.html
and prune the static_assets.rs allowlist.
EOF
)"
```

---

### Task 2: Delete `GET /api/entries/{id}` (handler + route + tests)

**Files:**
- Modify: `src/lib.rs` (drop the `.route("/api/entries/{id}", get(...))` line + nearby comment at lines 170-171)
- Modify: `src/handlers/entry.rs` (delete `pub async fn get_entry_detail` and its preamble comment at line 382)
- Modify: `tests/entry_handlers_test.rs` (delete `test_get_entry_detail_owner`, `test_get_entry_detail_not_owner`, `test_get_entry_detail_missing` — lines 2310-end of each)

- [ ] **Step 1: Confirm there are no other consumers of `get_entry_detail`**

```
grep -rn "get_entry_detail" src/ tests/
```
Expected: matches only in the three locations above (handler def, route, three tests).

- [ ] **Step 2: Delete the route line and any explanatory comment in `src/lib.rs`**

Lines 170-171 contain a comment "JSON /api/entries/{id} stays alive until..." — delete that comment AND the route line `.route("/api/entries/{id}", get(handlers::entry::get_entry_detail))` (line 223). Make sure the surrounding routes still chain correctly (no stray dangling dots).

- [ ] **Step 3: Delete `get_entry_detail` from `src/handlers/entry.rs`**

The handler starts with the comment block at line 382 (`// GET /api/entries/{id} — reading-pane deep link`) and continues through the function body (around line 404+). Delete the whole block including any directly-preceding doc comments. Don't delete unrelated handlers around it.

After the edit, run `grep -n "get_entry_detail\|GET /api/entries/{id}" src/handlers/entry.rs` and expect no matches.

- [ ] **Step 4: Delete the three tests in `tests/entry_handlers_test.rs`**

Tests to remove (use the line numbers as a starting point, but identify them by name and `#[tokio::test]` block):
- `test_get_entry_detail_owner` (~line 2310)
- `test_get_entry_detail_not_owner` (~line 2333)
- `test_get_entry_detail_missing` (~line 2349)

Make sure the `#[tokio::test]` attribute immediately above each test is also removed. Don't delete the test directly after the last removed one.

- [ ] **Step 5: Build and test**

```
source /tmp/rdrs-env.sh && cargo check
source /tmp/rdrs-env.sh && cargo nextest run --no-fail-fast
```
Expected: `cargo check` clean. `cargo nextest run` green; the three removed tests should no longer appear in the output.

- [ ] **Step 6: Commit**

```
git add src/lib.rs src/handlers/entry.rs tests/entry_handlers_test.rs
git commit -S -m "$(cat <<'EOF'
chore(cleanup): drop GET /api/entries/{id}

The JSON detail endpoint was a deep-link entry-loader for
<rdrs-entry-list>. The reading-pane now ships SSR via
GET /entries/{id}/fragment.
EOF
)"
```

---

### Task 3: Delete `GET /api/feeds` (handler + route + tests)

**Files:**
- Modify: `src/lib.rs` (drop the comment "GET /api/feeds is still consumed by..." at line 73 and the route line at line 76)
- Modify: `src/handlers/feeds.rs` (delete `pub async fn list_feeds` + its preamble comment around line 12-16, line 60+)
- Modify: `tests/handlers_test.rs` (delete `test_list_feeds_empty`, `test_list_feeds_unauthorized`)

- [ ] **Step 1: Confirm no other consumers of `list_feeds`**

```
grep -rn "list_feeds" src/ tests/
```
Expected: only the handler def, the route, and the two test fns.

- [ ] **Step 2: Delete the route + comment in `src/lib.rs`**

Lines 73-76: remove both the explanatory comment and the route line.

- [ ] **Step 3: Delete `list_feeds` from `src/handlers/feeds.rs`**

The preamble comment around line 16 ("`GET /api/feeds` is the JSON shape that `static/js/pages/entries.js` reads ...") should be deleted along with the `list_feeds` fn body (around line 60+). Don't delete unrelated handlers/structs in the file.

If `list_feeds` was the only user of a struct or import (e.g. a JSON response type only it returned), remove that too. Watch for unused-import warnings from `cargo check` and clean them up.

- [ ] **Step 4: Delete the two tests in `tests/handlers_test.rs`**

- `test_list_feeds_empty` (~line 312)
- `test_list_feeds_unauthorized` (~line 324)

Remove both, along with their `#[tokio::test]` attributes.

- [ ] **Step 5: Build and test**

```
source /tmp/rdrs-env.sh && cargo check
source /tmp/rdrs-env.sh && cargo nextest run --no-fail-fast
```
Expected: clean check, green tests, no warnings.

- [ ] **Step 6: Commit**

```
git add src/lib.rs src/handlers/feeds.rs tests/handlers_test.rs
git commit -S -m "$(cat <<'EOF'
chore(cleanup): drop GET /api/feeds JSON endpoint

The JSON feed list was consumed by static/js/pages/entries.js for
the feed-icon column. SSR entries pages now render feed icons via
templates/_entry_row.html + GET /api/feeds/{id}/icon directly.
EOF
)"
```

---

### Task 4: Scrub stale comments referencing deleted code

**Files:**
- Modify: `static/js/app.js` — lines 9, 251, 479 reference legacy keyboard/page modules / `<rdrs-entry-list>`
- Modify: `static/js/components/rdrs-sidebar.js` — lines 12, 51 reference `pages/entries.js`
- Modify: `src/handlers/entries.rs` — lines 68, 488, 545 reference `<rdrs-entry-list>` / JSON detail endpoint
- (other comments naturally went away with Tasks 1-3)

- [ ] **Step 1: Locate and edit each comment**

For each location, replace the legacy-referencing text with concise wording that still explains WHY the code exists, or delete the comment entirely if the WHY is now obvious from the SSR architecture. Don't delete code, only comments.

Per `/home/nixos/.claude/CLAUDE.md` (default-no-comments + WHY-only): if the comment is not naming a hidden constraint, just delete it.

- [ ] **Step 2: Sanity check — grep for any remaining stale references**

```
grep -rEn "pages/entries\.js|rdrs-entry-list|rdrs-entries-page|keyboard\.js" src/ static/ templates/
```
Expected: zero matches. (If e2e specs still reference them, those go in Task 5.)

- [ ] **Step 3: Build is unchanged (no code edits)**

```
source /tmp/rdrs-env.sh && cargo check
```
Expected: clean.

- [ ] **Step 4: Commit**

```
git add src/handlers/entries.rs static/js/app.js static/js/components/rdrs-sidebar.js
git commit -S -m "$(cat <<'EOF'
chore(cleanup): scrub stale comments referencing CSR scaffolding
EOF
)"
```

---

### Task 5: Drop e2e specs / blocks that are fully `test.fixme`-quarantined

**Files:**
- Delete: `e2e/tests/entry-detail.spec.ts` (3/3 fixme)
- Delete: `e2e/tests/entry-navigation.spec.ts` (4/4 fixme)
- Delete: `e2e/tests/entry-actions.spec.ts` (4/4 fixme)
- Delete: `e2e/tests/global-navigation.spec.ts` (3/3 fixme)
- Delete: `e2e/tests/ssr-no-double-render.spec.ts` (11/11 fixme — anti-CSR-fetch-and-fill, no longer applicable)
- Delete: `e2e/tests/sidebar-flicker.spec.ts` (1/1 fixme)
- Delete: `e2e/tests/sidebar-active-category.spec.ts` (2/2 fixme)
- Delete: `e2e/tests/keyboard-help.spec.ts` (1/1 fixme)
- Modify: `e2e/tests/responsive.spec.ts` — delete only the 8 `test.fixme(...)` blocks, keep the 6 live tests

- [ ] **Step 1: Delete the 8 fully-fixme'd spec files**

```
rm e2e/tests/entry-detail.spec.ts
rm e2e/tests/entry-navigation.spec.ts
rm e2e/tests/entry-actions.spec.ts
rm e2e/tests/global-navigation.spec.ts
rm e2e/tests/ssr-no-double-render.spec.ts
rm e2e/tests/sidebar-flicker.spec.ts
rm e2e/tests/sidebar-active-category.spec.ts
rm e2e/tests/keyboard-help.spec.ts
```

- [ ] **Step 2: Edit `responsive.spec.ts` to drop the 8 `test.fixme(...)` blocks**

Open the file, identify each `test.fixme("...", async ...)` block, and delete the entire block (function declaration through closing `});`). Keep any `test.describe` wrappers if they still contain live tests after the deletions. If a `describe` becomes empty, delete that too.

Verify after editing: `grep -c "test\.fixme" e2e/tests/responsive.spec.ts` should be 0.

- [ ] **Step 3: Confirm no remaining `test.fixme` anywhere**

```
grep -rEn "test\.fixme" e2e/
```
Expected: zero matches.

- [ ] **Step 4: Run the trimmed e2e suite (Chromium only, fast)**

```
source /tmp/rdrs-env.sh && cd e2e && pnpm exec playwright test --project=chromium 2>&1 | tail -30
```
Expected: all remaining tests pass. If anything in `responsive.spec.ts` fails because shared `beforeAll`/`beforeEach` state was removed alongside a fixme'd test, fix it.

- [ ] **Step 5: Commit**

```
git add e2e/tests/
git commit -S -m "$(cat <<'EOF'
chore(cleanup): drop test.fixme e2e specs (CSR-specific)

37 tests across 9 files were quarantined during the SSR-first
migration (PRs 1-11). They tested CSR-specific behaviors that no
longer exist (fetch-and-fill double-render anti-regression, route
nav without page reload, etc.). Delete 8 whole files and prune the
8 fixme'd tests from responsive.spec.ts.
EOF
)"
```

---

### Task 6: Push, open PR, watch CI, merge

- [ ] **Step 1: Final sanity sweep**

```
source /tmp/rdrs-env.sh && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --no-fail-fast
```
Expected: all green.

- [ ] **Step 2: Manual browser smoke**

Start dev server, log in, click through `/`, `/entries`, `/feeds/{id}/entries`, `/categories/{id}/entries`, `/search`, `/feeds`, `/settings`, `/admin`, `/user-settings`. Verify no JS console errors (especially no 404s for deleted assets, no failed fetches for `/api/entries/{id}` or `/api/feeds`).

- [ ] **Step 3: Push branch**

```
git push -u origin chore/12-cleanup-csr-scaffolding
```

- [ ] **Step 4: Open PR via `gh pr create`**

Title: `chore: SSR-first PR-12 — cleanup CSR scaffolding`. Body: brief summary + per-task bullets.

- [ ] **Step 5: Watch CI, merge after green**

```
gh pr checks --watch
```
Once green, squash-merge and delete the source branch.

---

## Self-Review

- All 14 SSR routes verified to NOT depend on any deleted JS/handler? Yes — sidebar/flash/kb chrome + passkey JS are kept; the only consumers of the deleted code were the entries family pages (now SSR).
- `static_assets.rs` allowlist still contains exactly the files mounted by `app_layout.html`? Tasks 1+3+4 enforce this.
- No dangling references? Task 4 grep catches it.
- E2e suite still meaningful? `responsive.spec.ts` keeps 6 live tests; live coverage elsewhere is unchanged.
