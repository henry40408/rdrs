// static/js/search.js — `/` focuses the search input; /search is pure SSR
// otherwise. Extracted from an inline <script> to survive `script-src 'self'`.
// Results are swapped, the input is not: mirror the error banner onto it.
function syncInvalid() {
    const input = document.querySelector('[data-testid="search-input"]');
    if (input) input.setAttribute('aria-invalid', String(!!document.querySelector('[data-testid="search-error"]')));
}
syncInvalid();
document.addEventListener('rdrs:swap-complete', syncInvalid);

document.addEventListener('keydown', (e) => {
    if (e.key !== '/') return;
    const t = document.activeElement;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
    const input = document.querySelector('[data-testid="search-input"]');
    if (input) {
        e.preventDefault();
        input.focus();
    }
});
