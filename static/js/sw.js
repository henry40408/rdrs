/**
 * Service worker: offline fallback and a cache for versioned static assets.
 * Served from the site root so its scope covers navigations.
 *
 * The Cache API ignores `no-store`/`Vary: Cookie`, so writes are an allowlist:
 * only same-origin GETs under `/static/` plus the precached `/offline` page go
 * into `rdrs-shell-<version>`. Everything else passes straight to the network.
 *
 * `rdrs-offline-<key>` (saved articles, opt-in) is written only by offline.js;
 * this worker just reads it for navigations and `<img>` loads, which a page
 * cannot rescue itself, and deletes it on sign-out.
 *
 * Static assets are safe to store because they are cookie-free and `?v=`-stamped
 * — except on dev builds, see [`CACHE_STATIC_ASSETS`].
 */

const VERSION = '__RDRS_ASSET_VERSION__';
const CACHE = `rdrs-shell-${VERSION}`;
const OFFLINE_URL = '/offline';

/** Prefix of the saved-article caches (written by offline.js, dropped on sign-out). */
const OFFLINE_PREFIX = 'rdrs-offline-';

/** The page listing what those caches hold. See `pages::offline_entries_page`. */
const LIBRARY_URL = '/entries/offline';

/**
 * Whether `/static/` responses may be kept; substituted server-side from
 * `cache_control_for`. False for dirty builds, whose `?v=` never changes between
 * edits. Written as a comparison so the file is valid JS before substitution.
 * The precache still runs regardless.
 */
const CACHE_STATIC_ASSETS = '__RDRS_CACHE_STATIC__' === 'true';

/**
 * Just enough for a legible offline page. `app.js` etc. are only reachable via
 * navigations, which fail offline, so precaching them would buy nothing.
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
      // Offsets the latency the worker adds to every navigation; not supported everywhere.
      if (self.registration.navigationPreload) {
        await self.registration.navigationPreload.enable();
      }
      // Keep saved articles across deploys; they are data, not assets.
      const names = await caches.keys();
      await Promise.all(
        names
          .filter((name) => name !== CACHE && !name.startsWith(OFFLINE_PREFIX))
          .map((name) => caches.delete(name)),
      );
      await self.clients.claim();
    })(),
  );
});

/**
 * The reader's saved-article cache, or `null`. Scanned each time because the
 * worker may restart at any moment; if more than one matches, take none.
 */
async function offlineCache() {
  const names = (await caches.keys()).filter((name) => name.startsWith(OFFLINE_PREFIX));
  return names.length === 1 ? caches.open(names[0]) : null;
}

/** A saved copy of `key`, or `undefined`. */
async function savedResponse(key) {
  const cache = await offlineCache();
  return cache ? cache.match(key) : undefined;
}

/**
 * Network-first, then the offline library (entry-list paths only), then the
 * precached offline page.
 */
async function handleNavigation(event) {
  try {
    const preloaded = await event.preloadResponse;
    if (preloaded) {
      return preloaded;
    }
    return await fetch(event.request);
  } catch (error) {
    const path = new URL(event.request.url).pathname;
    if (path === '/' || path === LIBRARY_URL || path.startsWith('/entries')) {
      const library = await savedResponse(LIBRARY_URL);
      if (library) {
        return library;
      }
    }
    const cached = await caches.match(OFFLINE_URL);
    if (cached) {
      return cached;
    }
    // Nothing to fall back to; let the browser show its own error.
    throw error;
  }
}

/**
 * Network-first, then a saved copy. Never populates the cache: offline.js owns
 * what fits the reader's budget.
 */
async function handleSavedImage(request, key) {
  try {
    return await fetch(request);
  } catch (error) {
    const saved = await savedResponse(key);
    if (saved) {
      return saved;
    }
    throw error;
  }
}

/**
 * Pass a sign-out through and, if it succeeded, drop every saved article. Only
 * the worker sees every sign-out path (scripted, no-JS, other tabs).
 */
async function handleSignOut(request) {
  const response = await fetch(request);
  if (response.status < 500) {
    const names = await caches.keys();
    await Promise.all(
      names.filter((name) => name.startsWith(OFFLINE_PREFIX)).map((name) => caches.delete(name)),
    );
  }
  return response;
}

/**
 * Cache-first (skipped on dev builds), then network, then the reader's saved
 * copy — so saved articles render styled offline without ever serving an online
 * reader a stale asset.
 */
async function handleStaticAsset(request, key) {
  if (CACHE_STATIC_ASSETS) {
    const cached = await (await caches.open(CACHE)).match(request);
    if (cached) {
      return cached;
    }
  }
  try {
    const response = await fetch(request);
    // Only `basic` (same-origin, readable) responses; others would poison the cache.
    if (CACHE_STATIC_ASSETS && response.ok && response.type === 'basic') {
      const cache = await caches.open(CACHE);
      cache.put(request, response.clone());
    }
    return response;
  } catch (error) {
    const saved = await savedResponse(key);
    if (saved) {
      return saved;
    }
    throw error;
  }
}

/** Images a saved article points at: proxied pictures and feed favicons. */
const SAVED_IMAGE_PATH = /^\/(api\/proxy\/image|api\/feeds\/\d+\/icon)$/;

self.addEventListener('fetch', (event) => {
  const { request } = event;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    return;
  }
  if (request.method !== 'GET') {
    // Not cached; a sign-out just triggers dropping saved articles.
    // `DELETE /api/session` is the scripted path, `POST /logout` the no-JS one.
    if (
      (request.method === 'POST' && url.pathname === '/logout') ||
      (request.method === 'DELETE' && url.pathname === '/api/session')
    ) {
      event.respondWith(handleSignOut(request));
    }
    return;
  }
  if (request.mode === 'navigate') {
    event.respondWith(handleNavigation(event));
    return;
  }
  if (url.pathname.startsWith('/static/')) {
    event.respondWith(handleStaticAsset(request, url.pathname + url.search));
    return;
  }
  if (SAVED_IMAGE_PATH.test(url.pathname)) {
    event.respondWith(handleSavedImage(request, url.pathname + url.search));
  }
  // Everything else goes to the network untouched (allowlist).
});
