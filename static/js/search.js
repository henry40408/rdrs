// static/js/search.js — the one interaction on /search.
//
// Extracted from an inline <script> in search.html so the page survives a
// strict `script-src 'self'` (see middleware::security_headers).

// `/` focuses the search input (matches the legacy entries.js mode='search'
// shortcut). The page is pure SSR otherwise.
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
