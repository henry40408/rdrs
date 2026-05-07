# CSR Migration — Entries Family (Step 6)

**Date:** 2026-05-07
**Status:** Approved (brainstorming, auto mode)
**Predecessor:** [`2026-05-06-ssr-to-csr-migration-design.md`](./2026-05-06-ssr-to-csr-migration-design.md)

## Goal

Migrate the entries-list family of pages from Askama SSR to the shared CSR shell pattern established in PRs #170–#174. After this step the only remaining server-rendered pages are `/login` and `/register` (handled in step 7) and the eventual SPA router (step 8).

In scope:

- `/` (unread)
- `/entries` (all)
- `/entries/read`
- `/entries/starred`
- `/entries/summarized`
- `/feeds/{id}/entries`
- `/categories/{id}/entries`
- `/search`
- `/entries/{id}` (redirect handler — no template, no template change needed)

Out of scope:

- Service Worker / IndexedDB / offline reading.
- SPA navigation (still full-reload between pages).
- Visual or feature changes — preserve every existing data-testid hook.

## Non-goals

- No new JS framework or build tooling — vanilla JS + native custom elements.
- No reshaping of the GReader-compatible API (`/reader/api/0/...`).
- No removal of `<rdrs-entry-list>` itself; it remains the list-rendering primitive.

## Architecture

### Shell

Each page handler returns the shared `templates/app_shell.html` with:

- `element_tag = "rdrs-entries-page"`
- `script_path = "/static/js/pages/entries.js"`
- Existing sidebar + flash bootstrap blobs (unchanged from steps 1–5).

The shell carries no per-page entry data. The page element reads `location.pathname` and `location.search` on connect to determine its `data-mode` and any IDs / search query.

### `<rdrs-entries-page>` element

One custom element handles all eight list-mode routes. `data-mode` is one of:

| mode | route | `<rdrs-entry-list>` config |
|------|-------|---------------------------|
| `unread` | `/` | `stream-id=user/-/state/com.google/reading-list`, `api-params={"xt":"user/-/state/com.google/read"}`, `show-feed`, `show-category`, `show-mark-above`, `origin=unread` |
| `all` | `/entries` | same stream, no `xt`, `show-feed`, `show-category`, `origin=entries` |
| `read` | `/entries/read` | `it=user/-/state/com.google/read`, `show-feed`, `show-category`, `origin=read` |
| `starred` | `/entries/starred` | `it=user/-/state/com.google/starred`, `show-feed`, `show-category`, `origin=starred` |
| `summarized` | `/entries/summarized` | `summarized-only=true` (or equivalent existing param), `show-feed`, `show-category`, `origin=summarized` |
| `feed` | `/feeds/{id}/entries` | `stream-id=feed/{url}`, `show-mark-above`, `origin=feed`; uses `?status=unread\|read\|starred` query for filter dropdown |
| `category` | `/categories/{id}/entries` | `stream-id=user/-/label/{name}`, `origin=category`; same `?status` filter |
| `search` | `/search` | `?q=` from URL, `no-auto-load`, `origin=search` |

The element constructs the page DOM in `connectedCallback` using a small lookup keyed by `data-mode`:

```js
const MODES = {
  unread:     { title: 'Unread',  header: renderUnreadHeader,  list: unreadListAttrs,  kb: unreadKb },
  all:        { title: 'Entries', header: renderTabsHeader,    list: allListAttrs,     kb: tabsKb },
  read:       { title: 'Entries', header: renderTabsHeader,    list: readListAttrs,    kb: tabsKb },
  starred:    { title: 'Entries', header: renderTabsHeader,    list: starredListAttrs, kb: tabsKb },
  summarized: { title: 'Entries', header: renderTabsHeader,    list: summarizedListAttrs, kb: tabsKb },
  feed:       { title: dynamic,   header: renderFeedHeader,    list: feedListAttrs,    kb: feedKb },
  category:   { title: dynamic,   header: renderCategoryHeader, list: categoryListAttrs, kb: categoryKb },
  search:     { title: 'Search',  header: renderSearchHeader,  list: searchListAttrs,  kb: searchKb },
};
```

Per-mode rendering returns a small HTML fragment for the list-pane header and an attributes object for `<rdrs-entry-list>`. The element then inlines:

```html
<div class="split-view">
  <div class="list-pane">
    <div class="list-pane-header">{{ headerHtml }}</div>
    <div class="list-pane-body"><rdrs-entry-list {...attrs}></rdrs-entry-list></div>
  </div>
  <div class="reading-pane" id="reading-pane">
    <div class="reading-pane-empty">Select an entry to read</div>
  </div>
</div>
```

Reading pane content is later filled by `<rdrs-entry-list>` itself (existing behavior — clicking an entry calls `_renderReadingPane()` against the in-memory entry data plus on-demand `/api/entries/{id}/full-content` etc).

`?entry=N` deep link: after `connectedCallback` builds the shell, the element checks `location.search` for `entry=N`. If present, it calls the new `GET /api/entries/{id}` endpoint and populates the reading pane from the response.

### `<rdrs-entry-list>` changes

Minimal. Remove the SSR hydration paths now that no page server-renders entries:

- Drop `_extractSsrData()`, `_hydrateSsr()`, `_consumeSsrData()`, `_ssrData`, `_hydrated`, `hydrated` getter.
- `connectedCallback` always calls `_render()` then `loadEntries()` unless `no-auto-load` is set.
- The `<script type="application/json" class="ssr-entries">` reading paths are gone with the macros.

This change lands in B3 (the cleanup PR), not B1. During B1 and B2 the SSR-hydration code stays dormant: `_extractSsrData()` early-returns when the embedded `<script class="ssr-entries">` is absent (which it is for any page rendered through `<rdrs-entries-page>`), and the rest of the SSR codepath is gated on `_ssrData`. So mixed-state behavior is safe.

`entries.js` MUST import `/static/js/components/rdrs-entry-list.js` so the element is registered before `<rdrs-entries-page>` mounts an instance into the DOM.

### New endpoint: `GET /api/entries/{id}`

Returns the data needed to render the reading pane for a single entry:

```jsonc
{
  "id": 42,
  "title": "...",
  "link": "https://...",
  "author": "...",
  "published_at": "2026-05-07T...",
  "content": "<p>...</p>",
  "feed_id": 7,
  "feed_title": "...",
  "feed_has_icon": true,
  "category_id": 3,
  "category_name": "...",
  "starred_at": null,
  "is_read": false,
  "summary_status": null,
  "summary": null
}
```

Implementation:

- New module `src/handlers/entry.rs` already exists; add a `get_entry_json` handler there.
- Route: `.route("/api/entries/{id}", get(handlers::entry::get_entry_json))` in `src/lib.rs`, registered before the more specific `/api/entries/{id}/...` routes (Axum matches longest-prefix on path segments, so order is for readability rather than correctness).
- Authorization: only the entry's owner may fetch (return 404 otherwise — same convention as existing entry endpoints).
- Reuses the same query helpers that currently feed `ssr_reading_pane`; the SsrReadingPaneEntry struct can be renamed `EntryDetail` and serialised directly.

### Handler shape

All eight list page handlers in `src/handlers/pages.rs` collapse to the standard CSR shell shape:

```rust
pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) { ... }
```

The query parameter parsing (`?entry=N`, `?status=`, `?q=`) moves to the JS — the server doesn't need it. `feed_entries_page` and `category_entries_page` keep the `Path(id)` extractor because they still need to verify the resource exists and belongs to the user (404 if not), but they no longer build SSR view data.

### Templates removed

`templates/entries.html`, `templates/unread.html`, `templates/entries_archive.html`, `templates/feed_entries.html`, `templates/category_entries.html`, `templates/search.html` — all deleted in B3. The `entry_list_content` and `reading_pane` macros in `templates/macros.html` are also removed in B3.

`templates/app_shell.html`, `templates/base.html`, the `sidebar` / `flash` / `theme_attr` macros — unchanged.

### `static_assets.rs` allowlist

Add `entries.js` to the `FILES` allowlist in `src/handlers/static_assets.rs`. (Per project convention from #170-#174 — missing this entry returns 404.)

## Data flow

```
Page request:
  Browser → Axum handler (e.g. unread_page)
                │
                ▼
        AppShellTemplate { element_tag: "rdrs-entries-page",
                          script_path: "/static/js/pages/entries.js",
                          sidebar_bootstrap_json, flash_bootstrap_json, theme }
                │
                ▼
        HTML envelope sent (no entry data)
                │
                ▼
        Browser parses, loads entries.js module
                │
                ▼
  <rdrs-entries-page>.connectedCallback()
        │ ├─ infer mode from location.pathname
        │ ├─ build header + reading-pane skeleton + <rdrs-entry-list>
        │ └─ if ?entry=N: GET /api/entries/{id} → render reading pane
                │
                ▼
  <rdrs-entry-list>.connectedCallback()
        │ └─ loadEntries() → GET /reader/api/0/stream/contents/...
                │
                ▼
        Render entries → user can click → existing reading-pane behavior
```

Total network requests on first paint:

| route | requests |
|-------|----------|
| `/` (no `?entry`) | sidebar bootstrap inlined; `stream/contents` ×1 |
| `/entries?entry=42` | `stream/contents` ×1 + `/api/entries/42` ×1 |
| `/search` (no `?q`) | 0 (no auto-load) |
| `/search?q=foo` | `stream/contents` ×1 |

`ssr-no-double-render.spec.ts` block 1 reframes to assert exactly 1 `stream/contents` call on first paint for the four routes that auto-load.

## Components

| File | Status | Note |
|------|--------|------|
| `src/handlers/pages.rs::{unread,entries,read_entries,starred_entries,summarized_entries,feed_entries,category_entries,search}_page` | rewrite | Standard `(Flash, AppShellTemplate)` shape |
| `src/handlers/pages.rs::entry_page` | unchanged | Already a redirect |
| `src/handlers/pages.rs::EntryQuery,EntryPageQuery,fetch_entry_list_config,SsrEntryView,SsrReadingPaneEntry,*Template` | delete (B3) | All SSR scaffolding removed |
| `src/handlers/entry.rs::get_entry_json` (new) | add (B1) | `GET /api/entries/{id}` |
| `src/lib.rs` | edit | Register new route; keep page routes pointing at refactored handlers |
| `src/handlers/static_assets.rs::FILES` | edit | Add `entries.js` |
| `static/js/pages/entries.js` (new) | add (B1) | `<rdrs-entries-page>` element |
| `static/js/components/rdrs-entry-list.js` | edit (B3) | Drop SSR hydration paths |
| `templates/{entries,unread,entries_archive,feed_entries,category_entries,search}.html` | delete (B3) | Replaced by shell |
| `templates/macros.html` | edit (B3) | Remove `entry_list_content` + `reading_pane` macros |
| `e2e/tests/ssr-no-double-render.spec.ts` | edit (B3) | Block 1 reframed to count == 1; blocks 2/3 unchanged |
| `e2e/tests/{entry-actions,entry-detail,entry-navigation,search,keyboard-help}.spec.ts` | sanity-check | Should still pass; test hooks preserved |

## PR sequence

### B1 — `refactor/csr-entries-list`

- Add `<rdrs-entries-page>` element supporting modes `unread`, `all`, `read`, `starred`, `summarized`.
- Migrate handlers for `/`, `/entries`, `/entries/read`, `/entries/starred`, `/entries/summarized`.
- Add `GET /api/entries/{id}` endpoint.
- Add `entries.js` to static-assets allowlist.
- Templates `entries.html`, `unread.html`, `entries_archive.html` are kept temporarily to avoid breaking B2's feed/category templates that still reference the same SSR helpers — but no longer rendered.

  Actually: simpler to delete the three list templates here, since their handlers no longer reference them. Keep only `feed_entries.html`, `category_entries.html`, `search.html` until their respective PRs.
- Old `*Template` structs (`UnreadTemplate`, `EntriesTemplate`, `EntriesArchiveTemplate`) deleted along with their templates.
- e2e: existing entries specs continue to pass (test hooks preserved). `ssr-no-double-render.spec.ts` block 1 is in a transitional state during B1: the `/` and `/entries` cases now fire exactly 1 fetch (CSR), while `/feeds/{id}/entries` and `/search?q=` still fire 0 (still SSR). Bridge by changing block 1 to `expect(count).toBeLessThanOrEqual(1)` for the duration of B1 and B2. B3 tightens it back to `expect(count).toBe(1)` once every list page is CSR.

### B2 — `refactor/csr-feed-category-entries`

- Extend `<rdrs-entries-page>` with `feed` and `category` modes.
- Migrate `/feeds/{id}/entries` and `/categories/{id}/entries` handlers.
- Add a small JSON endpoint if needed for feed metadata (`feed_title`, `feed_has_icon`, `category_id`, `category_name`) — likely `GET /api/feeds/{id}/page-meta` or extend the existing `GET /api/feeds`. Decide in implementation; both are cheap.
- Delete `feed_entries.html`, `category_entries.html`, their `*Template` structs, and the per-page handler view types.

### B3 — `refactor/csr-search-and-cleanup`

- Extend `<rdrs-entries-page>` with `search` mode.
- Migrate `/search` handler.
- Delete `search.html`, `SearchTemplate`.
- Delete `fetch_entry_list_config`, `SsrEntryView`, `SsrReadingPaneEntry`, and any remaining SSR view structs in `pages.rs`.
- Delete `entry_list_content` and `reading_pane` macros in `templates/macros.html`.
- Drop SSR hydration paths in `rdrs-entry-list.js` (`_extractSsrData`, `_hydrateSsr`, `_consumeSsrData`, `_ssrData`, `_hydrated`, `hydrated`).
- Reframe `ssr-no-double-render.spec.ts` block 1: rename describe to "First paint fires exactly one stream/contents fetch", change all `count.toBe(0)` to `count.toBe(1)`, drop the issue #148 reference, blocks 2/3 unchanged.
- Verify all e2e specs green.

## Error handling

- `GET /api/entries/{id}` for an entry not owned by the user → 404. Page-side: reading pane shows empty state if the deep link 404s; flash a warning ("Entry not found or you don't have access").
- `?entry=N` for an entry not in the current list mode (e.g. `?entry=42` on `/entries/starred` for an unstarred entry) — currently SSR would render it anyway because the reading-pane SSR was independent of the list filter. New behavior: the deep-link reading pane fetch is independent of the list, so this still works — but the entry won't be highlighted in the list since it's not in the filter. Acceptable: same as today's "click an entry, scroll list, mark read, switch tab" flow.
- Network failure during `loadEntries()` → existing `<rdrs-entry-list>` error path (status message + retry on next interaction). Unchanged.

## Testing

### Unit / integration (Rust)

- `tests/handlers_entry_test.rs` (or wherever entry tests live): add tests for `get_entry_json` covering owner / non-owner / nonexistent id.
- Existing handler tests for the 8 page handlers must be updated to assert the shell shape (`<rdrs-entries-page>` tag + `/static/js/pages/entries.js` script) instead of SSR markup. Pattern: same as #170–#174 did for statistics / categories / feeds / settings / admin.

### e2e (Playwright)

- `entry-actions.spec.ts`, `entry-detail.spec.ts`, `entry-navigation.spec.ts`, `keyboard-help.spec.ts`, `search.spec.ts` — must continue to pass unchanged. Their assertions are all data-testid based and the test hooks are preserved.
- `ssr-no-double-render.spec.ts` — block 1 reframed in B3 (see above).

### Known flake

`entry-actions.spec.ts :: keyboard s toggles star` is flaky on `main` already — out of scope for this work.

## Open questions

None — Q1–Q4 settled in brainstorming session 2026-05-07.

## Risks

- **`<rdrs-entries-page>` becomes 800+ lines** if mode-switching helpers aren't carefully extracted. Mitigation: lookup-table pattern + small per-mode helpers, file budget ~700 lines.
- **`feed` / `category` mode headers need feed-name / category-name** which are not available in the URL — must come from a JSON call (either the new `/api/entries/{id}` adjacent endpoint, or extend an existing one). Decided in B2.
- **e2e `count == 1` assertion is brittle** if some future per-page side-effect (e.g. unread-count refresh) calls `stream/contents` with different params — but the current test path-filters on `STREAM_CONTENTS_PATH`, so any same-path call counts. If we add a periodic refresh later, the test will need a different shape (count by URL + query rather than path prefix).
