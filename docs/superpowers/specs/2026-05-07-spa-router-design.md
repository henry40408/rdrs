# SPA Router Design

**Date:** 2026-05-07
**Status:** Approved (brainstorming, auto mode)
**Predecessor:** [`2026-05-06-ssr-to-csr-migration-design.md`](./2026-05-06-ssr-to-csr-migration-design.md), [`2026-05-07-csr-entries-design.md`](./2026-05-07-csr-entries-design.md)

## Goal

Replace cross-page full-reload navigation with in-place client-side routing for every CSR page. After this lands, clicking the sidebar or tabs no longer triggers a `Document` reload — the browser keeps the same JS context, the appropriate `<rdrs-X-page>` element is swapped in, and `history.pushState` updates the URL.

The user-visible win is the elimination of the brief blank/reflow flash between pages. Behind that, the JS runtime lifetime extends across navigations, which makes future caching, prefetch, and persistent state easier to add.

## Non-goals

- Service Worker / offline support — still out of scope.
- Login / register — they remain SSR (step 7 was skipped per design discussion 2026-05-07).
- New visual design or feature changes.
- Replacing the server-side route handlers — they continue to serve the shell on full-reload entry points.
- Single-bundle JS — page modules continue to be separate ES modules dynamically imported on demand.

## Architecture

### Single router module

A new `static/js/router.js` is loaded by `app_shell.html` (after the page-element module). It owns:

- Click interception on document-level `<a>` clicks.
- A hard-coded route table mapping URL patterns to `(element_tag, script_path)`.
- `popstate` handling for browser back/forward.
- `navigateTo(path, opts)` — the single entry point for in-app navigation.

Page-element code does NOT import the router. Page modules emit standard `<a href="/...">` links and the router intercepts at the document level.

### Route table

The router's table mirrors the server's URL → handler map. It is hard-coded — these routes don't change at runtime:

```js
const ROUTES = [
  { pattern: /^\/$/,                                      element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/entries$/,                               element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/entries\/(?:read|starred|summarized)$/,  element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/feeds\/\d+\/entries$/,                   element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/categories\/\d+\/entries$/,              element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/search$/,                                element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
  { pattern: /^\/feeds$/,                                 element: 'rdrs-feeds-page',         script: '/static/js/pages/feeds.js' },
  { pattern: /^\/categories$/,                            element: 'rdrs-categories-page',    script: '/static/js/pages/categories.js' },
  { pattern: /^\/admin$/,                                 element: 'rdrs-admin-page',         script: '/static/js/pages/admin.js' },
  { pattern: /^\/settings$/,                              element: 'rdrs-settings-page',      script: '/static/js/pages/settings.js' },
  { pattern: /^\/user-settings$/,                         element: 'rdrs-user-settings-page', script: '/static/js/pages/user-settings.js' },
  { pattern: /^\/statistics$/,                            element: 'rdrs-statistics-page',    script: '/static/js/pages/statistics.js' },
];
```

Path with query string (e.g. `/search?q=foo`) is matched on the pathname only; the query stays in the URL for the page to read on connect.

A path that matches nothing (e.g. `/login`, an external link, `/entries/{id}`'s redirect handler) falls through to a normal full-page navigation.

### Click interception

Single capturing listener on `document`:

```js
document.addEventListener('click', (e) => {
  const a = e.target.closest('a');
  if (!a) return;
  if (a.target && a.target !== '_self') return;          // _blank, _parent etc. → browser handles
  if (a.hasAttribute('download')) return;
  if (a.getAttribute('rel')?.includes('external')) return;
  if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;  // cmd-click, etc.
  if (e.button !== 0) return;                            // right click, middle click

  const url = new URL(a.href, location.origin);
  if (url.origin !== location.origin) return;            // external

  const route = ROUTES.find(r => r.pattern.test(url.pathname));
  if (!route) return;                                    // /login etc. → let browser navigate

  e.preventDefault();
  navigateTo(url.pathname + url.search, { route });
});
```

`a` elements rendered inside reading panes (e.g. entry content's `<a href="https://...">`) are external; the cross-origin check filters them out.

### popstate

```js
window.addEventListener('popstate', () => {
  navigateTo(location.pathname + location.search, { skipPushState: true });
});
```

`replaceState` calls (used by entries-page for `?entry=N` deep links and by search/feed/category for `?status=` / `?q=` filters) don't fire popstate, so they don't interact with the router.

### `navigateTo(path, opts)`

```js
async function navigateTo(path, opts = {}) {
  const route = opts.route ?? ROUTES.find(r => r.pattern.test(new URL(path, location.origin).pathname));
  if (!route) { location.href = path; return; }

  // Push history entry first so popstate during async import doesn't get
  // confused by the user clicking again. We never push for popstate-driven
  // calls.
  if (!opts.skipPushState) {
    history.pushState(null, '', path);
  }

  try {
    await import(route.script);                          // dedup'd by URL
  } catch (err) {
    // Module fetch failed (network, server 5xx). Fall back to full reload —
    // the user gets either the server-rendered shell or a server error
    // page, both of which are recoverable.
    location.href = path;
    return;
  }

  const host = document.getElementById('page-host');
  const newEl = document.createElement(route.element);
  host.replaceChildren(newEl);

  if (!opts.skipPushState) {
    window.scrollTo(0, 0);
  }
  // popstate keeps the browser-restored scroll position.
}
```

`page-host` is a new wrapper div introduced in `templates/app_shell.html` around the page element so the router has a stable DOM target. The element is created fresh each navigation so `connectedCallback` runs and the page does its normal setup (sidebar bootstrap read, list fetch, etc.) — the router does NOT cache page-element instances.

### `app_shell.html` changes

Two small edits:

1. Wrap `<{{ element_tag }}></{{ element_tag }}>` in `<div id="page-host">…</div>`.
2. Add `<script type="module" src="/static/js/router.js?v={{ git_version }}"></script>` after the page-module script.

The router script imports nothing else from the app — it's pure URL/DOM/history. It does NOT depend on `<rdrs-entries-page>` being defined.

### First paint (full-reload entry)

The server continues to:

1. Serve the appropriate shell HTML with `element_tag` / `script_path` matching the URL.
2. Inline the sidebar bootstrap and flash bootstrap JSON.
3. Set the theme attribute on `<html>`.

The router is loaded after the page module. By the time it parses, the initial page element is already mounted. The router does nothing on initial load — it just attaches its click + popstate listeners and waits.

This means existing handler tests continue to pass unchanged. The router is purely additive on the client side.

### Sidebar refresh strategy

The router does NOT touch sidebar bootstrap. Sidebar (rendered by `<rdrs-sidebar>`) has its own update lifecycle:

- Read bootstrap on first paint.
- Cache in `sessionStorage` for snappy back/forward.
- Refresh from `/api/sidebar` when `_invalidateSidebar()` is called (e.g. after mark-as-read in `<rdrs-entry-list>`).

Cross-page navigation that *should* invalidate the sidebar (e.g. visiting `/categories` and adding a new category) is handled inside the page-element's CRUD flow, same as today. The router doesn't need to know.

### Flash bootstrap on SPA nav

Server-side flash is tied to the cookie. SPA navigation doesn't touch the cookie, so server-set flash is invisible during in-app nav. This is fine — every existing CRUD flow already shows flashes via `flash.success()` / `flash.error()` from JS, not via redirect+cookie.

The one cookie-dependent flow is `flash.redirect(url, level, message)` (used after logout, etc.). Those calls do `location.href = url` — full reload — which falls outside the router. They keep working.

### Scroll behavior

- `pushState` (forward navigation): `window.scrollTo(0, 0)` — fresh page starts at the top.
- `popstate`: do nothing — the browser auto-restores the previous scroll position via `history.scrollRestoration = 'auto'` (default).

This matches what users expect from the major SPA frameworks.

### Cross-origin / external links

External `<a>` elements (entry content, "View original" buttons in reading pane) bypass the router via the `url.origin !== location.origin` check. They open in the same tab unless they have `target="_blank"` (which the click handler also bypasses).

`/api/feeds/{id}/icon` and other server-rendered images / `<img>` srcs aren't `<a>` elements — they don't go through the click handler at all.

## Components

| File | Status | Responsibility |
|------|--------|---------------|
| `static/js/router.js` | NEW | Click/popstate interception, route table, `navigateTo()`, page-element swap |
| `static/js/components/rdrs-flash.js` | EDIT | Tiny helper export — see below |
| `templates/app_shell.html` | EDIT | Wrap page-element in `#page-host`, load router module |
| `src/handlers/static_assets.rs` | EDIT | Register `js/router.js` in the FILES allowlist |
| `e2e/tests/spa-router.spec.ts` | NEW | Verify in-place navigation across every CSR route, popstate restore, click filters, fallback path |

### `rdrs-flash.js` helper

`flash.redirect(url, level, message)` currently does `location.href = url`. To allow callers (logout, masquerade end) to opt-in to SPA nav for in-app destinations, expose an alternative:

```js
flash.success(message);                  // in-place
flash.redirect('/login', 'info', '...'); // full reload (cross-SSR boundary, e.g. logout)
flash.spaNavigate('/', 'success', '...'); // SPA nav with deferred flash
```

`spaNavigate` queues the flash in sessionStorage, calls `router.navigateTo(url)`, then the next page's `<rdrs-flash>` flushes the queue on connect. This is purely an enhancement — no existing call sites need changing. We add it only if needed during implementation.

## Data flow

```
User clicks <a href="/feeds">
  ↓
document.click handler matches route → e.preventDefault()
  ↓
navigateTo('/feeds')
  ├─ history.pushState(null, '', '/feeds')
  ├─ await import('/static/js/pages/feeds.js')   ← cached after first import
  ├─ #page-host.replaceChildren(<rdrs-feeds-page>)
  └─ window.scrollTo(0, 0)
  ↓
<rdrs-feeds-page>.connectedCallback() runs
  ├─ Reads sidebar bootstrap (still inlined from initial paint)
  ├─ Fetches /api/feeds
  └─ Renders
```

Browser back from `/feeds` → `/`:

```
popstate fires (URL is now '/')
  ↓
navigateTo('/', { skipPushState: true })
  ├─ await import('/static/js/pages/entries.js')   ← already cached
  ├─ #page-host.replaceChildren(<rdrs-entries-page>)
  └─ scroll position auto-restored by browser
  ↓
<rdrs-entries-page>.connectedCallback() runs
  ├─ inferMode() returns 'unread'
  └─ Mounts <rdrs-entry-list>, fetches stream/contents
```

## Error handling

| Failure | Behavior |
|---------|----------|
| `import(route.script)` rejects | `location.href = path` → server-rendered shell or error page |
| Page-element `connectedCallback` throws | Browser logs the error; router does nothing further (the user sees a partially-rendered page; refresh fixes it). Acceptable — same as today's first-paint failure mode |
| Route pattern doesn't match | Click bypasses router; browser navigates normally |
| `/api/*` returns 401 mid-page (session expired) | Page-level error handlers redirect to `/login` via `location.href`. Unchanged from today |

## Testing

### New e2e spec: `e2e/tests/spa-router.spec.ts`

Coverage:

- **Same-element nav**: `/` → `/entries` → `/entries/starred` — verify `Document` request count stays 1 (no full reload), URL updates, list re-renders.
- **Cross-element nav**: `/` → `/feeds` → `/admin` → `/` — element tag changes between `rdrs-entries-page`, `rdrs-feeds-page`, `rdrs-admin-page`. No full reload.
- **Browser back/forward**: navigate forward, press back, verify URL + element correct, scroll restored.
- **Click filters**: cmd-click (open in new tab), `target="_blank"`, external link in reading pane — all bypass router.
- **Fallback**: clicking `/login` from a CSR page does a full reload (route doesn't match).
- **404 deep link**: visiting `/feeds/9999/entries` directly still 404s server-side; SPA nav from `/feeds` to `/feeds/9999/entries` triggers the page element's own 404 path (it fetches `/api/feeds`, doesn't find id 9999, shows the "Feed not found" header — already implemented in B2).

### Existing e2e specs

Unchanged. They use `page.goto()` which always triggers full navigation, so the router is invisible to them. The `ssr-no-double-render.spec.ts` block 1's `count == 1` assertion still holds because `page.goto()` fires a fresh document load.

### Unit tests

The router has no Rust counterpart. JS-only logic; coverage via Playwright e2e.

## Risks

- **Module dedup hinges on stable URL**. `app_shell.html` versions URLs via `?v={{ git_version }}`. Router uses the same versioned URL only if it reads `git_version` from somewhere. To keep the router simple, the dynamic `import()` call uses the *unversioned* URL (e.g. `/static/js/pages/feeds.js`). The browser caches this separately from the shell's versioned import. On first nav after a deploy, the router fetches a fresh copy — wasted bytes but correctness preserved. Acceptable.

  Alternative considered: thread `git_version` through to the router via a `<meta>` tag. Rejected as premature optimization.

- **`<rdrs-sidebar>` stale unread counts during long sessions**. Already a problem today (sidebar bootstrap caches, doesn't auto-refresh). Not introduced by the router. Out of scope; can be addressed separately by adding a poll or a "refresh on visibilitychange" hook to `<rdrs-sidebar>`.

- **Page-element memory leaks**. Page elements that wire global event listeners (popstate, pageshow) MUST clean up in `disconnectedCallback`. `<rdrs-entry-list>` already does this. New page elements added after the router lands MUST follow the same pattern. Documented in the existing `static/js/components/` files; reinforced in PR review.

- **Race**: user clicks link A, `await import(A)` is in flight, user clicks link B. The naive code would mount A's element after B's `pushState` already fired. Mitigated by storing a per-call sequence number and discarding stale completions.

  ```js
  let navSeq = 0;
  async function navigateTo(...) {
    const mySeq = ++navSeq;
    ...
    await import(...);
    if (mySeq !== navSeq) return;   // superseded
    host.replaceChildren(...);
  }
  ```

## Open questions

None. Q&A on 2026-05-07 settled scope (full SPA, all 13 CSR routes).
