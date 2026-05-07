# CSR Entries — Step B3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `/search` to the shared `<rdrs-entries-page>` shell, remove every remaining SSR scaffold from the entries family, add `GET /api/entries/{id}`, switch the `?entry=N` deep-link path to use it, and tighten `ssr-no-double-render.spec.ts` block 1 back to `count == 1`. After this PR every list page is CSR; only `/login` / `/register` remain SSR (handled in step 7) and the SPA router is the last step.

**Architecture:** `<rdrs-entries-page>` gains a `search` mode that renders the search input header and starts with `no-auto-load`; `?q=` triggers the fetch. The handler collapses to the standard shell shape. The `<rdrs-entry-list>` SSR-hydration code paths (`_extractSsrData`, `_hydrateSsr`, `_consumeSsrData`, `_hydrateReadingPaneSsr`) are deleted now that no template embeds the JSON. The new `GET /api/entries/{id}` endpoint replaces the GReader `stream/items/contents` fallback inside `_loadEntryByIdInPane` with a clean RDRS-native shape (the GReader endpoint stays — it's exposed for FreshRSS clients — but RDRS pages no longer call it for deep links).

**Tech Stack:** Rust (axum + askama 0.13) + vanilla JS (native custom elements) + Playwright e2e.

**Spec:** [`docs/superpowers/specs/2026-05-07-csr-entries-design.md`](../specs/2026-05-07-csr-entries-design.md)

**Predecessor PRs:** #175 (B1, merged 2026-05-07), #176 (B2, merged 2026-05-07).

**Branch:** `refactor/csr-search-and-cleanup` (already cut from current `main`).

**Environment:** Source `/tmp/rdrs-env.sh` before every `cargo` / `cargo nextest` / `npm` / Playwright invocation.

---

## File Structure

| File | Status | Responsibility |
|------|--------|---------------|
| `src/handlers/entry.rs` | EDIT | Add `get_entry_detail` returning `EntryDetailResponse` JSON |
| `src/lib.rs` | EDIT | Register `GET /api/entries/{id}` route |
| `static/js/components/rdrs-entry-list.js` | EDIT | Drop SSR-hydration paths; switch `_loadEntryByIdInPane` to new endpoint |
| `static/js/pages/entries.js` | EDIT | Add `search` mode (input header, Enter-to-search, `?q=` URL state, `no-auto-load`) |
| `src/handlers/pages.rs` | EDIT | Migrate `search_page` handler; delete every SSR scaffold (`EntryListConfig`, `EntryQuery`, `SsrEntry`, `SsrEntryView`, `SsrReadingPaneEntry`, `SidebarCategory`, `fetch_entries_for_ssr*`, `entries_to_ssr`, `ssr_entries_to_views`, `fetch_reading_pane_entry`, `fetch_sidebar_data`); delete `SearchTemplate`, `SearchPageQuery` |
| `templates/search.html` | DELETE | Replaced by shell |
| `templates/macros.html` | EDIT | Remove `entry_list_content` and `reading_pane` macros (other macros stay) |
| `tests/pages_test.rs` | EDIT | Update / delete search tests; assert shell shape on `/search` |
| `tests/entry_handlers_test.rs` | EDIT | Add tests for `GET /api/entries/{id}` (owner / non-owner / 404) |
| `e2e/tests/ssr-no-double-render.spec.ts` | EDIT | Block 1: tighten `count <= 1` to `count == 1`, drop transitional comment, drop `about:blank` flush comment about post-login race (the flush itself stays — still required) |

---

## Task 1: Add `GET /api/entries/{id}` JSON endpoint

Add the endpoint first so the entry-list refactor in Task 2 has a target. Owner / non-owner / 404 paths covered by integration tests.

**Files:**
- Modify: `src/handlers/entry.rs`
- Modify: `src/lib.rs` (register the route)
- Modify: `tests/entry_handlers_test.rs`

- [ ] **Step 1: Write the failing test**

In `tests/entry_handlers_test.rs`, append:

```rust
#[tokio::test]
async fn test_get_entry_detail_owner() {
    let (server, _state, user_id) = setup_test_server().await;
    let _other_id = create_user(&_state.db, "other", "password").await;

    // Create category, feed, entry for the test user.
    _state.db.user(move |conn| {
        conn.execute("INSERT INTO category (user_id, name) VALUES (?1, ?2)", rusqlite::params![user_id, "Cat"]).unwrap();
        let cat_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)", rusqlite::params![cat_id, "https://example.com/f.xml", "Feed"]).unwrap();
        let feed_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO entry (feed_id, guid, title, link, content) VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![feed_id, "g-1", "Entry Title", "https://example.com/e1", "<p>Body</p>"]).unwrap();
    }).await.unwrap();

    let response = server.get("/api/entries/1").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], 1);
    assert_eq!(body["title"], "Entry Title");
    assert_eq!(body["link"], "https://example.com/e1");
    assert_eq!(body["feed_title"], "Feed");
    assert_eq!(body["category_name"], "Cat");
    assert!(body["content"].as_str().unwrap().contains("Body"));
}

#[tokio::test]
async fn test_get_entry_detail_not_owner() {
    let (server, _state, user_id) = setup_test_server().await;
    let other_id = create_user(&_state.db, "other", "password").await;

    // Other user owns the entry.
    _state.db.user(move |conn| {
        conn.execute("INSERT INTO category (user_id, name) VALUES (?1, ?2)", rusqlite::params![other_id, "Other Cat"]).unwrap();
        let cat_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO feed (category_id, url) VALUES (?1, ?2)", rusqlite::params![cat_id, "https://example.com/o.xml"]).unwrap();
        let feed_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)", rusqlite::params![feed_id, "g-other", "Other Entry"]).unwrap();
    }).await.unwrap();
    let _ = user_id;

    let response = server.get("/api/entries/1").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_entry_detail_missing() {
    let (server, _state, _user_id) = setup_test_server().await;
    let response = server.get("/api/entries/9999").await;
    response.assert_status_not_found();
}
```

If the existing test file uses different fixture functions (e.g. `setup_test_server` may be named differently), grep for the file's existing test setup pattern and copy it:

```bash
grep -n "fn setup_test_server\|fn create_user\|async fn login" tests/entry_handlers_test.rs | head -10
```

Use the same helpers as the surrounding tests in that file.

- [ ] **Step 2: Run tests — expect fail with route 404 / handler missing**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test entry_handlers_test test_get_entry_detail 2>&1 | tail -10
```

Expected: 3 fails (route not registered).

- [ ] **Step 3: Add `EntryDetailResponse` + `get_entry_detail` handler**

Append to `src/handlers/entry.rs`:

```rust
#[derive(Debug, Serialize)]
pub struct EntryDetailResponse {
    pub id: i64,
    pub title: String,
    pub link: Option<String>,
    pub author: String,
    pub content: String,
    pub feed_id: i64,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub category_id: i64,
    pub category_name: String,
    pub published_at: Option<String>,
    pub read_at: Option<String>,
    pub starred_at: Option<String>,
    pub summary_status: Option<String>,
}

/// Returns the data needed to render the reading pane for a single
/// entry. Used by `<rdrs-entry-list>._loadEntryByIdInPane` for `?entry=N`
/// deep links — replaces the previous GReader `stream/items/contents`
/// fallback which returned a wrapped GReader-shape object.
pub async fn get_entry_detail(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<EntryDetailResponse>> {
    let user_id = auth_user.user.id;
    let secret = state.config.image_proxy_secret.clone();
    let proxy_base_url = state.config.public_base_url.clone();

    let response = state
        .db
        .read_user(move |conn| {
            let ewf =
                entry::find_by_id_with_feed(conn, id)?.ok_or(AppError::EntryNotFound)?;
            let cat = category::find_by_id(conn, ewf.category_id)?
                .ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::EntryNotFound);
            }

            let e = &ewf.entry;
            let link = e.link.as_deref().unwrap_or("");
            let base_url = if link.is_empty() { None } else { Some(link) };
            let referrer = ewf.custom_referrer.as_deref();

            let content = if let Some(c) = e.content.as_deref() {
                crate::services::sanitize_html(c, &secret, base_url, referrer, proxy_base_url.as_deref())
            } else {
                let fallback = e.summary.as_deref().unwrap_or("");
                crate::services::sanitize_html(fallback, &secret, base_url, referrer, proxy_base_url.as_deref())
            };

            let summary_status =
                entry_summary::get_statuses_for_entries(conn, user_id, &[id])
                    .ok()
                    .and_then(|m| m.get(&id).map(|s| s.as_str().to_string()));

            Ok::<_, AppError>(EntryDetailResponse {
                id: e.id,
                title: e.title.clone().unwrap_or_else(|| "Untitled".to_string()),
                link: e.link.clone(),
                author: e.author.clone().unwrap_or_default(),
                content,
                feed_id: e.feed_id,
                feed_title: ewf.feed_title.clone().unwrap_or_else(|| ewf.feed_url.clone()),
                feed_has_icon: ewf.feed_has_icon,
                category_id: ewf.category_id,
                category_name: ewf.category_name.clone(),
                published_at: e.published_at.map(|dt| dt.to_rfc3339()),
                read_at: e.read_at.map(|dt| dt.to_rfc3339()),
                starred_at: e.starred_at.map(|dt| dt.to_rfc3339()),
                summary_status,
            })
        })
        .await??;

    Ok(Json(response))
}
```

- [ ] **Step 4: Register the route**

In `src/lib.rs`, find the entry endpoints block (around line 142, the `/api/entries/{id}/...` cluster) and add:

```rust
        .route(
            "/api/entries/{id}",
            get(handlers::entry::get_entry_detail),
        )
```

Place it before the more-specific `/api/entries/{id}/fetch-full-content` route (Axum matches by full path — order is for readability).

- [ ] **Step 5: Run tests — expect pass**

```bash
source /tmp/rdrs-env.sh && cargo build && cargo nextest run --test entry_handlers_test test_get_entry_detail 2>&1 | tail -10
```

Expected: 3 pass.

- [ ] **Step 6: Format + commit**

```bash
cargo fmt
pwd  # /home/nixos/Develop/claude/rdrs
git add src/handlers/entry.rs src/lib.rs tests/entry_handlers_test.rs
git commit -S -m "$(cat <<'EOF'
feat(api): add GET /api/entries/{id} for reading-pane deep links

Returns the entry detail (title, sanitized content, link, feed +
category info, timestamps, summary status) as a clean RDRS-native
JSON shape. The next commit switches <rdrs-entry-list> deep-link
fetch to use this endpoint instead of the GReader-compat
stream/items/contents POST.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Switch `_loadEntryByIdInPane` to use `GET /api/entries/{id}`

**Files:**
- Modify: `static/js/components/rdrs-entry-list.js`

- [ ] **Step 1: Replace the fetch path**

Find `_loadEntryByIdInPane` (around line 340) and replace its body:

```js
    async _loadEntryByIdInPane(entryId) {
        const pane = this._getReadingPane();
        if (!pane) return;

        pane.classList.add('reading-pane-active');
        pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Loading...</p></div>`;

        try {
            const response = await fetch(`/api/entries/${entryId}`);
            if (response.status === 404) {
                pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Entry not found.</p></div>`;
                return;
            }
            if (!response.ok) throw new Error('Failed to load entry');
            const data = await response.json();
            this._readingPaneEntry = { id: entryId, ...data };
            this._readingPaneData = data;
            this._renderReadingPaneDetail(pane, data, entryId);
            pane.scrollTop = 0;

            if (data.read_at === null) {
                this.markRead(entryId, true);
            }

            if (data.summary_status) {
                this._handleSummaryStatus(data.summary_status, entryId);
            }
        } catch (err) {
            pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Failed to load entry.</p></div>`;
        }
    }
```

The new endpoint already returns the shape `_renderReadingPaneDetail` expects (same fields as `_extractEntryData`'s output) — no transform layer needed.

- [ ] **Step 2: Build + verify e2e for deep link still works**

```bash
source /tmp/rdrs-env.sh && cargo build
source /tmp/rdrs-env.sh && cd e2e && npx playwright test tests/entry-detail.spec.ts 2>&1 | tail -10
```

Expected: green (the deep-link e2e flow still works, now using the new endpoint).

- [ ] **Step 3: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add static/js/components/rdrs-entry-list.js
git commit -S -m "$(cat <<'EOF'
refactor(csr): switch reading-pane deep link to GET /api/entries/{id}

<rdrs-entry-list>._loadEntryByIdInPane previously POSTed to the
GReader-compat stream/items/contents endpoint and reshaped the
response. Now it GETs the new RDRS-native endpoint which returns
the reading-pane shape directly — fewer LOC, cleaner contract,
no GReader-protocol coupling for our own UI.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `search` mode to `<rdrs-entries-page>`

**Files:**
- Modify: `static/js/pages/entries.js`

- [ ] **Step 1: Extend `inferMode()` and add a `search` entry to `MODES`**

In `static/js/pages/entries.js`, update `inferMode()`:

```js
function inferMode() {
    const path = location.pathname;
    if (path === '/' || path === '') return 'unread';
    if (path === '/entries') return 'all';
    if (path === '/entries/read') return 'read';
    if (path === '/entries/starred') return 'starred';
    if (path === '/entries/summarized') return 'summarized';
    if (path === '/search') return 'search';
    if (/^\/feeds\/\d+\/entries$/.test(path)) return 'feed';
    if (/^\/categories\/\d+\/entries$/.test(path)) return 'category';
    return 'unread';
}
```

Append to `MODES` (just before the closing `};`):

```js
    search: {
        title: 'Search',
        navKey: 'search',
        renderHeader: () => `
<h1>Search</h1>
<div class="filter-bar">
    <div class="form-group form-group-inline flex-1">
        <input type="text" id="filter-search" placeholder="Search entries..." autofocus data-testid="search-input">
    </div>
    <div>
        <button type="button" id="search-btn" data-testid="search-btn">Search</button>
    </div>
</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            origin: 'search',
            'show-feed': '',
            'show-category': '',
            'no-auto-load': '',
            'empty-message': 'Enter a search term and press Enter to search.',
        },
        kb: [
            { key: '/', desc: 'Focus search box', handle: (list, page) => { const input = page.querySelector('#filter-search'); if (input) input.focus(); return true; } },
        ],
    },
```

- [ ] **Step 2: Branch `connectedCallback` for `search` mode**

`search` is a static-mode-shaped flow (the input + button live in the header, not behind an async meta fetch), so the existing static branch works. But after `_loadAndStart` it must also wire the search input event handlers. Update `_loadAndStart`:

```js
    async _loadAndStart() {
        const list = this.querySelector('rdrs-entry-list');
        if (!list) return;
        try {
            const res = await fetch('/api/user-settings');
            if (res.ok) {
                const settings = await res.json();
                if (settings.entries_per_page) list.setAttribute('entries-per-page', String(settings.entries_per_page));
                if (settings.linkding_configured) list.setAttribute('has-save-services', '');
                if (settings.kagi_configured) list.setAttribute('has-kagi-configured', '');
            }
        } catch { /* defaults */ }

        // Search mode is no-auto-load; wiring happens here so the input is
        // ready before any URL-driven first fetch.
        if (this.dataset.mode === 'search') {
            this._wireSearch(list);
            return;
        }

        list.loadEntries();
    }

    _wireSearch(list) {
        const input = this.querySelector('#filter-search');
        const btn = this.querySelector('#search-btn');
        if (!input || !btn) return;

        const doSearch = () => {
            const q = input.value.trim();
            if (!q) {
                list.search = '';
                list.showEmpty('Enter a search term and press Enter to search.');
                history.replaceState(null, '', '/search');
                return;
            }
            list.search = q;
            list.loadEntries();
            const params = new URLSearchParams();
            params.set('q', q);
            history.replaceState(null, '', '/search?' + params.toString());
        };

        btn.addEventListener('click', doSearch);
        input.addEventListener('keydown', (event) => {
            if (event.key === 'Enter') {
                event.preventDefault();
                doSearch();
            } else if (event.key === 'Escape') {
                event.preventDefault();
                input.blur();
            }
        });

        const initialQ = new URLSearchParams(location.search).get('q');
        if (initialQ) {
            input.value = initialQ;
            list.search = initialQ;
            list.loadEntries();
        } else {
            list.showEmpty('Enter a search term and press Enter to search.');
        }
    }
```

`<rdrs-entry-list>` already exposes `showEmpty(msg)` and the `search` setter — no additions there.

- [ ] **Step 3: Build to verify embedded source compiles**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
```

- [ ] **Step 4: Commit**

```bash
git add static/js/pages/entries.js
git commit -S -m "$(cat <<'EOF'
feat(csr): add search mode to rdrs-entries-page

Extends inferMode + MODES with a `search` mode driven by the existing
search input + button hooks ([data-testid=search-input], search-btn,
filter-search). Mounts <rdrs-entry-list> with no-auto-load and only
fires loadEntries when ?q= is present in the URL or the user submits
the form. URL state is mirrored via history.replaceState. Keyboard
shortcut: `/` focuses the input.

The /search handler still SSRs in this commit — the next commit
flips it to the shared shell.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Migrate `search_page` handler

**Files:**
- Modify: `src/handlers/pages.rs` (handler around line 1002 + `SearchTemplate` + `SearchPageQuery`)
- Modify: `tests/pages_test.rs` (`test_search_page`, delete `test_search_page_contains_ssr_entries_json_when_query_present`)

- [ ] **Step 1: Update test first**

In `tests/pages_test.rs`, find `test_search_page` (around line 658):

```rust
#[tokio::test]
async fn test_search_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/search").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
}
```

Delete `test_search_page_contains_ssr_entries_json_when_query_present` and `test_search_page_without_query_emits_empty_ssr_payload` (search for `test_search_page_*` in the file and delete each one that asserts SSR JSON). Find them with:

```bash
grep -n "test_search_page" tests/pages_test.rs
```

- [ ] **Step 2: Run tests — expect fail**

```bash
source /tmp/rdrs-env.sh && cargo nextest run --test pages_test test_search_page 2>&1 | tail -10
```

- [ ] **Step 3: Replace the handler**

In `src/handlers/pages.rs`, find `SearchTemplate` (line 966), `SearchPageQuery` (line 997), and `search_page` (line 1002). Replace the trio with:

```rust
/// Serves the CSR shell for `/search`. The query input + URL state
/// are managed client-side by `<rdrs-entries-page>` (mode `search`).
pub async fn search_page(
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
            title: "Search - RDRS",
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

- [ ] **Step 4: Run tests — expect pass**

```bash
source /tmp/rdrs-env.sh && cargo build && cargo nextest run --test pages_test test_search_page 2>&1 | tail -10
```

- [ ] **Step 5: Format + commit**

```bash
cargo fmt
git add src/handlers/pages.rs tests/pages_test.rs
git commit -S -m "$(cat <<'EOF'
refactor(csr): migrate /search to CSR shell

SearchTemplate + SearchPageQuery + IntoResponse impl deleted. The
search query is managed client-side by <rdrs-entries-page> (mode
`search`) — input, Enter handler, URL state, and ?q=-driven first
load all live in entries.js.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Delete `templates/search.html`

**Files:**
- Delete: `templates/search.html`

- [ ] **Step 1: Verify no Askama references remain**

```bash
grep -rn "search.html" src/ templates/ 2>&1
```

Expected: empty.

- [ ] **Step 2: Delete + build**

```bash
git rm templates/search.html
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
```

- [ ] **Step 3: Commit**

```bash
git commit -S -m "$(cat <<'EOF'
refactor(csr): remove obsolete templates/search.html

Replaced by the shared app_shell.html with rdrs-entries-page (mode
`search`).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Cleanup — drop SSR scaffolding from `pages.rs`, macros, `<rdrs-entry-list>`

This is the big sweep. Nothing references the SSR helpers anymore (verified by `cargo build` after Task 5). Group all cleanups into one commit because they're a coherent removal of one architectural layer.

**Files:**
- Modify: `src/handlers/pages.rs` (delete all SSR scaffold definitions)
- Modify: `templates/macros.html` (remove `entry_list_content` and `reading_pane`)
- Modify: `static/js/components/rdrs-entry-list.js` (drop SSR-hydration paths)

- [ ] **Step 1: Delete SSR scaffolding from `pages.rs`**

Delete each of these from `src/handlers/pages.rs`:

- `pub struct SidebarCategory` (around line 18)
- `fn fetch_sidebar_data` (around line 25)
- `struct SsrEntry` (around line 48)
- `fn escape_json_for_script` is **kept** — `sidebar_bootstrap_json` and `flash_bootstrap_json` still call it
- `pub struct SsrEntryView` (around line 141)
- `fn ssr_entries_to_views` (around line 157)
- `pub struct SsrReadingPaneEntry` (around line 203)
- `pub struct EntryQuery` (around line 225)
- `fn fetch_reading_pane_entry` (around line 302)
- `fn entries_to_ssr` (around line 359)
- `struct SsrEntryResult`, `fn fetch_entries_for_ssr`, `fn fetch_entries_for_ssr_with_sort` (one cluster around lines 391-475)
- `struct EntryListConfig` (around line 824)

After deletions, confirm `escape_json_for_script` still has callers (`sidebar_bootstrap_json`, `flash_bootstrap_json`):

```bash
grep -n "escape_json_for_script" src/handlers/pages.rs
```

Expected: 3 hits (1 definition + 2 callers).

- [ ] **Step 2: Remove the two macros from `templates/macros.html`**

Open `templates/macros.html`. Delete:
- The `{% macro entry_list_content(...) %}…{% endmacro %}` block (around line 150)
- The `{% macro reading_pane(...) %}…{% endmacro %}` block (around line 202)

Keep `theme_attr`, `flash`, `sidebar` macros — they're used by `app_shell.html` and `templates/login.html` / `register.html`.

Verify with:

```bash
grep -n "macro " templates/macros.html
```

Expected: only `theme_attr`, `flash`, `sidebar`.

- [ ] **Step 3: Drop SSR-hydration paths from `<rdrs-entry-list>`**

In `static/js/components/rdrs-entry-list.js`:

a) Delete `_extractSsrData()` (around line 52).
b) Delete `_hydrateSsr()` (around line 66).
c) Delete `_consumeSsrData()` (around line 86).
d) Delete `_hydrateReadingPaneSsr()` (around line 290).
e) Delete `_ssrData` and `_hydrated` fields from the constructor.
f) Delete the `hydrated` getter (around line 132-133).
g) Replace `connectedCallback` with the simplified version:

```js
    connectedCallback() {
        this._render();
        this._setupDelegation();
        this._setupPersistedRestore();
        this._setupPopstate();

        if (!this.hasAttribute('no-auto-load')) {
            this.loadEntries();
        }
    }
```

h) In `_checkEntryParam` (around line 262), remove the SSR-hydrate branch:

```js
    // --- Check URL for ?entry= param on load ---
    _checkEntryParam() {
        const urlEntry = new URLSearchParams(window.location.search).get('entry');
        if (!urlEntry) return;

        const entryId = parseInt(urlEntry, 10);
        if (isNaN(entryId)) return;

        const idx = this.entries.findIndex(e => e.id === entryId);
        if (idx >= 0) {
            this.selectEntry(idx);
            this._loadInReadingPane(idx);
        } else {
            this._loadEntryByIdInPane(entryId);
        }
    }
```

- [ ] **Step 4: Build + run full test suite**

```bash
source /tmp/rdrs-env.sh && cargo build 2>&1 | tail -3
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -5
```

Expected: clean build, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/pages.rs templates/macros.html static/js/components/rdrs-entry-list.js
git commit -S -m "$(cat <<'EOF'
refactor(csr): remove SSR scaffolding now that every list page is CSR

src/handlers/pages.rs:
- Drop SidebarCategory, fetch_sidebar_data, SsrEntry, SsrEntryView,
  ssr_entries_to_views, SsrReadingPaneEntry, EntryQuery,
  fetch_reading_pane_entry, entries_to_ssr, SsrEntryResult,
  fetch_entries_for_ssr*, EntryListConfig — all unused after the
  /search migration.
- escape_json_for_script stays (used by the sidebar + flash
  bootstrap JSON encoders).

templates/macros.html:
- Remove entry_list_content and reading_pane macros — no template
  calls them anymore. theme_attr, flash, sidebar macros stay.

static/js/components/rdrs-entry-list.js:
- Drop SSR-hydration paths: _extractSsrData, _hydrateSsr,
  _consumeSsrData, _hydrateReadingPaneSsr, _ssrData, _hydrated,
  hydrated getter. connectedCallback simplified to always _render +
  loadEntries (unless no-auto-load). _checkEntryParam no longer
  consults SSR state.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Tighten `ssr-no-double-render.spec.ts` block 1 to `count == 1`

After Task 6 every list route is CSR, so block 1's transitional `<= 1` can tighten back. Block 2 + 3 unchanged.

**Files:**
- Modify: `e2e/tests/ssr-no-double-render.spec.ts`

- [ ] **Step 1: Tighten the assertions**

In `e2e/tests/ssr-no-double-render.spec.ts`, replace every `expect(count).toBeLessThanOrEqual(1);` in block 1 (lines around 67/73/89/95) with `expect(count).toBe(1);`.

Update the describe-block docstring from "First paint fires at most one stream/contents fetch" to "First paint fires exactly one stream/contents fetch". Drop the transitional comment about CSR/SSR mix.

The `gotoCounting` helper's `about:blank` flush MUST stay — it's still required because the post-login `/` page is CSR and fires a fetch that would otherwise be double-counted on the next `goto`.

- [ ] **Step 2: Run e2e**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test tests/ssr-no-double-render.spec.ts 2>&1 | tail -10
```

Expected: 13 pass.

- [ ] **Step 3: Commit**

```bash
cd /home/nixos/Develop/claude/rdrs
git add e2e/tests/ssr-no-double-render.spec.ts
git commit -S -m "$(cat <<'EOF'
test(csr): tighten ssr-no-double-render block 1 back to count == 1

Every list route is CSR now (steps 6a + 6b + 6c). First paint fires
exactly one stream/contents fetch — no SSR pre-render skipping it,
no second fetch from a hydration race. The transitional `<= 1`
assertion from B1 is replaced with the strict `== 1`. gotoCounting's
about:blank flush stays required (post-login `/` page fires its own
fetch which would otherwise spill into the next goto).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Final verification + push + open PR + STOP

- [ ] **Step 1: Full Rust suite + clippy + fmt**

```bash
source /tmp/rdrs-env.sh && cargo nextest run 2>&1 | tail -5
source /tmp/rdrs-env.sh && cargo fmt --check
source /tmp/rdrs-env.sh && cargo clippy -- -D warnings 2>&1 | tail -5
```

All clean.

- [ ] **Step 2: Full e2e**

```bash
source /tmp/rdrs-env.sh && cd e2e && npx playwright test 2>&1 | tail -10
```

Expected: green except the documented `entry-actions :: keyboard s toggles star` flake.

- [ ] **Step 3: Restore screenshots if regenerated**

```bash
cd /home/nixos/Develop/claude/rdrs
git status --short | grep screenshots && git restore screenshots/
```

- [ ] **Step 4: Push + open PR**

```bash
git push -u origin refactor/csr-search-and-cleanup
```

```bash
gh pr create --title "refactor(csr): migrate /search + remove all SSR scaffolding (B3)" --body "$(cat <<'EOF'
## Summary

Step 6 final sub-PR (**B3 of 3**). Migrates `/search` to the shared `<rdrs-entries-page>` shell, adds `GET /api/entries/{id}` for clean reading-pane deep links, removes every remaining SSR scaffold from the entries family, and tightens `ssr-no-double-render.spec.ts` block 1 back to `count == 1`. After this PR every list page is CSR; only `/login` / `/register` (step 7) and the SPA router (step 8) remain.

**Spec:** `docs/superpowers/specs/2026-05-07-csr-entries-design.md`
**Plan:** `docs/superpowers/plans/2026-05-07-csr-entries-step-b3.md`
**Predecessors:** #175 (B1), #176 (B2)

## What changed

### New
- `src/handlers/entry.rs::get_entry_detail` + `EntryDetailResponse` — `GET /api/entries/{id}` returns the reading-pane shape directly. Replaces the GReader `stream/items/contents` fallback inside `<rdrs-entry-list>._loadEntryByIdInPane`.

### Edited
- `src/lib.rs` — register the new `/api/entries/{id}` route.
- `static/js/pages/entries.js` — adds `search` mode (input + Enter handler + URL state), wired through the existing `<rdrs-entry-list>` `no-auto-load` + manual `loadEntries` flow.
- `static/js/components/rdrs-entry-list.js` — drops `_extractSsrData`, `_hydrateSsr`, `_consumeSsrData`, `_hydrateReadingPaneSsr`, `_ssrData`, `_hydrated`, `hydrated` getter. `connectedCallback` simplified. `_loadEntryByIdInPane` uses the new endpoint.
- `src/handlers/pages.rs::search_page` — collapsed to the shared shell. `SearchTemplate`, `SearchPageQuery`, and the rest of the SSR scaffolding (`EntryListConfig`, `EntryQuery`, `SsrEntry`, `SsrEntryView`, `SsrReadingPaneEntry`, `SidebarCategory`, `fetch_entries_for_ssr*`, `entries_to_ssr`, `ssr_entries_to_views`, `fetch_reading_pane_entry`, `fetch_sidebar_data`) deleted. `escape_json_for_script` retained — used by the sidebar / flash bootstrap encoders.
- `templates/macros.html` — `entry_list_content` and `reading_pane` macros removed. `theme_attr`, `flash`, `sidebar` retained.
- `e2e/tests/ssr-no-double-render.spec.ts` — block 1 tightened from `count <= 1` to `count == 1`. `gotoCounting`'s about:blank flush stays.
- `tests/entry_handlers_test.rs` — adds tests for the new endpoint (owner / non-owner / 404).
- `tests/pages_test.rs` — `test_search_page` asserts shell shape; SSR-content tests for `/search` deleted.

### Deleted
- `templates/search.html` — replaced by the shell.

## Test plan

- [ ] `source /tmp/rdrs-env.sh && cargo nextest run` — all pass
- [ ] `source /tmp/rdrs-env.sh && cd e2e && npx playwright test` — all pass except the documented `entry-actions :: keyboard s toggles star` flake
- [ ] Manually visit `/search` (no `?q=`) — empty state with input focused
- [ ] Manually visit `/search?q=<term>` — results render, query in input
- [ ] Manually visit `/?entry=<id>` — reading pane populated via the new endpoint
- [ ] Manually verify `entries_test`, `entry-actions`, `entry-detail`, `entry-navigation`, `keyboard-help` e2e specs still pass

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: STOP — manual review**

Surface PR URL and stop. The user reviews + merges.

---

## Self-Review

**Spec coverage:**
- B3 scope from spec → covered by Tasks 1-7 + verified in Task 8. ✓
- New `GET /api/entries/{id}` → Task 1. ✓
- `<rdrs-entry-list>` SSR-hydration removal → Task 6 step 3. ✓
- `_loadEntryByIdInPane` switched to new endpoint → Task 2. ✓
- Search migration → Tasks 3 + 4. ✓
- Template + macros + scaffolding cleanup → Tasks 5 + 6. ✓
- Block 1 tightened to `== 1` → Task 7. ✓
- Final verification + PR → Task 8. ✓

**Placeholder scan:** None. Every code block is concrete. Every command has expected output described.

**Type / signature consistency:**
- `EntryDetailResponse` field set matches what `<rdrs-entry-list>._renderReadingPaneDetail` consumes (same field names as `_extractEntryData`'s output). ✓
- Mode key `search` consistent across `inferMode`, `MODES`, `_loadAndStart`, `_wireSearch`. ✓
- `_loadAndStart` early-returns for `search` mode (no `loadEntries` until URL has `?q=`). ✓
- Cleanup deletions all reference items confirmed unused via `grep -rn` — `SidebarCategory` differs from `SidebarCategoryDto` in `user.rs`; only the local `pages.rs::SidebarCategory` is removed. ✓

**Risks:**
- Task 6 is the largest commit. If anything regresses (e.g. some forgotten reference), it shows up in `cargo build`. Step 4 verifies clean build before commit.
- e2e `entry-detail.spec.ts` may break if `_renderReadingPaneDetail` consumes a field that's missing from `EntryDetailResponse`. Task 1's response struct mirrors `_extractEntryData`'s output exactly — should match. Verified in Task 2 step 2.
- The pre-existing `entry-actions :: keyboard s toggles star` flake is independent — not in this PR's scope.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-07-csr-entries-step-b3.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute in this session using executing-plans.

Per user instruction (manual review at PR open), execute inline and stop at PR creation.
