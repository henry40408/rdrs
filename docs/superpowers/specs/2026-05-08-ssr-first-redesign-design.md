# SSR-first redesign

**Status:** design / approval pending
**Date:** 2026-05-08
**Predecessors:** `2026-05-06-ssr-to-csr-migration-design.md`, `2026-05-07-csr-entries-design.md`, `2026-05-07-spa-router-design.md`

## Background

PRs #170-#180, #183, #184 (2026-05-06 → 2026-05-07) migrated all 13
logged-in routes from Askama SSR to a CSR shell + JSON API + SPA
router architecture. After a day of using it, we are reverting to
SSR-first with a small JS supplement.

Two driving principles:

1. **Performance first.** TTFB-first. APIs return only what is
   necessary. Cache aggressively at every layer.
2. **Minimal design.** Avoid large CSS / JS payloads.

## Goals

- All 13 logged-in routes are server-rendered Askama templates. The
  server returns a fully-populated HTML document on first byte; no
  JSON bootstrap blob, no shell + page-element pattern.
- Total client JS for the logged-in surface is one shared file
  (`app.js`, ~400-500 lines) plus one page-scoped exception
  (`passkey.js`, loaded only on `/user-settings`, ~80 lines).
  No build step.
- Standard caching surface: ETag/304 on SSR responses,
  brotli + gzip transport, in-process LRU per-user for hot reads,
  bfcache-friendly headers.
- GReader API (`/reader/api/0/*`) and login/register pages are
  unaffected.

## Non-goals

- Service Worker, IndexedDB, offline sync (the prior migration laid
  groundwork; this work does not pursue or remove that ambition —
  it is simply not in scope).
- Login / register pages (already SSR; untouched).
- GReader API surface (external clients depend on it; untouched).
- Build tooling (no esbuild, no minifier).
- MSRV / `rust-version` field changes.
- New features. This is a pure architectural revert.

## Architecture

### Server

Axum + Askama, one `*.html` template + one handler per route.
Handlers read directly from the DB (or LRU cache), build the
template context, render, and return `Html<String>`. No shell,
no JSON intermediaries for the SSR surface.

Static assets:
- `static/css/app.css` — current 1917-line file, dead-rule pruned
  during the migration; fonts unchanged (4-family bunny.net link
  retained per user decision).
- `static/js/app.js` — shared ~400-500 line file, served raw via
  `include_str!`. No modules, no build step.
- `static/js/passkey.js` — page-scoped ~80 line file loaded only
  by `/user-settings` (WebAuthn requires JS).

### Client

A shared `static/js/app.js` (loaded on every logged-in page)
exposing six small responsibilities, plus a page-scoped
`static/js/passkey.js` loaded only by `/user-settings`:

| Section | Lines (approx) | Purpose |
|---------|----------------|---------|
| `swap()` helper | 80 | Intercept `[data-swap]` form/link, fetch HTML fragment, replace target via `outerHTML`. Multi-target via `<template data-swap-target="#sel">…</template>` blocks in the response. Native fallback on fetch failure. |
| Keyboard | 120 | `j`/`k`/`space`/`s`/`m`/`o`/`?` shortcuts trigger the corresponding link/form via `swap()`. |
| Sidebar polling | 30 | `setInterval(20s)` fetch `/sidebar/unread` → swap. |
| Flash dismiss + sidebar mobile toggle | 30 | Two listeners. |
| Theme controller | 30 | Carry-over from current `window.theme`. `PUT /api/user/settings/theme` retained. |
| `pageshow` bfcache restore | 20 | Carry-over from current `window.onPersistedPageShow`. |
| Passkey controller (loaded only on `/user-settings`) | 80 | WebAuthn `navigator.credentials.create()/get()` + register/delete. The only JS that lives off the main `app.js` — kept inline in `user_settings.html` or a sibling `passkey.js`. |

No custom elements. No SPA router. No `<rdrs-*>` tags. No
page-element host. Old `static/js/components/` and
`static/js/pages/` directories are deleted at the end of the
migration.

### Templates

```
templates/
  base.html              # shared <head>, theme inline, sidebar mobile toggle
  _sidebar.html          # sidebar tree partial
  _flash.html            # flash messages partial
  _entry_row.html        # single entries-list row (re-rendered on star/read)
  _reading_pane.html     # reading pane content (also fragment endpoint output)
  _sidebar_unread.html   # sidebar unread counts (polling + piggyback fragment)
  _entries_layout.html   # shared two-pane shell for the entries family
  login.html             # untouched
  register.html          # untouched
  unread.html            # /
  entries.html           # /entries
  read_entries.html      # /entries/read
  starred_entries.html   # /entries/starred
  summarized_entries.html# /entries/summarized
  feed_entries.html      # /feeds/{id}/entries
  category_entries.html  # /categories/{id}/entries
  search.html            # /search
  feeds.html             # /feeds
  categories.html        # /categories
  settings.html          # /settings
  user_settings.html     # /user-settings
  admin.html             # /admin
  statistics.html        # /statistics
  error.html             # generic AppError surface
```

Fragment endpoints re-render the same partial used in the full
page — single source of truth, no markup duplication.

### Endpoints

**SSR pages** (one per route, listed above).

**Fragment endpoints** (return HTML, content-type `text/html`):

| Endpoint | Purpose | Renders |
|----------|---------|---------|
| `GET /entries/{id}/fragment` | Reading-pane swap target. Replaces `GET /api/entries/{id}` JSON. | `_reading_pane.html` |
| `POST /entries/{id}/star` | Toggle star, returns updated row + sidebar. | `_entry_row.html` + `_sidebar_unread.html` (multi-target) |
| `POST /entries/{id}/read` | Toggle read, returns updated row + sidebar. | same |
| `POST /entries/{id}/summarize` | Trigger summarization (existing logic), returns updated reading pane. | `_reading_pane.html` |
| `GET /entries?after={cursor}&fragment=1` | Load-More appending. | sequence of `_entry_row.html` |
| `GET /sidebar/unread` | Polling target. | `_sidebar_unread.html` |

**Internal JSON API** kept for:
- `PUT /api/user/settings/theme` (theme switcher).
- Passkey lifecycle: `GET /api/passkeys`, `POST /api/passkeys/...`,
  `DELETE /api/passkeys/{id}`. WebAuthn (`navigator.credentials.*`)
  is inherently JS-driven; the SSR `/user-settings` page hosts a
  small passkey section that talks to these endpoints.
- Anything still required after auditing during each per-page PR.
  Most others (`/api/sidebar`, `/api/feeds`, `/api/statistics`,
  `/api/me`, `/api/server-config`, `/api/user-settings`) are
  deleted as their last consumer goes SSR.

**GReader API** unchanged.

### Caching layers

1. **HTTP (`middleware/etag.rs`).** Compute weak ETag = body hash on
   2xx HTML responses. Honor `If-None-Match` → 304. Apply on SSR
   pages and fragment endpoints. Logged-in pages use
   `Cache-Control: private, must-revalidate` — bfcache-friendly,
   no mid-box caching.
2. **Compression.** `tower-http`'s `CompressionLayer` already on;
   add `compression-br` Cargo feature and chain `.br(true)`.
3. **In-process LRU (`services/cache.rs`).** Wrap `moka::future::Cache`
   keyed by `(user_id, kind, params...)`:
   - `sidebar_cache(user_id)` → sidebar tree payload.
   - `feeds_cache(user_id, filter, sort)` → feeds list rows.
   - `stats_cache(user_id, period)` → statistics rollup.
   Model-layer CRUD paths invalidate the relevant keys explicitly.
4. **bfcache.** Avoid `Cache-Control: no-store`. Avoid `unload`
   listeners (only `pageshow` for bfcache restore).

### Data flow examples

**Reading-pane swap:**
```
click <a data-swap="#reading-pane" href="/entries/42/fragment">
  → app.js intercepts, fetch /entries/42/fragment
    → handler: read entry from DB, render _reading_pane.html
    → ETag middleware: hash body, send 200 + ETag (or 304)
  → app.js: outerHTML replace #reading-pane
  → form/links inside new fragment are wired automatically
    (delegation, not per-mount)
```

**Star toggle (with sidebar piggyback):**
```
click <button> inside <form data-swap="#row-42" action="/entries/42/star" method="post">
  → app.js intercepts, fetch POST /entries/42/star
    → handler: flip star, invalidate sidebar_cache(user)
    → response body:
        <template data-swap-target="#row-42">…_entry_row.html…</template>
        <template data-swap-target="#sidebar-unread">…_sidebar_unread.html…</template>
  → app.js: swap each target listed
```

**Sidebar polling:**
```
setInterval(20s) fetch /sidebar/unread
  → handler: read sidebar_cache, render _sidebar_unread.html
  → ETag check ⇒ 304 if unchanged, no swap
  → app.js: outerHTML replace #sidebar-unread on 200
```

## Error handling

- Handlers continue to return `Result<T, AppError>`. SSR error path
  renders `error.html` (replacing the prior JSON error path).
- Fragment endpoints return 4xx + text on failure. The `swap()`
  helper detects non-2xx and shows an inline error toast in the
  flash region. Form fallback is native submit (full reload).
- Reading-pane fragment 404/403 clears the pane and shows "entry
  not found".
- Cache correctness: every CRUD op invalidates explicitly; ETag is
  computed from final body, so cache misses surface as
  `If-None-Match` mismatch, never stale HTML.

## Testing

- **Rust integration (`cargo nextest`):** primary safety net. Each
  SSR page handler, each fragment endpoint, ETag 304 behavior, and
  LRU invalidation gets coverage. Existing handler tests are
  retained; new tests added per-PR.
- **Playwright e2e:** retained only for JS-driven behaviors that
  Rust tests cannot verify — auth flow, reading-pane swap on click,
  star/mark-read swap, keyboard shortcuts. Drop:
  - `ssr-no-double-render.spec.ts` (CSR shell removed; assertion
    no longer meaningful).
  - `e2e/scripts/css-coverage.spec.ts` (per user decision —
    measurement not paying off in practice).
  - keyboard / search / pagination / category-management specs that
    duplicate Rust integration coverage.

## Migration plan

Long-running, per-page PRs (mirrors the prior CSR migration cadence
in reverse):

| # | Scope | Risk |
|---|-------|------|
| 1 | **Foundation.** Add `swap()` helper, ETag middleware, LRU primitives, brotli. Add `base.html` + shared partials. **Touch nothing on existing routes.** | Low (pure addition) |
| 2 | **Shell teardown.** Remove `app_shell.html` shell + SPA router + page-host model from the rendering pipeline. All 13 routes still mount their CSR page-element under the new `base.html`, temporarily. | Medium (renderer surface) |
| 3 | `/settings` SSR | Low |
| 4 | `/user-settings` SSR. Account, preferences, integrations form-ized; passkey register/list/delete keeps a small dedicated JS section because WebAuthn requires `navigator.credentials.create()`. The passkey JSON endpoints (`/api/passkeys`, etc.) stay. | Medium |
| 5 | `/admin` SSR | Low |
| 6 | `/statistics` SSR | Low |
| 7 | `/categories` SSR | Low |
| 8 | `/feeds` SSR (incl. add/edit/delete/import/export form-ization) | Medium |
| 9 | `/search` SSR | Low |
| 10 | entries family — `/`, `/entries`, `/entries/{read,starred,summarized}` SSR with reading pane + swap | High |
| 11 | entries family — `/feeds/{id}/entries`, `/categories/{id}/entries` SSR | Medium |
| 12 | **Cleanup.** Delete `app_shell.html`, `static/js/components/`, `static/js/pages/`, `router.js`, unused `/api/*`, unused CSS. Drop e2e specs per the testing section. Tighten `static_assets.rs` allowlist to `app.css` + `app.js` + `passkey.js`. | Medium (mass deletion) |

Steps 3-9 are mechanical (template + handler) once the foundation
is in place. Step 10 is the largest single-page move and is
deliberately scheduled after the swap helper has been exercised.

Each PR is independently mergeable: between PRs the repo is in a
consistent (mixed) state — old CSR pages still work, new SSR pages
work.

## Repo conventions for new SSR pages

- One template per route under `templates/` plus shared partials in
  `templates/_*.html`.
- Handler in `src/handlers/pages.rs` returns `(Flash, T)` where
  `T: IntoResponse + Template`. Flash extractor consumed.
- Per-user data passed in template context, not bootstrap blobs.
- Fragment endpoints share their partial with the full-page render
  (`{% include "_partial.html" %}`).
- Action handlers (POST `*/star`, `*/read`, etc.) invalidate
  relevant LRU keys before responding.
- `static/js/app.js` is the only logged-in JS file. Add to
  `static_assets.rs` allowlist if a new asset is introduced
  (preferable: don't introduce one).

## Open questions

None at design time. Per-page PRs may surface specifics
(e.g. exact list of LRU keys, cursor encoding for Load-More) and
will be answered in the per-page implementation plans.
