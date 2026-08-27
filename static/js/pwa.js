/**
 * Registers the service worker. Progressive enhancement, like every other
 * module here: with this file blocked the app is exactly what the server sent.
 *
 * Loaded from `app_layout.html`, not `base.html`, so the sign-in and setup pages
 * do not register anything. A reader who never gets past `/login` has no use for
 * an installed app, and registering there would make them pay the worker's
 * install fetches for a page they see once.
 *
 * `/sw.js` is deliberately *not* `?v=`-stamped. The registration is keyed by
 * script URL, so a stamped URL would register a second worker on every deploy
 * instead of updating the one that exists. The build version travels inside the
 * script body instead (substituted server-side), which is what the browser
 * byte-compares to decide an update is due.
 */

if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    // Registration fails on an insecure origin and in private-browsing modes
    // that disable the API. Neither is recoverable and neither costs the reader
    // anything, so the rejection is swallowed rather than surfaced.
    navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch(() => {});
  });
}
