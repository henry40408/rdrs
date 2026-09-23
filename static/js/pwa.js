/**
 * Registers the service worker. Loaded from app_layout.html so /login and /setup
 * don't register one. `/sw.js` is not `?v=`-stamped: registrations are keyed by
 * URL, so stamping would add a worker per deploy instead of updating it.
 */

if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    // Fails on insecure origins / private browsing; unrecoverable and harmless.
    navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch(() => {});
  });
}
