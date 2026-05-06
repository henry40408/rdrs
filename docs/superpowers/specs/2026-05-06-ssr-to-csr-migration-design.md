# SSR-to-CSR Progressive Migration

**Date:** 2026-05-06
**Status:** Approved (brainstorming, auto-mode)

## Goal

Migrate every page in RDRS from Askama server-side rendering to vanilla-JS
client-side rendering. The endpoint architecture should be PWA-friendly (Service
Worker / IndexedDB / offline sync are explicitly **out of scope** for this
round, but the shell + JSON-API split is laid down so they can land later
without re-architecture).

## Non-goals

- Offline reading, IndexedDB, Service Worker, or sync conflict resolution.
- Introducing a JS framework or build tooling — the project remains
  vanilla-JS + native custom elements with zero build step.
- Rewriting the JSON API. Existing endpoints (`/api/...`) stay as-is; new
  endpoints are added only when SSR data has no JSON twin.
- Changing the visual design or feature set.

## Architecture

### Shared shell template

Each migrated page route returns the same minimal HTML envelope from a new
`templates/app_shell.html`:

- `<head>` carries the existing meta tags, font links, app stylesheet, theme
  bootstrap script, and shared module imports (`rdrs-flash`, `rdrs-kb-help`,
  `rdrs-kb-pending`, `keyboard.js`).
- `<body>` contains exactly one custom element tag for the page (e.g.
  `<rdrs-statistics-page></rdrs-statistics-page>`) plus a `<script type="module">`
  importing that page module from `/static/js/pages/<name>.js`.
- The handler passes `element_tag`, `script_path`, `title`, and the
  sidebar bootstrap JSON to the template.

The shell embeds **only** the sidebar payload as
`<script type="application/json" id="rdrs-sidebar-bootstrap">…</script>`.
This is the smallest piece of per-user data needed to paint the sidebar
without an empty-skeleton-then-content flash on first visit. Page-specific
data (statistics rows, etc.) is still fetched after mount — only the
shared chrome is inlined.

### Auth bootstrap

Shell route handlers continue to depend on the existing `PageAuthUser` /
`PageAdminUser` extractors. Unauthenticated requests still receive a server
redirect to `/login`. The client never sees an unauthenticated shell.

For user info the page module calls **`GET /api/me`** (new), which is a small
extension of `get_current_user` that adds session-derived flags
(`is_admin`, `is_masquerading`) — fields the SSR templates compute via
`PageAuthUser`. The existing `/api/user` is preserved for backwards
compatibility but the CSR pages will prefer `/api/me`.

### Sidebar as a shared component

Most pages share the sidebar (`templates/macros.html::sidebar`). For CSR we
introduce **`<rdrs-sidebar>`** in `static/js/components/rdrs-sidebar.js`. It:

- Is given the active section via attribute (`active="statistics"`)
- Calls `GET /api/sidebar` (new) for `{ username, is_admin, is_masquerading,
  categories: [{id, name, unread_count}], total_unread }`
- Renders the same DOM structure the Askama macro produces, reusing the
  existing CSS classes verbatim

The new `/api/sidebar` endpoint exists so we avoid forcing every page module
to re-aggregate sidebar data; it directly mirrors `fetch_sidebar_data()` in
`pages.rs`.

### Page module convention

```
static/js/pages/
  statistics.js    // exports + registers <rdrs-statistics-page>
  categories.js
  ...
```

Each page module:

1. Declares one custom element class extending `HTMLElement`.
2. In `connectedCallback`, parses URL state (`URLSearchParams`), kicks off
   data fetches in parallel, and renders skeleton + then real content.
3. Re-uses shared helpers from `static/js/utils.js` for loading/error UX
   (a small `renderLoading()` / `renderError()` pair will be added there).
4. Self-registers via `customElements.define(...)` at the bottom.
5. Loads `<rdrs-sidebar>` as a child element.

### Static-asset allowlist

Every new JS file is added to the `FILES` const in
`src/handlers/static_assets.rs`. Until that registration, `/static/js/...`
returns 404 (existing convention).

### Flash messages

Existing flash flow (cookie + `tower_http` middleware) keeps working.
`<rdrs-flash>` already exists — we extend it to optionally read the flash
cookie via `document.cookie` (or via a new `GET /api/flash` consumer
endpoint). For the statistics page, no flash messages are produced, so we
defer the flash adaptation until the first migrated CRUD page (categories).

### Client router (deferred)

A `static/js/router.js` using the History API will be introduced **last**,
after every page is migrated. Until then, navigation is full-reload. This is
explicit in the migration order.

## Migration order

1. Foundation PR (this design + initial scaffolding):
   - `app_shell.html`
   - `serve_app_shell()` handler helper
   - `GET /api/me`, `GET /api/sidebar` endpoints
   - `<rdrs-sidebar>` component
   - First page: **`/statistics`**
2. `/categories` (first CRUD; establishes form + flash patterns)
3. `/feeds`
4. `/settings`, `/user-settings`
5. `/admin`
6. Entry pages (`/`, `/entries`, `/entries/read`, `/entries/starred`,
   `/entries/summarized`, `/entries/{id}`, `/feeds/{id}/entries`,
   `/categories/{id}/entries`, `/search`)
7. `/login`, `/register` (optional, can land any time)
8. `static/js/router.js` (SPA navigation)

## Testing strategy

For each migrated page:

- **Rust integration tests** (`tests/`) update to assert that the shell HTML
  contains the correct element tag and script path, instead of full SSR
  content.
- **e2e tests** (`e2e/*.spec.ts`) keep their feature-level coverage but the
  selectors switch from SSR-rendered DOM to the post-mount CSR DOM. Using
  Playwright's `page.waitForSelector()` with a stable CSR selector handles the
  fetch latency.
- **Model-level tests** for new endpoints (`/api/me`, `/api/sidebar`,
  `/api/statistics`).

## Risks

- **Network waterfall:** Shell → JS → JSON adds two round trips. Mitigation:
  batch initial fetches via `Promise.all`; later, embed a server-side hint
  via `<link rel="preload">` if measured.
- **JS file growth:** `pages/*.js` collectively grow large (especially
  entries page, which already has `rdrs-entry-list.js` at 1706 lines).
  Mitigation: per-page lazy import via the future client router.
- **CSR regressions in keyboard shortcuts / bfcache:** The current
  `setupPersistedRestore` lives in `base.html`. Since `app_shell.html`
  inherits the same head scripts, this keeps working. Validate via e2e.

## First-PR scope

The first PR (this branch) lands:

1. `templates/app_shell.html`
2. `pages.rs::serve_app_shell()` helper + statistics route migration
3. `GET /api/me`
4. `GET /api/sidebar`
5. `GET /api/statistics`
6. `<rdrs-sidebar>` component
7. `<rdrs-statistics-page>` page module
8. `static_assets.rs` allowlist update
9. Updated tests (Rust integration + e2e)

The other 15 templates remain untouched and continue to be served by
existing Askama handlers. Subsequent PRs migrate one page at a time.
