/**
 * Service worker: offline fallback and a cache for versioned static assets.
 *
 * Served from the site root rather than from beside the other scripts: a
 * worker's default scope is the directory it was served from, and one scoped to
 * the static-asset prefix could never see the navigations it exists to catch.
 *
 * ## What it may cache, and what it must never
 *
 * Every logged-in response is `Cache-Control: no-store` + `Vary: Cookie`, and the
 * Cache API honours neither: `cache.put` stores whatever it is handed. So the
 * rule is enforced here instead, as an allowlist rather than a denylist. This
 * worker writes to exactly one cache, `rdrs-shell-<version>`, and only
 * same-origin `GET`s under `/static/` plus the precached `/offline` page ever
 * go into it. Navigations, `/api/*`, `/events`, feed icons and proxied images
 * are passed straight to the network and their responses are never written
 * here. A denylist would have to stay in sync with every route added later;
 * this cannot drift.
 *
 * ## The offline caches, which this worker only reads
 *
 * `rdrs-offline-<key>` holds the reader's saved articles. It is opt-in
 * (`offline_keep`, off by default), namespaced by an opaque per-user key, and
 * **written entirely by `static/js/offline.js`** — a page has the session, the
 * budget and the manifest, and splitting the decision of what to keep across
 * two files is how the two halves come to disagree.
 *
 * The worker reads from it for the two request kinds a page cannot rescue on
 * its own: navigations, which replace the document before any script of ours
 * runs, and `<img>` loads, which no page-level code can retry. A reading pane
 * is neither — `performSwap` issues that fetch itself and asks `offline.js` for
 * the saved copy when it fails, which keeps the request page-originated and so
 * still visible to anything watching the network. The worker also deletes every
 * one of these caches the moment it sees a sign-out.
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
 * Name prefix of the reader's saved-article caches. Written by `offline.js`,
 * read here, and dropped wholesale on sign-out.
 */
const OFFLINE_PREFIX = 'rdrs-offline-';

/** The page listing what those caches hold. See `pages::offline_entries_page`. */
const LIBRARY_URL = '/entries/offline';

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
      // Everything but this build's shell cache goes — except the reader's
      // saved articles, which are data rather than assets. Keying those by
      // build version would throw away the whole offline library on every
      // deploy, which is precisely when a reader is least able to rebuild it.
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
 * The reader's saved-article cache, or `null` when offline reading is off.
 *
 * Found by scanning rather than remembered: a worker is torn down and restarted
 * at the browser's discretion, so anything it "knows" between events has to be
 * re-derived. `offline.js` deletes every cache but the current account's before
 * its first network call of a page load, so in practice at most one matches —
 * and if a second somehow lingers, taking none is the safe answer rather than
 * guessing which reader it belongs to.
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
 * Network-first, falling back to the reader's offline library and then to the
 * precached apology page.
 *
 * The library stands in for the entry-list paths only. Answering *every* dead
 * navigation with it would put a list of articles under the URL of the feeds
 * page or the settings page, which is a worse lie than an honest error.
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
    // Nothing to fall back to; let the browser show its own error page rather
    // than inventing a worse one.
    throw error;
  }
}

/**
 * Network-first, falling back to a saved copy — the same shape as
 * [`handleFragment`], and for the same reason: the cache is only ever consulted
 * on the failure path, so an online reader pays nothing for a feature they may
 * not even have switched on.
 *
 * Deliberately *not* "populate on first use" like the static assets either.
 * What belongs in the offline cache is decided by `offline.js` against the
 * reader's budget; a worker that quietly added every image scrolled past would
 * be spending that budget behind their back.
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
 * Pass a sign-out through and, if it took, drop every saved article with it.
 *
 * This is the only place a sign-out is reliably observable: it can be triggered
 * from the scripted nav, the scriptless fallback form or a session revoked in
 * another tab, and only the worker sees all three. A network failure leaves the
 * caches alone — the reader is still signed in.
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
 * Cache-first, then network, then the reader's saved copy.
 *
 * The first hop is safe only because these URLs are version-stamped and public,
 * and is skipped entirely on a dev build — see [`CACHE_STATIC_ASSETS`].
 *
 * The last hop is what makes a saved article *readable* rather than merely
 * present: the library page is server-rendered markup that still needs
 * `app.css` and `app.js` to look like the app and to open an entry at all.
 * `offline.js` puts them in the reader's own cache for exactly this, and
 * reaching for them only after the network has failed means an online reader
 * can never be served a stale asset — which is the one thing the dev-build
 * opt-out above exists to prevent.
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
    // `basic` means same-origin and fully readable — an opaque or error response
    // would poison the cache with something that can never be served correctly.
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
    // The one non-GET the worker looks at, and it does not cache the response —
    // it uses the fact that a sign-out happened to throw the saved articles
    // away. `DELETE /api/session` is the scripted path, `POST /logout` the
    // scriptless one.
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
  // Anything else falls through to the network untouched — see the module note
  // on why this is an allowlist.
});
