/**
 * Offline reading: mirror the reader's queue into the service worker's cache.
 *
 * The articles stored here are the *server's own* reading-pane markup, fetched
 * through the ordinary `GET /entries/{id}/fragment` route. Nothing is rendered
 * twice: offline, `app.js` performs the same swap it always does and the worker
 * answers the same request from cache. That is the whole design — this module
 * decides *what* to hold, never *how* it looks.
 *
 * ## Why this is opt-in
 *
 * Every signed-in response is `no-store`, and until this feature nothing
 * belonging to a reader was written to disk by the browser at all. Offline
 * reading trades that away, so it is off by default, bounded by a number the
 * reader chooses (`offline_keep`), and namespaced by an opaque per-user key so
 * one account's articles cannot outlive a sign-out into the next account's
 * session on a shared device.
 *
 * ## Why the cache is its own ledger
 *
 * There is no separate index of what is held: the cache keys *are* the list of
 * entries, and each stored response carries its `entry.updated_at` in an
 * `x-rdrs-offline-version` header. A ledger in `localStorage` would be a second
 * copy of the same truth, and the two would disagree the first time a write
 * failed halfway.
 */

const CACHE_PREFIX = 'rdrs-offline-';
const MANIFEST_URL = '/api/offline/manifest';
const LIBRARY_URL = '/entries/offline';

/** Where a stored entry's `updated_at` lives. See the module note. */
const VERSION_HEADER = 'x-rdrs-offline-version';

/**
 * Per-image and per-sync ceilings on what an article's pictures may cost.
 *
 * An entry with fifty full-bleed photographs is not worth a reader's disk, and
 * the budget is spent newest-entry-first, so the cap degrades by dropping the
 * images of the oldest articles rather than by failing the sync.
 */
const MAX_IMAGE_BYTES = 2 * 1024 * 1024;
const MAX_SYNC_IMAGE_BYTES = 48 * 1024 * 1024;

/**
 * Fraction of the origin's storage quota above which the sync stops writing.
 * Browsers evict the *whole* origin when it runs out, which would take the
 * session's `sessionStorage` sidebar mirror with it, so the budget stays well
 * clear of the ceiling rather than discovering it.
 */
const QUOTA_HEADROOM = 0.8;

/** The canonical cache key for an entry — the URL `app.js` actually requests. */
function fragmentPath(id) {
  return `/entries/${id}/fragment`;
}

/**
 * The URL the *sync* fetches. `offline=1` is what stops mirroring the queue
 * from marking every entry in it read: opening an entry marks it read, and a
 * sync opens all of them. Stored under [`fragmentPath`] regardless, so the
 * reader's own click matches it.
 */
function prefetchUrl(id) {
  return `${fragmentPath(id)}?offline=1`;
}

/** Reader's cache name and budget, as the server rendered them into the page. */
function pageConfig() {
  const root = document.documentElement;
  const key = root.dataset.offlineKey || '';
  const keep = Number.parseInt(root.dataset.offlineKeep || '0', 10);
  return { key, keep: Number.isFinite(keep) ? keep : 0 };
}

/**
 * Drop every offline cache that is not `key`'s.
 *
 * Runs before the first network call of every page load, which is the point:
 * signing in as someone else is only possible online, so this is the first
 * moment after a switch at which the previous account's articles can be
 * removed, and it does not wait for a round trip to do it.
 */
async function dropForeignCaches(key) {
  const mine = key ? CACHE_PREFIX + key : null;
  const names = await caches.keys();
  await Promise.all(
    names.filter((name) => name.startsWith(CACHE_PREFIX) && name !== mine).map((name) => caches.delete(name)),
  );
}

/**
 * Store `response` under `url`, rebuilt from its body rather than put as it
 * arrived.
 *
 * Three headers make a signed-in response unstorable as-is. `Vary: Cookie` is
 * honoured by `cache.match` while the worker's own `Request` carries no cookie
 * header at all, so a stored copy may never match again. `Set-Cookie` would put
 * a session cookie in a cache the worker replays from. `Cache-Control: no-store`
 * is ignored by the Cache API but keeping it around invites the next reader of
 * this code to believe it did something. Rebuilding drops all three by
 * construction, which no denylist of header names could promise.
 */
async function put(cache, url, response, version) {
  const body = await response.blob();
  const headers = new Headers();
  const type = response.headers.get('content-type');
  if (type) headers.set('Content-Type', type);
  if (version) headers.set(VERSION_HEADER, version);
  await cache.put(url, new Response(body, { status: 200, statusText: 'OK', headers }));
}

/** Whether the origin still has room to spend. See [`QUOTA_HEADROOM`]. */
async function hasHeadroom() {
  if (!navigator.storage?.estimate) return true;
  try {
    const { usage = 0, quota = 0 } = await navigator.storage.estimate();
    return quota === 0 || usage / quota < QUOTA_HEADROOM;
  } catch {
    return true;
  }
}

/**
 * Same-origin image URLs an entry's markup references: proxied article images
 * and the feed's favicon. Both are signed or id-addressed and cookie-free, and
 * an article whose pictures are all broken frames is not the offline reading
 * anyone asked for.
 */
function imageUrls(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const urls = new Set();
  for (const img of doc.querySelectorAll('img[src]')) {
    try {
      const url = new URL(img.getAttribute('src'), location.origin);
      if (url.origin === location.origin) urls.add(url.pathname + url.search);
    } catch {
      // A `src` that will not parse cannot be fetched either; the browser will
      // render it as broken whether we look at it here or not.
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
 * Extensions the static handler actually serves.
 *
 * [`referencesIn`] reads *source text*, so it cannot tell a real import from a
 * mention of one in a comment — this module's own prose about `url(…)` being
 * the first casualty, which turned into a 404 for `/static/js/...` on every
 * sync. Demanding a real asset extension is what keeps an example in a comment
 * from becoming a request.
 */
const ASSET_EXTENSION = /\.(?:js|css|woff2?|png|svg|ico|webmanifest)$/i;

/**
 * Same-origin `/static/` URLs referenced from inside a fetched asset: a
 * stylesheet's `url(…)` targets, a module's import specifiers. Resolved against
 * `from`, because a module's specifier may be relative to itself
 * (`'./utils.js'`).
 *
 * Each pattern is applied only to the kind of file it belongs to. Running the
 * stylesheet one over JavaScript matched the tail of `bufferToBase64url(buffer)`
 * and sent the sync looking for `/static/js/buffer` — a reminder that these are
 * substring matches over source text, not parses of it. [`ASSET_EXTENSION`] is
 * the backstop for whatever the next such coincidence turns out to be.
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
      // A `data:` URL, or something that is not a URL at all. Neither is ours.
    }
  }
  return found;
}

/**
 * Store the `/static/` assets a saved page needs to render and to be usable,
 * and report which ones those are.
 *
 * Saving the markup is not enough on its own: the library page is ordinary
 * server-rendered HTML that needs `app.css` to look like the app and `app.js`
 * to open an entry at all — without the latter a click is a real navigation to
 * a fragment URL, which offline resolves to nothing. They are version-stamped
 * and public, and the worker only falls back to these copies once the network
 * has failed, so a stale one can never reach an online reader.
 *
 * The walk is transitive and starts from the live document rather than from a
 * list kept here, so neither a module added to `app_layout.html` nor one
 * `import`ed by a module already in it needs a second list to stay in step.
 * That matters twice over: fonts are named only inside the stylesheet, and
 * `app.js` pulls in `utils.js` through an import the document never mentions.
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
      // Not a URL this browser will fetch either.
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
        // Nothing to do but try again next sync.
        continue;
      }
    }

    // Only stylesheets and modules can name anything. Reading a woff2 as text
    // would decode a hundred kilobytes into mojibake to find no references in
    // it, once per font, on every sync.
    const type = stored?.headers.get('content-type') || '';
    if (!/javascript|css/.test(type)) continue;
    const references = referencesIn(await stored.text(), url, type.includes('css'));
    for (const found of references) pending.push(found);
  }
  return seen;
}

/**
 * Bring the cache in line with the manifest: fetch what is missing or stale,
 * drop what has left the set, and re-store the library page that lists it.
 */
async function sync() {
  if (!('caches' in window) || !navigator.onLine) return;

  const page = pageConfig();
  await dropForeignCaches(page.key);
  if (!page.key) return;

  let manifest;
  try {
    const response = await fetch(MANIFEST_URL, { credentials: 'same-origin' });
    if (!response.ok) return;
    manifest = await response.json();
  } catch {
    // Offline, or the session ended under us. Either way the cache we already
    // hold is the reader's own and stays exactly as it is.
    return;
  }

  // The manifest's key wins over the page's: the document may have been
  // rendered before a masquerade started or ended, and this one was minted for
  // the session that just answered.
  if (manifest.cache_key !== page.key) await dropForeignCaches(manifest.cache_key);

  const name = CACHE_PREFIX + manifest.cache_key;
  if (!manifest.keep || manifest.keep <= 0) {
    await caches.delete(name);
    return;
  }

  const cache = await caches.open(name);
  const wanted = new Map(manifest.entries.map((e) => [fragmentPath(e.id), e.updated_at]));

  // Evict first, so the budget checks below are made against what the cache
  // will actually hold rather than against a peak it passes through.
  const held = await cache.keys();
  const heldPaths = new Set();
  for (const request of held) {
    const path = new URL(request.url).pathname;
    if (path === LIBRARY_URL) continue;
    if (wanted.has(path)) {
      heldPaths.add(path);
      continue;
    }
    // Images are reconciled below against the entries that survive, so
    // anything not in `wanted` and not an entry fragment is left for that pass.
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
      // No room to refresh it, but a stale article still reads better than a
      // missing one, so it keeps its place and its images.
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

  // Images and assets whose entry — or whose build — has left the set. Done
  // after the loop so a picture shared by two entries is only dropped once
  // neither of them wants it, and so a `?v=` bump evicts the previous build's
  // scripts rather than accumulating one copy per deploy.
  for (const request of await cache.keys()) {
    const url = new URL(request.url);
    const path = url.pathname + url.search;
    if (url.pathname === LIBRARY_URL || wanted.has(url.pathname)) continue;
    if (!referenced.has(path)) await cache.delete(request);
  }

  // Last, so the page listing the entries is only stored once they are all
  // actually there.
  try {
    const response = await fetch(LIBRARY_URL, { credentials: 'same-origin' });
    if (response.ok) await put(cache, LIBRARY_URL, response, manifest.cache_key);
  } catch {
    // Nothing to do: the previous copy, if any, is still a truthful list of
    // what the cache holds.
  }
}

/**
 * Serialise and rate-limit [`sync`].
 *
 * `rdrs:sidebar-stale` fires on every mark-as-read, and two syncs running at
 * once would race each other's evictions — one deciding an entry has left the
 * set while the other is still writing it. The trailing delay also lets a burst
 * of triage settle into a single pass over the queue.
 */
const SYNC_DEBOUNCE_MS = 3000;
let syncing = null;
let syncTimer = 0;

function scheduleSync(delay = 0) {
  clearTimeout(syncTimer);
  syncTimer = setTimeout(() => {
    if (syncing) {
      // Fold into the run in flight rather than queueing behind it: its
      // manifest fetch has not happened yet often enough to matter, and a chain
      // of catch-up syncs is how a busy triage session ends up refetching the
      // same queue five times.
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
 * The reader's own cache, or `null` when offline reading is off.
 *
 * `caches.open` would *create* the cache, so its absence is checked first —
 * an account with the feature off must not end up owning an empty cache named
 * after them.
 */
async function readerCache() {
  const { key } = pageConfig();
  if (!key) return null;
  const name = CACHE_PREFIX + key;
  return (await caches.has(name)) ? caches.open(name) : null;
}

/**
 * The saved reading pane for `url`, or `null`.
 *
 * Published on `window` for `performSwap`, which owns the fetch that fails and
 * so is the only place that can substitute this for it — the same arrangement
 * as `window.flash`. Doing it here rather than in the service worker keeps the
 * request page-originated: a worker that re-issued it would make the fetch
 * invisible to everything watching the page's network, the test harness
 * included.
 *
 * Matched on the path alone, because the stored key is the canonical
 * `/entries/{id}/fragment` — the URL the reader's own click produces. The one
 * casualty is `?view=original`, which offline hands back the same saved pane
 * instead of what the feed published; storing both views of every article to
 * fix that would double the library for a toggle.
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
 * Stop the actions that need a server, and say so.
 *
 * `navigator.onLine` is a hint, not a fact — it is true on a captive portal
 * that answers nothing. That is tolerable here because this is presentation
 * only: a POST that slips through fails exactly as it did before, and
 * `performSwap` already falls back to a real navigation.
 */
function installOfflineGuards(keep) {
  const paint = () => {
    document.documentElement.toggleAttribute('data-offline', !navigator.onLine);
  };

  window.addEventListener('online', () => {
    paint();
    scheduleSync();
  });
  window.addEventListener('offline', () => {
    paint();
    if (keep > 0) flash('info', 'You are offline. Saved entries are still readable.');
  });
  paint();

  document.addEventListener(
    'submit',
    (event) => {
      if (navigator.onLine) return;
      const form = event.target;
      if (!(form instanceof HTMLFormElement)) return;
      if ((form.getAttribute('method') || 'get').toUpperCase() !== 'POST') return;
      event.preventDefault();
      // Capture phase, so neither the swap helper nor a native submit gets the
      // event. Same-node listeners still run, which is what leaves `csrf.js`
      // free to do its (now pointless, and harmless) token rewrite.
      event.stopPropagation();
      flash('warning', 'You are offline — that will have to wait for the connection.');
    },
    true,
  );
}

if ('serviceWorker' in navigator && 'caches' in window) {
  const { keep } = pageConfig();
  window.rdrsOffline = { fragment: savedFragment };
  installOfflineGuards(keep);
  // After paint, and after `pwa.js` has had its chance to register the worker:
  // a sync that beats the registration writes a cache nothing is yet able to
  // read from.
  window.addEventListener('load', () => {
    scheduleSync();
  });
  // A mark-as-read or a feed refresh changes what belongs in the set, and this
  // signal is already raised for every one of them.
  document.addEventListener('rdrs:sidebar-stale', () => {
    scheduleSync(SYNC_DEBOUNCE_MS);
  });
}
