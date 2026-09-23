/**
 * Offline reading: mirror the reader's queue into the service worker's cache.
 *
 * Stores the server's own `GET /entries/{id}/fragment` markup, so offline
 * swaps are identical; this module only decides *what* to hold. Opt-in,
 * bounded by `offline_keep`, and namespaced per user so nothing survives a
 * sign-out on a shared device. The cache keys are the ledger (no second index);
 * each response carries its `updated_at` in `x-rdrs-offline-version`.
 */

const CACHE_PREFIX = 'rdrs-offline-';
const MANIFEST_URL = '/api/offline/manifest';
const LIBRARY_URL = '/entries/offline';

/** Where a stored entry's `updated_at` lives. */
const VERSION_HEADER = 'x-rdrs-offline-version';

/**
 * Image ceilings. The budget is spent newest-first, so oldest articles lose
 * their images rather than the sync failing.
 */
const MAX_IMAGE_BYTES = 2 * 1024 * 1024;
const MAX_SYNC_IMAGE_BYTES = 48 * 1024 * 1024;

/**
 * Fraction of quota above which sync stops writing: running out evicts the
 * whole origin, sessionStorage included.
 */
const QUOTA_HEADROOM = 0.8;

/** The canonical cache key for an entry — the URL `app.js` actually requests. */
function fragmentPath(id) {
  return `/entries/${id}/fragment`;
}

/**
 * The URL the sync fetches; `offline=1` stops it marking every entry read.
 * Stored under [`fragmentPath`] so the reader's own click matches.
 */
function prefetchUrl(id) {
  return `${fragmentPath(id)}?offline=1`;
}

/**
 * The reader's cache key as rendered into the page. The manifest's budget wins
 * over `data-offline-keep`, which may be stale.
 */
function pageCacheKey() {
  return document.documentElement.dataset.offlineKey || '';
}

/**
 * Drop every offline cache that is not `key`'s. Runs before the first network
 * call of each page load, the earliest point after an account switch.
 */
async function dropForeignCaches(key) {
  const mine = key ? CACHE_PREFIX + key : null;
  const names = await caches.keys();
  await Promise.all(
    names.filter((name) => name.startsWith(CACHE_PREFIX) && name !== mine).map((name) => caches.delete(name)),
  );
}

/**
 * Store `response` rebuilt from its body, dropping `Vary: Cookie` (would never
 * match the worker's cookieless request), `Set-Cookie` and `Cache-Control`.
 */
async function put(cache, url, response, version) {
  const body = await response.blob();
  const headers = new Headers();
  const type = response.headers.get('content-type');
  if (type) headers.set('Content-Type', type);
  if (version) headers.set(VERSION_HEADER, version);
  await cache.put(url, new Response(body, { status: 200, statusText: 'OK', headers }));
}

/** Whether the origin still has room. See [`QUOTA_HEADROOM`]. */
async function hasHeadroom() {
  if (!navigator.storage?.estimate) return true;
  try {
    const { usage = 0, quota = 0 } = await navigator.storage.estimate();
    return quota === 0 || usage / quota < QUOTA_HEADROOM;
  } catch {
    return true;
  }
}

/** Same-origin images an entry references: proxied images and the feed favicon. */
function imageUrls(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const urls = new Set();
  for (const img of doc.querySelectorAll('img[src]')) {
    try {
      const url = new URL(img.getAttribute('src'), location.origin);
      if (url.origin === location.origin) urls.add(url.pathname + url.search);
    } catch {
      // Unparseable `src`: unfetchable anyway.
    }
  }
  return urls;
}

/** Fetch and store one image, honouring the budget. Returns the bytes spent. */
async function cacheImage(cache, url, remaining) {
  if (await cache.match(url)) return 0;
  let response;
  try {
    response = await fetch(url, { credentials: 'same-origin' });
  } catch {
    return 0;
  }
  if (!response.ok || response.type !== 'basic') return 0;
  const declared = Number.parseInt(response.headers.get('content-length') || '0', 10);
  if (declared > MAX_IMAGE_BYTES || declared > remaining) return 0;
  const blob = await response.blob();
  if (blob.size > MAX_IMAGE_BYTES || blob.size > remaining) return 0;
  const headers = new Headers();
  const type = response.headers.get('content-type');
  if (type) headers.set('Content-Type', type);
  await cache.put(url, new Response(blob, { status: 200, statusText: 'OK', headers }));
  return blob.size;
}

/**
 * Asset extensions the static handler serves. [`referencesIn`] scans source
 * text, so this stops mentions in comments becoming requests.
 */
const ASSET_EXTENSION = /\.(?:js|css|woff2?|png|svg|ico|webmanifest)$/i;

/**
 * Same-origin `/static/` URLs referenced by an asset (CSS `url(…)`, JS imports),
 * resolved against `from`. Substring matches, not parses: each pattern runs only
 * on its own file type, and [`ASSET_EXTENSION`] is the backstop.
 */
function referencesIn(text, from, isStylesheet) {
  const base = new URL(from, location.origin);
  const found = new Set();
  const specs = isStylesheet
    ? [...text.matchAll(/\burl\(["']?([^)"']+)/g)]
    : [
        ...text.matchAll(/\bfrom\s+["']([^"']+)["']/g),
        ...text.matchAll(/\bimport\s+["']([^"']+)["']/g),
      ];
  for (const [, spec] of specs) {
    try {
      const url = new URL(spec, base);
      if (
        url.origin === location.origin &&
        url.pathname.startsWith('/static/') &&
        ASSET_EXTENSION.test(url.pathname)
      ) {
        found.add(url.pathname + url.search);
      }
    } catch {
      // A `data:` URL or not a URL at all.
    }
  }
  return found;
}

/**
 * Store the `/static/` assets a saved page needs (app.css to look right,
 * app.js to open entries) and return them. Walked transitively from the live
 * document so fonts and nested imports need no second list. The worker only
 * uses these after the network fails, so online readers never see stale copies.
 */
async function cacheShellAssets(cache) {
  const pending = [];
  for (const el of document.querySelectorAll('script[src], link[href]')) {
    const raw = el.getAttribute('src') || el.getAttribute('href');
    if (!raw) continue;
    try {
      const url = new URL(raw, location.origin);
      if (url.origin === location.origin && url.pathname.startsWith('/static/')) {
        pending.push(url.pathname + url.search);
      }
    } catch {
      // Not fetchable either.
    }
  }

  const seen = new Set();
  while (pending.length > 0) {
    const url = pending.pop();
    if (seen.has(url)) continue;
    seen.add(url);

    let stored = await cache.match(url);
    if (!stored) {
      try {
        const response = await fetch(url, { credentials: 'same-origin' });
        if (!response.ok || response.type !== 'basic') continue;
        await put(cache, url, response);
        stored = await cache.match(url);
      } catch {
        // Retry next sync.
        continue;
      }
    }

    // Only stylesheets and modules can reference anything; skip decoding fonts.
    const type = stored?.headers.get('content-type') || '';
    if (!/javascript|css/.test(type)) continue;
    const references = referencesIn(await stored.text(), url, type.includes('css'));
    for (const found of references) pending.push(found);
  }
  return seen;
}

/**
 * Bring the cache in line with the manifest: fetch missing/stale entries, drop
 * departed ones, and re-store the library page.
 */
async function sync() {
  if (!('caches' in window)) return;

  const pageKey = pageCacheKey();
  await dropForeignCaches(pageKey);
  if (!pageKey) return;

  let manifest;
  try {
    const response = await fetch(MANIFEST_URL, { credentials: 'same-origin' });
    if (!response.ok) return;
    manifest = await response.json();
    setOffline(false);
  } catch {
    // Offline or signed out: keep the cache as is. Also the connection probe;
    // see [`setOffline`].
    setOffline(true);
    return;
  }

  // Manifest key wins: the page may predate a masquerade start/stop.
  if (manifest.cache_key !== pageKey) await dropForeignCaches(manifest.cache_key);

  const name = CACHE_PREFIX + manifest.cache_key;
  if (!manifest.keep || manifest.keep <= 0) {
    await caches.delete(name);
    return;
  }

  const cache = await caches.open(name);
  const wanted = new Map(manifest.entries.map((e) => [fragmentPath(e.id), e.updated_at]));

  // Evict first so budget checks see the final size, not a peak.
  const held = await cache.keys();
  const heldPaths = new Set();
  for (const request of held) {
    const path = new URL(request.url).pathname;
    if (path === LIBRARY_URL) continue;
    if (wanted.has(path)) {
      heldPaths.add(path);
      continue;
    }
    // Images are reconciled in a later pass against surviving entries.
    if (/^\/entries\/\d+\/fragment$/.test(path)) await cache.delete(request);
  }

  const referenced = new Set();
  let imageBudget = MAX_SYNC_IMAGE_BYTES;
  let room = await hasHeadroom();

  for (const [path, version] of wanted) {
    let html = null;
    const cached = heldPaths.has(path) ? await cache.match(path) : null;
    if (cached && cached.headers.get(VERSION_HEADER) === version) {
      html = await cached.text();
    } else if (room) {
      const id = path.split('/')[2];
      let response;
      try {
        response = await fetch(prefetchUrl(id), { credentials: 'same-origin' });
      } catch {
        continue;
      }
      if (!response.ok) continue;
      html = await response.clone().text();
      await put(cache, path, response, version);
    } else if (cached) {
      // No room to refresh; a stale article beats a missing one.
      html = await cached.text();
    } else {
      continue;
    }

    for (const url of imageUrls(html)) {
      referenced.add(url);
      if (!room || imageBudget <= 0) continue;
      imageBudget -= await cacheImage(cache, url, imageBudget);
    }
    room = room && (await hasHeadroom());
  }

  for (const url of await cacheShellAssets(cache)) referenced.add(url);

  // After the loop, so shared images are dropped only when unused and old-build
  // assets are evicted rather than accumulating per deploy.
  for (const request of await cache.keys()) {
    const url = new URL(request.url);
    const path = url.pathname + url.search;
    if (url.pathname === LIBRARY_URL || wanted.has(url.pathname)) continue;
    if (!referenced.has(path)) await cache.delete(request);
  }

  // Last, so the library page only lists entries that are stored.
  try {
    const response = await fetch(LIBRARY_URL, { credentials: 'same-origin' });
    if (response.ok) await put(cache, LIBRARY_URL, response, manifest.cache_key);
  } catch {
    // Keep the previous copy; it is still accurate.
  }
}

/**
 * Serialise and debounce [`sync`]: concurrent syncs would race each other's
 * evictions, and `rdrs:sidebar-stale` fires on every mark-as-read.
 */
const SYNC_DEBOUNCE_MS = 3000;
let syncing = null;
let syncTimer = 0;

function scheduleSync(delay = 0) {
  clearTimeout(syncTimer);
  syncTimer = setTimeout(() => {
    if (syncing) {
      // Fold into the in-flight run rather than chaining catch-up syncs.
      syncing = syncing.then(() => sync()).catch(() => {});
      return;
    }
    syncing = sync()
      .catch(() => {})
      .finally(() => {
        syncing = null;
      });
  }, delay);
}

/**
 * The reader's own cache, or `null` when off. Checked first because
 * `caches.open` would create an empty one.
 */
async function readerCache() {
  const key = pageCacheKey();
  if (!key) return null;
  const name = CACHE_PREFIX + key;
  return (await caches.has(name)) ? caches.open(name) : null;
}

/**
 * The saved reading pane for `url`, or `null`, exposed on `window` for
 * `performSwap`. Done in the page, not the worker, so the request stays visible
 * to network observers (including tests). Matched on path only, so offline
 * `?view=original` returns the same saved pane.
 */
async function savedFragment(url) {
  const cache = await readerCache();
  if (!cache) return null;
  return (await cache.match(new URL(url, location.origin).pathname)) || null;
}

/** Raise a toast on the page-level `<rdrs-flash>`, if one is mounted. */
function flash(level, message) {
  const host = document.querySelector('rdrs-flash');
  if (host && typeof host.show === 'function') host.show(level, message);
}

/**
 * What still works offline, as a selector — the short, stable half; everything
 * else reaches the server.
 */
const WORKS_OFFLINE = 'a[data-swap="#reading-pane"], a[href="/"], a[href="/entries/offline"]';

/**
 * Server-bound controls. All forms, including GET ones like Load More and search.
 */
const SERVER_BOUND = 'form, a[href], select[data-mark-read-scope], select[data-status-select]';

/** Flag server-bound controls for the CSS and [`blockOffline`]. */
function markServerBound() {
  for (const el of document.querySelectorAll(SERVER_BOUND)) {
    const disable = offlineNow && !el.matches(WORKS_OFFLINE);
    el.toggleAttribute('data-offline-disabled', disable);
    // Not `disabled`: `setFormBusy` in app.js owns that, and two owners get stuck.
    if (disable) {
      el.setAttribute('aria-disabled', 'true');
    } else if (el.getAttribute('aria-disabled') === 'true') {
      el.removeAttribute('aria-disabled');
    }
  }
}

/**
 * Swallow activations that cannot work, and say why. CSS already blocks the
 * mouse; this catches keyboard and programmatic submits. Capture phase so the
 * swap helper and native submit never see it.
 */
function blockOffline(event) {
  if (!offlineNow) return;
  const target = event.target instanceof Element ? event.target : null;
  if (!target?.closest('[data-offline-disabled]')) return;
  event.preventDefault();
  event.stopPropagation();
  flash('warning', 'You are offline — that will have to wait for the connection.');
}

/**
 * Re-mark controls as swaps and the sidebar replace markup. Only observes
 * while offline.
 */
let offlineObserver = null;

function watchForNewControls() {
  if (offlineObserver) return;
  offlineObserver = new MutationObserver(() => markServerBound());
  offlineObserver.observe(document.body, { childList: true, subtree: true });
}

function stopWatching() {
  offlineObserver?.disconnect();
  offlineObserver = null;
}

/**
 * Poll interval while offline; only a successful request proves the
 * connection is back (see [`setOffline`]).
 */
const RECHECK_MS = 30000;

/**
 * Whether the app believes it cannot reach the server. Starts optimistic:
 * `navigator.onLine` is unreliable and a false `false` (common in headless CI)
 * disables the whole app.
 */
let offlineNow = false;
let recheckTimer = 0;

/**
 * Set [`offlineNow`]. Driven by request evidence, not `navigator.onLine`, which
 * stays true behind captive portals and in Chrome's DevTools offline mode.
 * `data-offline` on the root is the whole interface (CSS + sidebar lamp); no
 * toast, which would flash on every blink.
 */
function setOffline(next) {
  if (next === offlineNow) return offlineNow;
  offlineNow = next;
  document.documentElement.toggleAttribute('data-offline', next);
  markServerBound();
  clearTimeout(recheckTimer);
  if (next) {
    watchForNewControls();
    recheckTimer = setTimeout(() => scheduleSync(), RECHECK_MS);
  } else {
    stopWatching();
  }
  return offlineNow;
}

/** Stop the controls that need a server, and say so. */
function installOfflineGuards() {
  // Online/offline events are a fast hint, not proof: going online only
  // schedules a sync, and that request decides.
  window.addEventListener('online', () => scheduleSync());
  window.addEventListener('offline', () => setOffline(true));

  for (const type of ['click', 'submit', 'change']) {
    document.addEventListener(type, blockOffline, true);
  }
}

if ('serviceWorker' in navigator && 'caches' in window) {
  // `performSwap` reports a failed fetch here immediately.
  window.rdrsOffline = { fragment: savedFragment, networkFailed: () => setOffline(true) };
  installOfflineGuards();
  // After paint and after pwa.js registers the worker, or the cache is unreadable.
  window.addEventListener('load', () => {
    scheduleSync();
  });
  // Raised for every mark-as-read and feed refresh.
  document.addEventListener('rdrs:sidebar-stale', () => {
    scheduleSync(SYNC_DEBOUNCE_MS);
  });
}
