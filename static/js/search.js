// static/js/search.js — `/` focuses the search input; /search is pure SSR
// otherwise. Extracted from an inline <script> to survive `script-src 'self'`.
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
