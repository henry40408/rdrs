/**
 * Service worker: offline fallback and a cache for versioned static assets.
 *
 * Served from the site root rather than from beside the other scripts: a
 * worker's default scope is the directory it was served from, and one scoped to
 * the static-asset prefix could never see the navigations it exists to catch.
 *
 * ## What it must never cache
 *
 * Every logged-in response is `Cache-Control: no-store` + `Vary: Cookie`, and the
 * Cache API honours neither: `cache.put` stores whatever it is handed. So the
 * rule is enforced here instead, as an allowlist rather than a denylist —
 * only same-origin `GET`s under `/static/` are stored, plus the one precached
 * `/offline` page, which carries no user data by construction. Navigations,
 * `/api/*`, `/events`, feed icons and proxied images are passed straight to the
 * network and their responses are never written anywhere. A denylist would have
 * to stay in sync with every route added later; this cannot drift.
 *
 * ## Why static assets are safe to store
 *
 * They are cookie-free (`/static` is skipped by the session, CSRF and
 * forward-auth layers) and their URLs carry the build's `?v=` stamp under an
 * `immutable` header, so a cache entry can never be stale for its URL. The whole
 * cache is dropped on activate anyway, keyed by the same version.
 *
 * That argument rests entirely on the stamp changing when the bytes do, which is
 * not true of a development build — see [`CACHE_STATIC_ASSETS`], which turns
 * this half off for exactly that case.
 */

const VERSION = '__RDRS_ASSET_VERSION__';
const CACHE = `rdrs-shell-${VERSION}`;
const OFFLINE_URL = '/offline';

/**
 * Whether `/static/` responses may be kept. Substituted server-side.
 *
 * False for a build from a working tree with uncommitted changes. `git describe
 * --dirty` gives every such build the *same* version string, so the `?v=` stamp
 * — and with it this cache's key — is identical across consecutive edits and can
 * never notice a rebuild. The server already drops those responses to
 * `no-cache` for exactly that reason (`cache_control_for` in
 * `static_assets.rs`); without the same opt-out here the worker would hand the
 * stale copy straight back, and editing a stylesheet would appear to do nothing.
 *
 * Decided in Rust, off that same function, rather than re-derived from the
 * version string here: two copies of one rule is how they drift apart. Written
 * as a comparison so this file is still valid JavaScript before substitution.
 *
 * Only *runtime* caching is affected. The precache below still runs, so the
 * offline page stays testable locally — it is then a snapshot from first load,
 * and iterating on `offline.html` itself needs the worker unregistered.
 */
const CACHE_STATIC_ASSETS = '__RDRS_CACHE_STATIC__' === 'true';

/**
 * The minimum needed to render a legible offline page: the page itself and the
 * stylesheet. Deliberately not the whole app shell — `app.js` and friends are
 * only reachable from navigations, which fail offline, so precaching them would
 * buy nothing and cost every visitor the download up front. Everything else
 * under `/static/` lands in the same cache on first use instead, when
 * [`CACHE_STATIC_ASSETS`] allows it.
 */
const PRECACHE_URLS = [OFFLINE_URL, `/static/css/app.css?v=${VERSION}`];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(PRECACHE_URLS))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    (async () => {
      // The worker sits in front of every navigation, so it also has to pay back
      // the latency it adds: without this the request cannot start until the
      // worker has booted. Not supported everywhere, hence the guard.
      if (self.registration.navigationPreload) {
        await self.registration.navigationPreload.enable();
      }
      const names = await caches.keys();
      await Promise.all(
        names.filter((name) => name !== CACHE).map((name) => caches.delete(name)),
      );
      await self.clients.claim();
    })(),
  );
});

/** Network-first, with the precached offline page as the only fallback. */
async function handleNavigation(event) {
  try {
    const preloaded = await event.preloadResponse;
    if (preloaded) {
      return preloaded;
    }
    return await fetch(event.request);
  } catch (error) {
    const cached = await caches.match(OFFLINE_URL);
    if (cached) {
      return cached;
    }
    // Nothing to fall back to; let the browser show its own error page rather
    // than inventing a worse one.
    throw error;
  }
}

/** Cache-first. Safe only because these URLs are version-stamped and public. */
async function handleStaticAsset(request) {
  const cached = await caches.match(request);
  if (cached) {
    return cached;
  }
  const response = await fetch(request);
  // `basic` means same-origin and fully readable — an opaque or error response
  // would poison the cache with something that can never be served correctly.
  if (response.ok && response.type === 'basic') {
    const cache = await caches.open(CACHE);
    cache.put(request, response.clone());
  }
  return response;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') {
    return;
  }
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    return;
  }
  if (request.mode === 'navigate') {
    event.respondWith(handleNavigation(event));
    return;
  }
  if (CACHE_STATIC_ASSETS && url.pathname.startsWith('/static/')) {
    event.respondWith(handleStaticAsset(request));
  }
  // Anything else falls through to the network untouched — see the module note
  // on why this is an allowlist.
});
