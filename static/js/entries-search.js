// Scoped-search drawer and live search on entry-list pages.

// `?v=` is substituted at serve time; unversioned, this import would go stale
// forever under the `immutable` cache header.
import { debounce } from './utils.js?v=__RDRS_ASSET_VERSION__';

// ── Scoped-search drawer ─────────────────────────────────────────────
// Server renders it open for `?q=`. Closing also clears the search, so a hidden
// filter never lingers.
function searchDrawerParts() {
    const drawer = document.querySelector('[data-search-drawer]');
    return {
        drawer,
        toggle: document.querySelector('[data-search-toggle]'),
        input: drawer?.querySelector('input[name="q"]'),
        form: drawer?.querySelector('form[data-entries-search]'),
    };
}

function openSearchDrawer() {
    const { drawer, toggle, input } = searchDrawerParts();
    if (!drawer) return;
    drawer.classList.add('is-open');
    toggle?.setAttribute('aria-expanded', 'true');
    input?.focus();
}

function closeSearchDrawer() {
    const { drawer, toggle, input, form } = searchDrawerParts();
    if (!drawer) return;
    drawer.classList.remove('is-open');
    toggle?.setAttribute('aria-expanded', 'false');
    // Only if needed; a swap would drop scroll position.
    if (input && input.value !== '') {
        input.value = '';
        form?.requestSubmit();
    }
    // Don't strand focus inside a collapsed container.
    toggle?.focus();
}

// Delegated (installed once), since list-pane swaps replace these controls.
function installSearchDrawer() {
    document.addEventListener('click', (e) => {
        if (e.target.closest('[data-search-toggle]')) {
            e.preventDefault();
            const open = document.querySelector('[data-search-drawer]')?.classList.contains('is-open');
            if (open) closeSearchDrawer(); else openSearchDrawer();
        } else if (e.target.closest('[data-search-close]')) {
            e.preventDefault();
            closeSearchDrawer();
        }
    });
    document.addEventListener('keydown', (e) => {
        // The same key /search binds (static/js/search.js).
        if (e.key === '/' && !e.metaKey && !e.ctrlKey && !e.altKey &&
            !e.target.matches('input, textarea, select')) {
            if (!document.querySelector('[data-search-drawer]')) return;
            e.preventDefault();
            openSearchDrawer();
            return;
        }
        // Esc inside the box closes the drawer rather than the reading pane.
        if (e.key === 'Escape' && e.target.closest('[data-search-drawer]')) {
            e.stopPropagation();
            closeSearchDrawer();
        }
    }, true);
}

// The error banner is swapped, the input is not: mirror the banner onto the input.
function syncSearchInvalid() {
    const input = document.querySelector('[data-search-drawer] input[name="q"]');
    const error = document.querySelector('[data-search-error]');
    if (input && error) input.setAttribute('aria-invalid', String(!error.hidden));
}

// Search as you type for `form[data-live-search="<debounce ms>"]`. The form sits
// outside the swapped results, so it keeps focus while typing; `installSwap()`
// performs the actual swap. `data-live-search-min` holds back a too-short ASCII
// query (one CJK character is already a word); clearing always submits. While an
// IME is composing (zhuyin, pinyin) nothing is submitted; `compositionend` does it.
function installLiveSearch() {
    for (const form of document.querySelectorAll('form[data-live-search]')) {
        if (form.dataset.searchBound) continue;
        form.dataset.searchBound = '1';
        const input = form.querySelector('input[name="q"]');
        if (!input) continue;
        const min = Number(form.dataset.liveSearchMin) || 0;
        const submit = debounce(() => {
            const q = input.value.trim();
            if (q && q.length < min && /^[\x00-\x7f]*$/.test(q)) return;
            form.requestSubmit();
        }, Number(form.dataset.liveSearch) || 250);
        input.addEventListener('input', (e) => { if (!e.isComposing) submit(); });
        input.addEventListener('compositionstart', submit.cancel);
        input.addEventListener('compositionend', submit);
    }
}

export function installEntriesSearch() {
    installSearchDrawer();
    syncSearchInvalid();
    document.addEventListener('rdrs:swap-complete', syncSearchInvalid);
    installLiveSearch();
    document.addEventListener('rdrs:swap-complete', installLiveSearch);
}
