# CSR Entries — Step B2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/feeds/{id}/entries` and `/categories/{id}/entries` from Askama SSR to the shared `<rdrs-entries-page>` shell by adding `feed` and `category` modes, completing 8 of the 8 list routes' CSR migration except `/search` (which is B3).

**Architecture:** `<rdrs-entries-page>` already infers its mode from `location.pathname`. B2 extends the path matcher to handle `/feeds/{id}/entries` and `/categories/{id}/entries`, the `MODES` lookup gains two entries with `data-mode="feed"` / `data-mode="category"`, and the page handlers collapse to the standard `(Flash, AppShellTemplate)` shape with the `Path(id)` ownership verification preserved (404 on foreign id).

**Tech Stack:** Rust (axum + askama 0.13) + vanilla JS (native custom elements, no build step) + Playwright e2e.

**Spec:** [`docs/superpowers/specs/2026-05-07-csr-entries-design.md`](../specs/2026-05-07-csr-entries-design.md)

**Predecessor PR:** #175 (B1, merged 2026-05-07).

**Branch:** `refactor/csr-feed-category-entries` (already cut from current `main`).

**Environment:** Source `/tmp/rdrs-env.sh` before every `cargo` / `cargo nextest` / `npm` / Playwright invocation.

---

## File Structure

| File | Status | Responsibility |
|------|--------|---------------|
| `static/js/pages/entries.js` | EDIT | Add `feed` + `category` to `inferMode` and `MODES`. Each new mode renders breadcrumb + filter + mark-as-read header, fetches the right meta (feed: `GET /api/feeds` then plucks by id; category: read from inlined sidebar bootstrap), and applies `?status=` from URL on first paint. |
| `src/handlers/pages.rs` | EDIT | `feed_entries_page` and `category_entries_page` collapse to `Result<(Flash, AppShellTemplate), AppError>` keeping the existing `Path(id)` + ownership checks. `FeedEntriesTemplate`, `CategoryEntriesTemplate` and their `IntoResponse` impls deleted. |
| `templates/feed_entries.html` | DELETE | Replaced by shell. |
| `templates/category_entries.html` | DELETE | Replaced by shell. |
| `tests/pages_test.rs` | EDIT | Update `test_feed_entries_page`, `test_category_entries_page`, `test_feed_entries_page_other_user`, `test_feed_entries_page_not_found` to assert shell shape. Delete `test_feed_entries_page_contains_ssr_json`, `test_category_entries_page_contains_ssr_json`. |

`<rdrs-entry-list>`, `templates/macros.html`, `templates/search.html`, `e2e/tests/ssr-no-double-render.spec.ts` are **untouched**. Block 1's transitional `count <= 1` continues to hold (after B2: only `/search` still SSR with count=0, all four CSR routes count=1).

No new HTTP endpoint in B2 — the page module reuses the existing `GET /api/feeds` (filters by id client-side) and reads category metadata from the sidebar bootstrap blob already inlined in `app_shell.html`.

---

## Task 1: Extend `<rdrs-entries-page>` with `feed` + `category` modes

This task is JS-only. After it lands, the next two handler-migration tasks just flip the routes; the element is ready to receive them.

**Files:**
- Modify: `static/js/pages/entries.js`

- [ ] **Step 1: Add path parsing for the two new routes**

In `static/js/pages/entries.js`, replace `inferMode()` with:

```js
function inferMode() {
    const path = location.pathname;
    if (path === '/' || path === '') return 'unread';
    if (path === '/entries') return 'all';
    if (path === '/entries/read') return 'read';
    if (path === '/entries/starred') return 'starred';
    if (path === '/entries/summarized') return 'summarized';
    const feedMatch = path.match(/^\/feeds\/(\d+)\/entries$/);
    if (feedMatch) return 'feed';
    const catMatch = path.match(/^\/categories\/(\d+)\/entries$/);
    if (catMatch) return 'category';
    return 'unread';
}

function pathId() {
    const m = location.pathname.match(/^\/(?:feeds|categories)\/(\d+)\/entries$/);
    return m ? parseInt(m[1], 10) : null;
}
```

- [ ] **Step 2: Add helpers at the top of the file (after the `READING_LIST_STREAM` constants block)**

```js
const FILTER_STATUS_DROPDOWN = `
    <div class="form-group form-group-inline">
        <select id="filter-status" data-testid="filter-status" class="select-auto">
            <option value="">All</option>
            <option value="unread">Unread</option>
            <option value="read">Read</option>
            <option value="starred">Starred</option>
        </select>
    </div>
`;

function statusToApiParams(status) {
    if (status === 'unread') return { xt: READ_STATE };
    if (status === 'read') return { it: READ_STATE };
    if (status === 'starred') return { it: STARRED_STATE };
    return {};
}

function readSidebarBootstrap() {
    const el = document.getElementById('rdrs-sidebar-bootstrap');
    if (!el) return null;
    try { return JSON.parse(el.textContent); } catch { return null; }
}

async function fetchFeedMeta(feedId) {
    const res = await fetch('/api/feeds');
    if (!res.ok) return null;
    const data = await res.json();
    return data.feeds.find(f => f.id === feedId) || null;
}
```

- [ ] **Step 3: Add `feed` and `category` entries to the `MODES` lookup table**

Append to `MODES` (before the closing `};`):

```js
    feed: {
        title: 'Feed',
        navKey: 'feeds',
        // Async — overridden in connectedCallback once meta arrives.
        renderHeader: () => `<h1>Loading…</h1>`,
        // Constructed with stream-id at runtime; the static placeholder
        // here is replaced by _mountFeedEntryList.
        listAttrs: null,
        kb: [
            { key: '1', desc: 'Show all entries', handle: (list, page) => { page._setStatus(''); return true; } },
            { key: '2', desc: 'Show unread only', handle: (list, page) => { page._setStatus('unread'); return true; } },
            { key: '3', desc: 'Show read only', handle: (list, page) => { page._setStatus('read'); return true; } },
            { key: '4', desc: 'Show starred only', handle: (list, page) => { page._setStatus('starred'); return true; } },
            { key: 'A', desc: 'Mark above as read', handle: (list) => { list.markAboveAsRead(); return true; } },
            { key: 'c', desc: 'Go to category page', handle: (list, page) => { if (page._categoryId) location.href = `/categories/${page._categoryId}/entries`; return true; } },
            { key: 'x', desc: 'Go to category page', handle: (list, page) => { if (page._categoryId) location.href = `/categories/${page._categoryId}/entries`; return true; } },
        ],
    },
    category: {
        title: 'Category',
        navKey: 'category',
        renderHeader: () => `<h1>Loading…</h1>`,
        listAttrs: null,
        kb: [
            { key: '1', desc: 'Show all entries', handle: (list, page) => { page._setStatus(''); return true; } },
            { key: '2', desc: 'Show unread only', handle: (list, page) => { page._setStatus('unread'); return true; } },
            { key: '3', desc: 'Show read only', handle: (list, page) => { page._setStatus('read'); return true; } },
            { key: '4', desc: 'Show starred only', handle: (list, page) => { page._setStatus('starred'); return true; } },
            { key: 'A', desc: 'Mark above as read', handle: (list) => { list.markAboveAsRead(); return true; } },
            { key: 'x', desc: 'Go to unread page', handle: () => { location.href = '/'; return true; } },
        ],
    },
```

- [ ] **Step 4: Branch in `connectedCallback`**

Replace the entire body of `RdrsEntriesPage.connectedCallback` with:

```js
    connectedCallback() {
        const mode = inferMode();
        this.dataset.mode = mode;
        const cfg = MODES[mode];

        if (mode === 'feed' || mode === 'category') {
            this._connectAsync(mode, cfg);
            return;
        }

        // Static-mode flow (unread / all / read / starred / summarized) —
        // unchanged from B1.
        const attrs = { ...cfg.listAttrs, 'no-auto-load': '' };
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="${cfg.navKey}"></rdrs-sidebar>
<main class="main-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <div class="split-view">
        <div class="list-pane">
            <div class="list-pane-header">${cfg.renderHeader()}</div>
            <div class="list-pane-body">
                <rdrs-entry-list ${attrString(attrs)} reading-pane="#reading-pane"></rdrs-entry-list>
            </div>
        </div>
        <div class="reading-pane" id="reading-pane">
            <div class="reading-pane-empty">Select an entry to read</div>
        </div>
    </div>
</main>
</div>`;

        this._wireMarkAsRead();
        this._wireTabActive(mode);
        this._wireKeyboardHandlers(mode);
        this._loadAndStart();
    }
```

- [ ] **Step 5: Implement `_connectAsync` for feed + category**

Add as a method on `RdrsEntriesPage` (near `_loadAndStart`):

```js
    /// feed/category modes resolve their stream-id and breadcrumb data
    /// asynchronously. Render a placeholder shell first so sidebar +
    /// flash paint immediately, then await meta and mount the entry-list.
    async _connectAsync(mode, cfg) {
        const id = pathId();
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="${cfg.navKey}"></rdrs-sidebar>
<main class="main-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <div class="split-view">
        <div class="list-pane">
            <div class="list-pane-header" id="list-pane-header"><h1>Loading…</h1></div>
            <div class="list-pane-body" id="list-pane-body"></div>
        </div>
        <div class="reading-pane" id="reading-pane">
            <div class="reading-pane-empty">Select an entry to read</div>
        </div>
    </div>
</main>
</div>`;

        const initialStatus = new URLSearchParams(location.search).get('status') || 'unread';

        let streamId, headerHtml;
        if (mode === 'feed') {
            const meta = await fetchFeedMeta(id);
            if (!meta) {
                this.querySelector('#list-pane-header').innerHTML = `<h1>Feed not found</h1>`;
                return;
            }
            this._feedId = id;
            this._categoryId = meta.category_id;
            this._feedUrl = meta.url;
            this._feedTitle = meta.title;
            streamId = `feed/${meta.url}`;
            const iconImg = meta.has_icon
                ? `<img src="/api/feeds/${id}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">`
                : '';
            headerHtml = `
<div class="breadcrumb">
    <a href="/feeds">Feeds</a> / <a href="/categories/${meta.category_id}/entries">${escapeHtmlInline(meta.category_name)}</a> / ${escapeHtmlInline(meta.title)}
</div>
<h1>${iconImg}${escapeHtmlInline(meta.title)}</h1>
<div class="filter-bar">
    ${FILTER_STATUS_DROPDOWN}
    ${MARK_AS_READ_DROPDOWN}
</div>`;
        } else {
            const sidebar = readSidebarBootstrap();
            const cat = sidebar?.categories?.find(c => c.id === id);
            if (!cat) {
                this.querySelector('#list-pane-header').innerHTML = `<h1>Category not found</h1>`;
                return;
            }
            this._categoryId = id;
            this._categoryName = cat.name;
            streamId = `user/-/label/${cat.name}`;
            headerHtml = `
<div class="breadcrumb">
    <a href="/categories">Categories</a> / ${escapeHtmlInline(cat.name)}
</div>
<h1>${escapeHtmlInline(cat.name)}</h1>
<div class="filter-bar">
    ${FILTER_STATUS_DROPDOWN}
    ${MARK_AS_READ_DROPDOWN}
</div>`;
        }

        this._streamId = streamId;
        this.querySelector('#list-pane-header').innerHTML = headerHtml;

        const attrs = {
            'stream-id': streamId,
            origin: mode,
            'show-feed': '',
            ...(mode === 'category' ? { 'show-category': '' } : {}),
            'show-mark-above': '',
            'no-auto-load': '',
            'empty-message': 'No entries found.',
        };
        const body = this.querySelector('#list-pane-body');
        body.innerHTML = `<rdrs-entry-list ${attrString(attrs)} reading-pane="#reading-pane"></rdrs-entry-list>`;

        this._currentStatus = initialStatus;
        this.querySelector('#filter-status').value = initialStatus;
        this._wireFilterStatus();
        this._wireMarkAsReadStream(streamId, mode);
        this._wireKeyboardHandlers(mode);

        const list = this.querySelector('rdrs-entry-list');
        try {
            const res = await fetch('/api/user-settings');
            if (res.ok) {
                const settings = await res.json();
                if (settings.entries_per_page) list.setAttribute('entries-per-page', String(settings.entries_per_page));
                if (settings.linkding_configured) list.setAttribute('has-save-services', '');
                if (settings.kagi_configured) list.setAttribute('has-kagi-configured', '');
            }
        } catch { /* defaults */ }
        list.setApiParams(statusToApiParams(initialStatus));
        list.loadEntries();
    }

    _setStatus(status) {
        this._currentStatus = status;
        const select = this.querySelector('#filter-status');
        if (select) select.value = status;
        const params = new URLSearchParams();
        if (status) params.set('status', status);
        const url = location.pathname + (params.toString() ? '?' + params.toString() : '');
        history.replaceState(null, '', url);
        const list = this.querySelector('rdrs-entry-list');
        if (list) {
            list.setApiParams(statusToApiParams(status));
            list.loadEntries();
        }
    }

    _wireFilterStatus() {
        const select = this.querySelector('#filter-status');
        if (!select) return;
        select.addEventListener('change', () => this._setStatus(select.value));
    }

    /// Stream-scoped mark-as-read for feed / category modes. Posts to
    /// /reader/api/0/mark-all-as-read with `s=<streamId>` and an optional
    /// `ts=` cutoff in microseconds.
    _wireMarkAsReadStream(streamId, mode) {
        const select = this.querySelector('#mark-read-age');
        if (!select) return;
        const scopeLabel = mode === 'feed' ? 'this feed' : 'this category';
        select.addEventListener('change', async () => {
            const age = select.value;
            select.selectedIndex = 0;
            if (!age) return;
            const ageLabel = AGE_LABELS[age] || age;
            if (!confirm(`Mark ${ageLabel} entries in ${scopeLabel} as read?`)) return;
            try {
                const body = new URLSearchParams();
                body.set('s', streamId);
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
```

- [ ] **Step 6: Add the `escapeHtmlInline` helper**

The shell already imports `escapeHtml` indirectly through other modules in some pages, but `entries.js` currently doesn't import any util. Add a small inline helper near the top of `entries.js` (after the constants):

```js
function escapeHtmlInline(s) {
    return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}
```

- [ ] **Step 7: Update `_wireKeyboardHandlers` to pass the page reference to `handle`**

Currently the kb `handle` callback only receives `list`. The new feed/category handlers need access to `this._categoryId` etc. Update:

```js
    _wireKeyboardHandlers(mode) {
        const cfg = MODES[mode];
        if (!cfg.kb || cfg.kb.length === 0) return;
        const page = this;
        customElements.whenDefined('rdrs-entry-list').then(() => {
            const list = page.querySelector('rdrs-entry-list');
            if (!list) return;
            list.registerKeyboardHandlers({
                helpItems: cfg.kb.map(k => ({ key: k.key, desc: k.desc })),
                handleKey(key) {
                    const entry = cfg.kb.find(k => k.key === key);
                    if (!entry) return false;
                    return entry.handle(list, page);
                },
            });
        });
    }
```

- [ ] **Step 8: Build to verify embedded source compiles**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add static/js/pages/entries.js
git commit -S -m "$(cat <<'EOF'
feat(csr): extend rdrs-entries-page with feed + category modes

inferMode() now matches /feeds/{id}/entries and /categories/{id}/entries.
Both modes resolve their stream-id and breadcrumb asynchronously:
feed mode pulls from GET /api/feeds (filtered by id), category mode
reads from the sidebar bootstrap JSON. Filter dropdown (?status=)
mutates api-params + URL via replaceState. Mark-as-read scoped to
the per-stream stream-id.

The page handlers still SSR — the next two commits flip /feeds/{id}/
entries and /categories/{id}/entries to the shared shell.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Migrate `feed_entries_page` handler

**Files:**
- Modify: `src/handlers/pages.rs` (the handler around line 1258 + the `FeedEntriesTemplate` struct above it)
- Modify: `tests/pages_test.rs` (`test_feed_entries_page`, `test_feed_entries_page_other_user`, `test_feed_entries_page_not_found`, delete `test_feed_entries_page_contains_ssr_json`)

- [ ] **Step 1: Update tests first**

In `tests/pages_test.rs`:

```rust
#[tokio::test]
async fn test_feed_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "Feed Title"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/1/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
}
```

`test_feed_entries_page_other_user` and `test_feed_entries_page_not_found` already test 404 behavior — keep their structure but allow the response status check to remain (404 / SEE_OTHER as before). Verify by reading the existing bodies before editing.

Delete `test_feed_entries_page_contains_ssr_json` entirely (around line 884).

- [ ] **Step 2: Run tests — expect fail**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test test_feed_entries_page 2>&1 | tail -10
```

Expected: failures (handler still returns SSR template).

- [ ] **Step 3: Replace the handler + delete the template struct**

In `src/handlers/pages.rs`, find the `FeedEntriesTemplate` struct (around line 1224) + its `IntoResponse` impl + the `feed_entries_page` handler. Replace all of them with:

```rust
/// Serves the CSR shell for `/feeds/{id}/entries`. Mode `feed` in
/// `<rdrs-entries-page>` resolves the stream-id, breadcrumb, and icon
/// asynchronously from `GET /api/feeds`. The handler still verifies
/// that `id` belongs to the authenticated user (404 otherwise).
pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, AppShellTemplate), AppError> {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            Ok::<_, AppError>(user_settings::get_theme(c, user_id).unwrap_or(None))
        })
        .await??;

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    Ok((
        flash,
        AppShellTemplate {
            title: "Feed Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    ))
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
source /tmp/rdrs-env.sh && cargo build && cargo nextest run --test pages_test test_feed_entries_page 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
git add src/handlers/pages.rs tests/pages_test.rs
git commit -S -m "$(cat <<'EOF'
refactor(csr): migrate /feeds/{id}/entries to CSR shell

FeedEntriesTemplate + IntoResponse impl deleted. Handler retains the
Path(id) ownership check (404 on foreign feed). Stream-id, breadcrumb,
icon, and mark-as-read scope are resolved client-side by
<rdrs-entries-page> (mode `feed`).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Migrate `category_entries_page` handler

Same shape as Task 2, for `/categories/{id}/entries`.

**Files:**
- Modify: `src/handlers/pages.rs` (around line 928–1054)
- Modify: `tests/pages_test.rs` (`test_category_entries_page`, delete `test_category_entries_page_contains_ssr_json`)

- [ ] **Step 1: Update tests first**

```rust
#[tokio::test]
async fn test_category_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test Category"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/categories/1/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    // Sidebar bootstrap carries the category list, so the category
    // name must appear in the inlined JSON for client-side breadcrumb.
    assert!(body.contains("Test Category"));
    assert!(!body.contains(r#"class="ssr-entries""#));
}
```

Delete `test_category_entries_page_contains_ssr_json`.

- [ ] **Step 2: Run tests — expect fail**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test test_category_entries_page 2>&1 | tail -10
```

- [ ] **Step 3: Replace the handler + delete the template struct**

Find `CategoryEntriesTemplate` and `category_entries_page` in `src/handlers/pages.rs`. Replace with:

```rust
/// Serves the CSR shell for `/categories/{id}/entries`. Mode `category`
/// in `<rdrs-entries-page>` reads the category name from the sidebar
/// bootstrap blob. The handler verifies ownership (404 otherwise).
pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, AppShellTemplate), AppError> {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| {
            category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
            Ok::<_, AppError>(user_settings::get_theme(c, user_id).unwrap_or(None))
        })
        .await??;

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    Ok((
        flash,
        AppShellTemplate {
            title: "Category Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    ))
}
```

- [ ] **Step 4: Run tests — expect pass**

```bash
source /tmp/rdrs-env.sh && cargo build && cargo nextest run --test pages_test test_category_entries_page 2>&1 | tail -10
```

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
git add src/handlers/pages.rs tests/pages_test.rs
git commit -S -m "$(cat <<'EOF'
refactor(csr): migrate /categories/{id}/entries to CSR shell

CategoryEntriesTemplate + IntoResponse impl deleted. Handler retains
the ownership check via category::find_by_id_and_user. The category
name is read from the inlined sidebar bootstrap by <rdrs-entries-page>
(mode `category`).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Delete obsolete templates

**Files:**
- Delete: `templates/feed_entries.html`, `templates/category_entries.html`

- [ ] **Step 1: Verify no remaining Askama references**

```bash
grep -rn "feed_entries.html\|category_entries.html" src/ templates/ 2>&1
```

Expected: no results (both `#[template(path = ...)]` attributes were removed in Tasks 2 + 3).

- [ ] **Step 2: Delete + build**

```bash
git rm templates/feed_entries.html templates/category_entries.html
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -5
```

Expected: clean build.

- [ ] **Step 3: Run full test suite**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git commit -S -m "$(cat <<'EOF'
refactor(csr): remove obsolete feed + category list templates

feed_entries.html and category_entries.html were the SSR templates
for /feeds/{id}/entries and /categories/{id}/entries. Their handlers
now return the shared app_shell.html with rdrs-entries-page.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Final verification + push + open PR + STOP

- [ ] **Step 1: Full Rust suite + clippy (CI command)**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -5
source /tmp/rdrs-env.sh && cargo fmt --check
source /tmp/rdrs-env.sh && cargo clippy -- -D warnings 2>&1 | tail -5
```

All clean.

- [ ] **Step 2: Full e2e suite**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test 2>&1 | tail -10
```

Expected: all pass except the documented `entry-actions :: keyboard s toggles star` flake.

If `ssr-no-double-render` block 1 fails on a CSR-now route with `count > 1`, **stop**. The transitional `<= 1` should still hold: after B2 only `/search` is SSR (count=0), all four CSR routes count=1. If a route counts 2, that's a real bug — debug before pushing.

- [ ] **Step 3: Push + open PR**

```bash
cd /home/nixos/Develop/claude/rdrs
git push -u origin refactor/csr-feed-category-entries
```

```bash
gh pr create --title "refactor(csr): migrate /feeds/{id}/entries + /categories/{id}/entries to CSR shell (B2)" --body "$(cat <<'EOF'
## Summary

Step 6 of the SSR-to-CSR migration, sub-PR **B2 of 3**. Migrates `/feeds/{id}/entries` and `/categories/{id}/entries` to the shared `<rdrs-entries-page>` shell by adding `feed` and `category` modes. After this PR only `/search` and the SSR cleanup remain (B3).

**Spec:** `docs/superpowers/specs/2026-05-07-csr-entries-design.md`
**Plan:** `docs/superpowers/plans/2026-05-07-csr-entries-step-b2.md`
**Predecessor:** #175 (B1)

## What changed

### Edited
- `static/js/pages/entries.js` — `inferMode()` extended to match `/feeds/{id}/entries` and `/categories/{id}/entries`. Two new entries in the `MODES` lookup. The new `_connectAsync` flow renders the shell + sidebar + flash immediately, then resolves stream-id and breadcrumb asynchronously: feed mode pulls from `GET /api/feeds` (filtered by id), category mode reads from the sidebar bootstrap blob already inlined by the shell. `?status=` filter dropdown and per-stream mark-as-read are wired client-side.
- `src/handlers/pages.rs` — `feed_entries_page` and `category_entries_page` collapsed to `Result<(Flash, AppShellTemplate), AppError>`. `Path(id)` ownership verification preserved (404 on foreign id). `FeedEntriesTemplate` and `CategoryEntriesTemplate` and their `IntoResponse` impls deleted.
- `tests/pages_test.rs` — page tests assert shell shape; SSR-JSON content tests for the two migrated routes deleted.

### Deleted
- `templates/feed_entries.html`, `templates/category_entries.html` — replaced by the shared shell.

### Untouched in B2 (deferred to B3)
- `<rdrs-entry-list>` SSR-hydration paths — still dormant.
- `templates/macros.html`, `templates/search.html`. `/search` handler still SSR.
- `EntryListConfig`, `fetch_entries_for_ssr*`, `entries_to_ssr`, `fetch_reading_pane_entry`, `EntryQuery`, `SsrEntry*`, `SsrReadingPaneEntry` — retained for `/search` until B3 cleanup.
- `ssr-no-double-render.spec.ts` block 1 — still asserts `count <= 1`. After B2 only `/search` fires count=0; the four CSR-migrated routes fire count=1.

## Test plan

- [ ] `source /tmp/rdrs-env.sh && cargo nextest run` — all pass
- [ ] `source /tmp/rdrs-env.sh && cd e2e && npx playwright test` — green except the pre-existing `entry-actions :: keyboard s toggles star` flake
- [ ] Manually visit `/feeds/{id}/entries` — breadcrumb shows `Feeds / <category> / <feed>`, icon renders if present, status filter dropdown changes URL + reloads list, mark-as-read scoped to feed, `c` / `x` jumps to category page
- [ ] Manually visit `/categories/{id}/entries` — breadcrumb shows `Categories / <name>`, status filter works, mark-as-read scoped to category, `x` jumps to `/`
- [ ] Manually visit `/feeds/9999/entries` and `/categories/9999/entries` for foreign ids — 404
- [ ] Manually visit `/search` and `/entries` — unchanged behavior (search still SSR, entries still CSR from B1)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: STOP — manual review**

Do **not** run `gh pr merge`. Surface the PR URL and stop.

---

## Self-Review

**Spec coverage:**
- B2 scope from spec → covered by Tasks 1-4 + verified in Task 5. ✓
- `<rdrs-entries-page>` extended with feed + category modes → Task 1. ✓
- 2 handler migrations → Tasks 2 + 3. ✓
- Template deletion → Task 4. ✓
- Final verification + PR → Task 5. ✓
- Endpoint deferral (no new endpoint in B2) → respected; `GET /api/feeds` reused, sidebar bootstrap reused for category. ✓

**Placeholder scan:** None. Every code block contains literal code; every command has expected output described.

**Type / signature consistency:**
- `kb.handle(list, page)` is a 2-arg call — Task 1 step 7 updates the call site, Task 1 step 3 has new modes using both args, B1 modes still use `(list)` — backwards compatible (page arg ignored when not needed). ✓
- `escapeHtmlInline` defined once, used twice. ✓
- `pathId()` and `inferMode()` consistent regex patterns. ✓

**Risks pre-flagged:**
- `_connectAsync` reads sidebar bootstrap which is per-user. If sidebar fetch is bypassed (rare race), category meta is missing → fall back to "Category not found" header. The list-pane body is empty in that case; user sees the issue immediately.
- `GET /api/feeds` returns ALL feeds even though we only need one. With many feeds this is wasteful. Acceptable for now; B3 or a follow-up can add `GET /api/feeds/{id}` if needed.
- Filter dropdown's `_setStatus` calls `loadEntries()` which fires a fresh fetch. The previous SSR template had similar behavior (the kb shortcut `1`/`2`/`3`/`4` updated the dropdown, called `handleFilterChange`). No new fetch pattern.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-07-csr-entries-step-b2.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute in this session using executing-plans.

Per user instruction (manual review at PR open), execute inline and stop at PR creation.
