# CSR Entries — Step B1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the entries-family list pages (`/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`) from Askama SSR to the shared CSR shell pattern by introducing a `<rdrs-entries-page>` custom element with `data-mode` switching.

**Architecture:** Each migrated page handler returns `(Flash, AppShellTemplate)` carrying only sidebar/flash bootstrap blobs — no per-page entry data. A single `<rdrs-entries-page>` element parses `location.pathname` to derive its mode (`unread`/`all`/`read`/`starred`/`summarized`), renders the list-pane header + reading-pane skeleton, and mounts an `<rdrs-entry-list>` configured for that mode. Existing `<rdrs-entry-list>` SSR-hydration paths remain dormant in B1 (they are deleted in B3) — first paint always issues exactly one `stream/contents` fetch instead of zero.

**Tech Stack:** Rust (axum + askama 0.13) + vanilla JS (native custom elements, no build step) + Playwright e2e.

**Spec:** [`docs/superpowers/specs/2026-05-07-csr-entries-design.md`](../specs/2026-05-07-csr-entries-design.md)

**Branch:** `refactor/csr-entries-list` (already cut from `main`, contains the spec commits).

**Environment:** Source `/tmp/rdrs-env.sh` before every `cargo`/`cargo nextest`/`npm`/Playwright invocation — without it OpenSSL link/runtime fails on this NixOS box.

---

## File Structure

| File | Status | Responsibility |
|------|--------|---------------|
| `static/js/pages/entries.js` | NEW | `<rdrs-entries-page>` element — 5 modes, header lookup, kb handlers, deep-link via `<rdrs-entry-list>` |
| `src/handlers/static_assets.rs` | EDIT | Register `js/pages/entries.js` in `FILES` allowlist |
| `src/handlers/pages.rs` | EDIT | Rewrite 5 handlers as `(Flash, AppShellTemplate)`; delete `EntriesTemplate`, `UnreadTemplate`, `EntriesArchiveTemplate` and their `IntoResponse` impls. The SSR helper functions (`fetch_entry_list_config*`, `fetch_entries_for_ssr*`, `entries_to_ssr`, `fetch_reading_pane_entry`, `SsrEntryView`, `SsrEntry`, `SsrReadingPaneEntry`, `EntryQuery`) **stay** for B2/B3 callers (feed, category, search) |
| `templates/entries.html` | DELETE | Replaced by shell |
| `templates/unread.html` | DELETE | Replaced by shell |
| `templates/entries_archive.html` | DELETE | Replaced by shell |
| `tests/pages_test.rs` | EDIT | Update tests for migrated routes; delete the 4 `*_contains_ssr_entries_json` tests for migrated routes |
| `e2e/tests/ssr-no-double-render.spec.ts` | EDIT | Block 1: change `count.toBe(0)` to `count.toBeLessThanOrEqual(1)` for the 4 measured routes (transitional — B3 tightens to `== 1`) |

`<rdrs-entry-list>`, `templates/macros.html`, `templates/feed_entries.html`, `templates/category_entries.html`, `templates/search.html` are **untouched** in B1 — their handlers still SSR.

---

## Task 1: Add `entries.js` skeleton and register it in the static-assets allowlist

The skeleton lets us register the script path before any handler points at it; the file just defines an empty custom element so `app_shell.html`'s `<script type="module" src="...entries.js">` parses cleanly.

**Files:**
- Create: `static/js/pages/entries.js`
- Modify: `src/handlers/static_assets.rs:8-59`
- Test: `tests/handlers_test.rs` (sanity GET) — likely already covers similar paths; add only if missing

- [ ] **Step 1: Create the skeleton element**

```js
// static/js/pages/entries.js
// <rdrs-entries-page> — CSR shell for the entries-family list pages.
// data-mode in {unread, all, read, starred, summarized, feed, category, search}
// is inferred from location.pathname on connect.

class RdrsEntriesPage extends HTMLElement {
    connectedCallback() {
        // TODO(Task 3): full implementation.
        this.innerHTML = '<p class="muted">Loading...</p>';
    }
}

customElements.define('rdrs-entries-page', RdrsEntriesPage);
```

- [ ] **Step 2: Add the file to the static-assets allowlist**

In `src/handlers/static_assets.rs`, the existing `FILES` array (lines 8-59) ends with the `admin.js` entry. Append before the closing `];`:

```rust
    (
        "js/pages/entries.js",
        include_str!("../../static/js/pages/entries.js"),
    ),
```

- [ ] **Step 3: Verify it compiles and serves**

```bash
source /tmp/rdrs-env.sh && cargo build
```

Expected: clean build (the new skeleton is now embedded). Confirm by grepping the binary or running:

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test handlers_test serve_static_assets 2>&1 | tail -20
```

Expected: existing tests still pass. (If a static-assets-listing test exists that asserts an exact set of files, update it to include the new entry.)

- [ ] **Step 4: Format**

```bash
cargo fmt
```

- [ ] **Step 5: Commit**

```bash
pwd  # /home/nixos/Develop/claude/rdrs
git add static/js/pages/entries.js src/handlers/static_assets.rs
git commit -S -m "$(cat <<'EOF'
feat(csr): scaffold rdrs-entries-page element + register asset

Empty skeleton so the static-assets allowlist accepts entries.js
before any page handler routes to it. Full element body lands in
the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Implement `<rdrs-entries-page>` with 5 modes

Build the full element body. Mode detection from `location.pathname`. Header lookup table. List-pane attribute table. Per-mode keyboard handlers. Reading-pane skeleton (the existing `<rdrs-entry-list>._loadEntryByIdInPane` handles `?entry=N` itself).

**Files:**
- Modify: `static/js/pages/entries.js` (replace skeleton with full impl)

- [ ] **Step 1: Replace the skeleton**

Overwrite the entire file with the full implementation. The element renders shell DOM once in `connectedCallback`, never again — `<rdrs-entry-list>` mutations stay scoped to its own subtree.

```js
// static/js/pages/entries.js
// <rdrs-entries-page> — CSR shell for the entries-family list pages.
//
// One element handles every list-mode route. data-mode is derived from
// location.pathname on connect:
//
//   /                       -> unread
//   /entries                -> all
//   /entries/read           -> read
//   /entries/starred        -> starred
//   /entries/summarized     -> summarized
//   (B2 will add feed + category, B3 will add search.)
//
// The element renders the shell once: sidebar (already mounted globally),
// flash (already mounted globally), main + split-view + list-pane header +
// list-pane body wrapping <rdrs-entry-list> + reading-pane skeleton. Once
// rendered, only <rdrs-entry-list> updates its own subtree on data reloads.
//
// Deep links (?entry=N) work via the existing fallback inside
// <rdrs-entry-list>._checkEntryParam → _loadEntryByIdInPane (which calls
// /reader/api/0/stream/items/contents). No deep-link logic at this layer.

import '/static/js/components/rdrs-entry-list.js';

const MARK_AS_READ_DROPDOWN = `
    <div class="form-group form-group-inline">
        <select id="mark-read-age" data-testid="mark-read-select" class="select-auto">
            <option value="">Mark as Read...</option>
            <option value="1">Older than 1 day</option>
            <option value="7">Older than 1 week</option>
            <option value="30">Older than 1 month</option>
            <option value="365">Older than 1 year</option>
            <option value="all">All entries</option>
        </select>
    </div>
`;

const TAB_BAR = `
    <div class="tab-bar">
        <a href="/entries" data-testid="tab-all" data-tab="all">All</a>
        <a href="/entries/read" data-testid="tab-read" data-tab="read">Read</a>
        <a href="/entries/starred" data-testid="tab-starred" data-tab="starred">Starred</a>
        <a href="/entries/summarized" data-testid="tab-summarized" data-tab="summarized">Summarized</a>
    </div>
`;

const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};

const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';
const READ_STATE = 'user/-/state/com.google/read';
const STARRED_STATE = 'user/-/state/com.google/starred';

const MODES = {
    unread: {
        title: 'Unread',
        navKey: 'unread',
        renderHeader: () => `<h1>Unread</h1><div class="filter-bar">${MARK_AS_READ_DROPDOWN}</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ xt: READ_STATE }),
            origin: 'unread',
            'show-feed': '',
            'show-category': '',
            'show-mark-above': '',
            'empty-message': 'No unread entries.',
        },
        kb: [
            { key: 'A', desc: 'Mark above as read', handle(list) { list.markAboveAsRead(); return true; } },
        ],
    },
    all: {
        title: 'Entries',
        navKey: 'entries',
        renderHeader: () => `<h1>Entries</h1>${TAB_BAR}<div class="filter-bar">${MARK_AS_READ_DROPDOWN}</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            origin: 'entries',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No entries found.',
        },
        kb: [
            { key: '1', desc: 'Go to All entries', handle: () => { location.href = '/entries'; return true; } },
            { key: '2', desc: 'Go to Read entries', handle: () => { location.href = '/entries/read'; return true; } },
            { key: '3', desc: 'Go to Starred entries', handle: () => { location.href = '/entries/starred'; return true; } },
            { key: '4', desc: 'Go to Summarized entries', handle: () => { location.href = '/entries/summarized'; return true; } },
        ],
    },
    read: {
        title: 'Read',
        navKey: 'entries',
        renderHeader: () => `<h1>Read</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ it: READ_STATE }),
            origin: 'read',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No read entries.',
        },
        kb: [
            { key: '1', desc: 'Go to All entries', handle: () => { location.href = '/entries'; return true; } },
            { key: '2', desc: 'Go to Read entries', handle: () => { location.href = '/entries/read'; return true; } },
            { key: '3', desc: 'Go to Starred entries', handle: () => { location.href = '/entries/starred'; return true; } },
            { key: '4', desc: 'Go to Summarized entries', handle: () => { location.href = '/entries/summarized'; return true; } },
        ],
    },
    starred: {
        title: 'Starred',
        navKey: 'starred',
        renderHeader: () => `<h1>Starred</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ it: STARRED_STATE }),
            origin: 'starred',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No starred entries.',
        },
        kb: [
            { key: '1', desc: 'Go to All entries', handle: () => { location.href = '/entries'; return true; } },
            { key: '2', desc: 'Go to Read entries', handle: () => { location.href = '/entries/read'; return true; } },
            { key: '3', desc: 'Go to Starred entries', handle: () => { location.href = '/entries/starred'; return true; } },
            { key: '4', desc: 'Go to Summarized entries', handle: () => { location.href = '/entries/summarized'; return true; } },
        ],
    },
    summarized: {
        title: 'Summarized',
        navKey: 'entries',
        renderHeader: () => `<h1>Summarized</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ has_summary: 'true' }),
            origin: 'summarized',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No summarized entries.',
        },
        kb: [
            { key: '1', desc: 'Go to All entries', handle: () => { location.href = '/entries'; return true; } },
            { key: '2', desc: 'Go to Read entries', handle: () => { location.href = '/entries/read'; return true; } },
            { key: '3', desc: 'Go to Starred entries', handle: () => { location.href = '/entries/starred'; return true; } },
            { key: '4', desc: 'Go to Summarized entries', handle: () => { location.href = '/entries/summarized'; return true; } },
        ],
    },
};

function inferMode() {
    const path = location.pathname;
    if (path === '/' || path === '') return 'unread';
    if (path === '/entries') return 'all';
    if (path === '/entries/read') return 'read';
    if (path === '/entries/starred') return 'starred';
    if (path === '/entries/summarized') return 'summarized';
    return 'unread';
}

function attrString(attrs) {
    return Object.entries(attrs)
        .map(([k, v]) => v === '' ? k : `${k}="${String(v).replace(/"/g, '&quot;')}"`)
        .join(' ');
}

class RdrsEntriesPage extends HTMLElement {
    connectedCallback() {
        const mode = inferMode();
        this.dataset.mode = mode;
        const cfg = MODES[mode];

        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="${cfg.navKey}"></rdrs-sidebar>
<main class="main-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <div class="split-view">
        <div class="list-pane">
            <div class="list-pane-header">${cfg.renderHeader()}</div>
            <div class="list-pane-body">
                <rdrs-entry-list ${attrString(cfg.listAttrs)} reading-pane="#reading-pane"></rdrs-entry-list>
            </div>
        </div>
        <div class="reading-pane" id="reading-pane">
            <div class="reading-pane-empty">Select an entry to read</div>
        </div>
    </div>
</main>
</div>`;

        this._wireMarkAsRead(mode);
        this._wireTabActive(mode);
        this._wireKeyboardHandlers(mode);
    }

    _wireMarkAsRead(mode) {
        const select = this.querySelector('#mark-read-age');
        if (!select) return;
        select.addEventListener('change', async () => {
            const age = select.value;
            select.selectedIndex = 0;
            if (!age) return;
            const ageLabel = AGE_LABELS[age] || age;
            if (!confirm(`Mark ${ageLabel} entries as read?`)) return;
            try {
                const body = new URLSearchParams();
                body.set('s', READING_LIST_STREAM);
                if (age !== 'all') {
                    const days = parseInt(age, 10);
                    const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
                    body.set('ts', tsUsec.toString());
                }
                const response = await fetch('/reader/api/0/mark-all-as-read', { method: 'POST', body });
                if (!response.ok) throw new Error('Failed to mark as read');
                window.flash && window.flash.success('Marked entries as read.');
                this.querySelector('rdrs-entry-list').loadEntries();
            } catch (err) {
                window.flash && window.flash.error(err.message);
            }
        });
    }

    _wireTabActive(mode) {
        const tabs = this.querySelectorAll('.tab-bar a[data-tab]');
        const activeTab = mode === 'all' ? 'all' : mode;
        tabs.forEach(a => {
            if (a.dataset.tab === activeTab) a.classList.add('active');
        });
    }

    _wireKeyboardHandlers(mode) {
        const cfg = MODES[mode];
        customElements.whenDefined('rdrs-entry-list').then(() => {
            const list = this.querySelector('rdrs-entry-list');
            if (!list || !cfg.kb || cfg.kb.length === 0) return;
            list.registerKeyboardHandlers({
                helpItems: cfg.kb.map(k => ({ key: k.key, desc: k.desc })),
                handleKey(key, shiftKey) {
                    const entry = cfg.kb.find(k => k.key === key);
                    if (!entry) return false;
                    return entry.handle(list);
                },
            });
        });
    }
}

customElements.define('rdrs-entries-page', RdrsEntriesPage);
```

- [ ] **Step 2: Build to verify the embedded source compiles**

```bash
source /tmp/rdrs-env.sh && cargo build
```

Expected: clean build (no Askama errors, no missing files).

- [ ] **Step 3: Quick browser sanity (optional, only if dev server already running)**

If no migrated handler exists yet, point a browser at `/admin` and confirm nothing breaks (entries.js shouldn't load on that page). Skipping the smoke test here is fine — Task 3 will exercise the full path.

- [ ] **Step 4: Commit**

```bash
git add static/js/pages/entries.js
git commit -S -m "$(cat <<'EOF'
feat(csr): implement rdrs-entries-page with 5 modes

Single custom element handles /, /entries, /entries/read,
/entries/starred, /entries/summarized via data-mode lookup. Mode
inferred from location.pathname; header / list-attributes / kb
handlers all driven by a per-mode config table. Deep links
(?entry=N) flow through the existing rdrs-entry-list fallback.

No handler is wired to this element yet — the next commits flip
each route from SSR template to the shared shell.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Migrate `unread_page` (`/`) to the CSR shell

The first handler flip — pattern to copy for the remaining four. After this commit `/` returns the shell + sidebar/flash bootstraps; the entries-list itself is fetched by JS.

**Files:**
- Modify: `src/handlers/pages.rs:597-647` (the `unread_page` body and the `UnreadTemplate` struct above it)
- Modify: `tests/pages_test.rs` (rewrite `test_unread_page_shows_unread_count`, `test_unread_page_while_masquerading`, delete `test_unread_page_contains_ssr_entries_json`)

- [ ] **Step 1: Read current handler shape** (already in plan context — handler is at `src/handlers/pages.rs:597`, returns `(Flash, UnreadTemplate)` with 19 fields populated from `fetch_entry_list_config`).

- [ ] **Step 2: Update the failing test first**

In `tests/pages_test.rs`, replace `test_unread_page_shows_unread_count` (around line 115) with:

```rust
#[tokio::test]
async fn test_unread_page_returns_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    // Shell shape — no SSR entry markup.
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    // SSR machinery for entries must be gone from this route.
    assert!(!body.contains(r#"class="ssr-entries""#));
    assert!(!body.contains(r#"class="ssr-reading-pane""#));
}
```

Update `test_unread_page_while_masquerading` (line 154) to assert shell presence + sidebar bootstrap shape rather than SSR markup. The body assertion now becomes:

```rust
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    // Sidebar bootstrap embeds the masqueraded user's view.
    assert!(body.contains(r#"id="rdrs-sidebar-bootstrap""#));
    // Admin nav is reachable because the masquerader is admin.
    assert!(body.contains(r#"data-testid="nav-admin""#));
```

Delete `test_unread_page_contains_ssr_entries_json` entirely (around line 799).

- [ ] **Step 3: Run the failing tests**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test test_unread_page 2>&1 | tail -20
```

Expected: failures (handler still returns the old SSR template).

- [ ] **Step 4: Replace the handler**

In `src/handlers/pages.rs`, find `pub async fn unread_page` (line 597) and replace the entire function body + the surrounding `UnreadTemplate` struct (located between the line ranges shown by `grep -n 'UnreadTemplate\|unread_page' src/handlers/pages.rs`) with:

```rust
/// Serves the CSR shell for `/` (unread). The list itself is loaded by
/// `<rdrs-entries-page>` (mode `unread`) from `/reader/api/0/stream/contents`.
pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Unread - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    )
}
```

Delete the `UnreadTemplate` struct and its `IntoResponse` impl (above the handler, also referenced from `EntryQuery` and friends — leave those, B2/B3 still need them).

- [ ] **Step 5: Build + run the migrated tests**

```bash
source /tmp/rdrs-env.sh && cargo build && cargo nextest run --test pages_test test_unread_page 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 6: Format + commit**

```bash
cargo fmt
pwd  # /home/nixos/Develop/claude/rdrs
git add src/handlers/pages.rs tests/pages_test.rs
git commit -S -m "$(cat <<'EOF'
refactor(csr): migrate / (unread) to CSR shell

Drops UnreadTemplate + SSR scaffolding for the unread route. The
list is now rendered by <rdrs-entries-page data-mode="unread">.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Migrate `entries_page`, `read_entries_page`, `starred_entries_page`, `summarized_entries_page`

Same shape as Task 3 × 4 — each handler collapses to the CSR shell. Test updates mirror the same pattern (assert `<rdrs-entries-page>` + script path; delete the corresponding `*_contains_ssr_entries_json` test).

**Files:**
- Modify: `src/handlers/pages.rs` (the four handlers + their `*Template` structs)
- Modify: `tests/pages_test.rs` (existing assertions for each route)

- [ ] **Step 1: Replace all four handlers**

For each of `entries_page`, `read_entries_page`, `starred_entries_page`, `summarized_entries_page` in `src/handlers/pages.rs`, replace the body with the CSR shell pattern. Title strings:

- `entries_page` → `"Entries - RDRS"`
- `read_entries_page` → `"Read Entries - RDRS"`
- `starred_entries_page` → `"Starred Entries - RDRS"`
- `summarized_entries_page` → `"Summarized Entries - RDRS"`

Each body is structurally identical to the `unread_page` body in Task 3, only the `title` literal changes. `element_tag` stays `"rdrs-entries-page"`, `script_path` stays `"/static/js/pages/entries.js"`.

Delete the `EntriesTemplate` struct (around line 770-797) and the `EntriesArchiveTemplate` struct (around line 921-960) along with their `IntoResponse` impls.

`EntryQuery` (the `?entry=N` extractor type) is **kept** — it's a no-op for these handlers now (we don't take it as a parameter), but B2's `feed_entries_page` and `category_entries_page` still extract it. Same for `fetch_entry_list_config` and the SSR helpers.

- [ ] **Step 2: Update the existing handler tests in `tests/pages_test.rs`**

Tests that need rewriting (same shell-shape assertion as Task 3):

- `test_entries_page_with_flash` (line 344)
- `test_read_entries_page` (line 610)
- `test_starred_entries_page` (line 622)

Tests that need deleting:

- `test_entries_page_contains_ssr_entries_json` (line 848)
- `test_read_entries_page_contains_ssr_entries_json` (line 956)
- `test_starred_entries_page_contains_ssr_entries_json` (line 992)

If a `test_summarized_entries_page*` test exists, apply the same treatment. Find any with:

```bash
grep -n "summarized_entries\|test_summarized" tests/pages_test.rs
```

Rewrite shell-shape assertion template (paste-and-tweak per route):

```rust
#[tokio::test]
async fn test_entries_page_returns_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
}
```

Repeat for `/entries/read`, `/entries/starred`, `/entries/summarized` (different test name + path).

- [ ] **Step 3: Run the migrated tests**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test 2>&1 | tail -40
```

Expected: pass for the 5 migrated routes; B2/B3 routes (`feed_entries`, `category_entries`, `search`) still SSR and still pass their existing tests untouched.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt
git add src/handlers/pages.rs tests/pages_test.rs
git commit -S -m "$(cat <<'EOF'
refactor(csr): migrate /entries family to CSR shell

/entries, /entries/read, /entries/starred, /entries/summarized now
return the shared shell. EntriesTemplate and EntriesArchiveTemplate
deleted along with their IntoResponse impls. fetch_entry_list_config
and friends remain — feed/category/search still SSR.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Delete obsolete templates

The five migrated routes no longer reference `entries.html`, `unread.html`, or `entries_archive.html`. Remove them so Askama doesn't try to compile dead templates.

**Files:**
- Delete: `templates/entries.html`
- Delete: `templates/unread.html`
- Delete: `templates/entries_archive.html`

- [ ] **Step 1: Verify no remaining Askama references**

```bash
grep -rn "entries.html\|unread.html\|entries_archive.html" src/ templates/ 2>&1
```

Expected: no results (the `#[template(path = "...")]` attributes were removed in Tasks 3 and 4 along with their structs).

- [ ] **Step 2: Delete the files**

```bash
git rm templates/entries.html templates/unread.html templates/entries_archive.html
```

- [ ] **Step 3: Build to confirm clean**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 4: Run full test suite**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -20
```

Expected: all tests pass (701 baseline + a handful changed in tasks 3-4).

- [ ] **Step 5: Commit**

```bash
git commit -S -m "$(cat <<'EOF'
refactor(csr): remove obsolete entries-family templates

entries.html, unread.html, entries_archive.html were the SSR
templates for /, /entries/*. Their handlers now return the shared
app_shell.html with rdrs-entries-page.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Reframe `ssr-no-double-render.spec.ts` block 1 to `<= 1`

Block 1 measures `stream/contents` requests on first paint. Pre-B1: all 4 measured routes were SSR-pre-rendered → 0 fetches. After B1: `/` and `/entries` (now CSR) fire 1 fetch; `/feeds/{id}/entries` and `/search?q=...` (still SSR) fire 0. The transitional `<= 1` assertion is green for both states. B3 tightens back to `== 1`.

**Files:**
- Modify: `e2e/tests/ssr-no-double-render.spec.ts:11-95`

- [ ] **Step 1: Update block 1 only**

Replace the four `expect(count).toBe(0);` lines in the first describe (lines 63-94, the four `test(...)` blocks) with `expect(count).toBeLessThanOrEqual(1);`. Update the describe-level docstring to reflect the transitional state:

```ts
test.describe("First paint fires at most one stream/contents fetch", () => {
  /* Transitional during B1/B2: routes that have moved to CSR fire 1 fetch,
   * routes still on SSR fire 0. B3 tightens this back to exactly 1 once every
   * list page is CSR. The original SSR-pre-render perf goal (issue #148) is
   * preserved as "no redundant fetch on top of the necessary one." */
  test.beforeAll(async ({ api, seed }) => { /* unchanged */ });
  // ...
  test("/ (unread)", async ({ page, serverUrl }) => {
    await login(page, serverUrl);
    const count = await gotoCounting(page, `${serverUrl}/`);
    expect(count).toBeLessThanOrEqual(1);
  });
  // …apply the same change to the /entries, /feeds/:id/entries, /search test bodies.
});
```

Block 2 (`Load More appends without duplicates...`) and block 3 (`Load More surfaces back-dated entries...`) are **unchanged**.

- [ ] **Step 2: Run the affected e2e specs**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test tests/ssr-no-double-render.spec.ts 2>&1 | tail -30
```

Expected: all green. (`/`, `/entries` now hit count == 1; `/feeds/:id/entries`, `/search?q=…` hit count == 0.)

- [ ] **Step 3: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/tests/ssr-no-double-render.spec.ts
git commit -S -m "$(cat <<'EOF'
test(csr): relax ssr-no-double-render block 1 to count <= 1

Transitional during the entries-family migration: routes that have
moved to CSR fire exactly 1 stream/contents call on first paint;
the rest still SSR-pre-render and fire 0. B3 will tighten this back
to == 1 once every list page is CSR.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Final verification + push + open PR

**Files:** none (this is the verification + delivery task).

- [ ] **Step 1: Full Rust suite**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -10
```

Expected: 701 ± a few tests pass (per memory, baseline was 701 after #174). Net change: 4 tests deleted (`*_contains_ssr_entries_json`), 5 tests rewritten (still pass).

- [ ] **Step 2: Format + clippy**

```bash
source /tmp/rdrs-env.sh && cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 3: Full e2e suite**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test 2>&1 | tail -30
```

Expected: all pass except for the known flake (`entry-actions.spec.ts :: keyboard s toggles star` — pre-existing on `main`, not introduced here).

- [ ] **Step 4: Push the branch**

```bash
cd /home/nixos/Develop/claude/rdrs
git push -u origin refactor/csr-entries-list
```

- [ ] **Step 5: Open the PR**

```bash
gh pr create --title "refactor(csr): migrate /entries family to CSR shell (B1)" --body "$(cat <<'EOF'
## Summary

Step 6 of the SSR-to-CSR migration, sub-PR B1 of 3. Migrates the entries-family list routes (`/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`) from Askama SSR to the shared `AppShellTemplate` introduced in #170, driven by a single new custom element `<rdrs-entries-page>` with a `data-mode` lookup.

**Spec:** [`docs/superpowers/specs/2026-05-07-csr-entries-design.md`](../blob/refactor/csr-entries-list/docs/superpowers/specs/2026-05-07-csr-entries-design.md)

**Plan:** [`docs/superpowers/plans/2026-05-07-csr-entries-step-b1.md`](../blob/refactor/csr-entries-list/docs/superpowers/plans/2026-05-07-csr-entries-step-b1.md)

## What changed

- New `static/js/pages/entries.js` — `<rdrs-entries-page>` element, 5 modes (`unread`/`all`/`read`/`starred`/`summarized`). Mode inferred from `location.pathname`.
- `src/handlers/static_assets.rs` — `entries.js` registered.
- `src/handlers/pages.rs` — 5 page handlers collapsed to `(Flash, AppShellTemplate)`. `UnreadTemplate`, `EntriesTemplate`, `EntriesArchiveTemplate` and their `IntoResponse` impls deleted. `fetch_entry_list_config` and the SSR helpers stay (B2 + B3 callers still use them).
- `templates/{entries,unread,entries_archive}.html` deleted.
- `tests/pages_test.rs` — assertions switched to shell-shape; SSR-JSON-content tests deleted.
- `e2e/tests/ssr-no-double-render.spec.ts` block 1 — transitional `<= 1` (B3 tightens to `== 1`).

## What did **not** change in B1

- `<rdrs-entry-list>` is untouched. Its SSR-hydration paths stay dormant for now and are removed in B3.
- `templates/macros.html`, `templates/feed_entries.html`, `templates/category_entries.html`, `templates/search.html` are untouched. Their handlers still SSR.
- `?entry=N` deep links continue to work via the existing `<rdrs-entry-list>._loadEntryByIdInPane` GReader fallback. The new `GET /api/entries/{id}` endpoint is deferred to B3 (see spec).

## Test plan

- [ ] `source /tmp/rdrs-env.sh && cargo nextest run` — green
- [ ] `source /tmp/rdrs-env.sh && cd e2e && npx playwright test` — green (modulo the pre-existing `entry-actions :: keyboard s toggles star` flake)
- [ ] Manually visit `/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized` — list renders, Load More works, click an entry → reading pane populates
- [ ] Manually visit `/?entry=<some id>` — reading pane deep-link still works (uses GReader fallback)
- [ ] Manually visit `/feeds/{id}/entries` and `/search` — still SSR, unchanged behavior

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 6: STOP — manual review**

Do **not** run `gh pr merge`. Surface the PR URL to the user and stop. The user will review, request changes if needed, and merge manually. After merge, delete the branch with `gh pr merge --squash --delete-branch` or `git push origin --delete refactor/csr-entries-list` (per CLAUDE.md, feature branches are squash-merged and deleted).

---

## Self-Review

**Spec coverage check:**

- B1 scope from spec → covered by Tasks 1-6 + verified in Task 7. ✓
- `entries.js` allowlist → Task 1. ✓
- `<rdrs-entries-page>` modes (5) → Task 2. ✓
- 5 handler migrations → Tasks 3-4. ✓
- Template deletion → Task 5. ✓
- `ssr-no-double-render.spec.ts` block 1 transitional → Task 6. ✓
- `GET /api/entries/{id}` deferred to B3 (per amended spec) → not in this plan, correct. ✓
- Test updates (`pages_test.rs`) → Tasks 3-4. ✓
- Final verification + PR → Task 7. ✓

**Placeholder scan:** None. Every code block contains the literal code to write. Every command has expected output described.

**Type / signature consistency:**

- `AppShellTemplate` field set used identically in Tasks 3 + 4 (matches `src/handlers/pages.rs:1649-1657`). ✓
- `<rdrs-entries-page>` mode keys (`unread`/`all`/`read`/`starred`/`summarized`) match between MODES table (Task 2) and `inferMode` helper (Task 2). ✓
- e2e block 1 stays at 4 routes (`/`, `/entries`, `/feeds/:id/entries`, `/search?q=`) — Task 6 updates exactly those four. ✓

**Risks pre-flagged in plan:**

- Task 1 mentions "If a static-assets-listing test exists that asserts an exact set of files, update it" — the agent should grep for this; if absent, no-op.
- Task 4 mentions hunting for any `test_summarized_entries_page*` — the agent runs grep to find it; original `pages_test.rs` lines 956 / 992 cover read/starred SSR-JSON tests, summarized variant might not exist.
- Task 6's transitional `<= 1` assertion is brittle if a future side-effect (e.g. periodic refresh) calls `stream/contents` post-paint within the 300ms wait — but block 1 already has that wait, so the risk is unchanged from current.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-07-csr-entries-step-b1.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
