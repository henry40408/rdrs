// Keyboard shortcuts and the shortcut help overlay. Everything that touches the
// swap engine or the reading pane arrives through `installKeyboard`'s `deps`,
// so this module never imports `app.js`.

// Entry ids with an 'm' toggle POST in flight; stops double-press double-POSTs.
const pendingRowToggles = new Set();

let deps;

// Single source of truth for the shortcut help overlay.
const KB_SHORTCUTS = [
    { group: 'Navigation', key: 'j / k', desc: 'Next / previous entry (switches the open entry when the reading pane is open)' },
    { group: 'Navigation', key: 'o / Enter', desc: 'Open selected entry' },
    { group: 'Navigation', key: 'Space / Shift+Space', desc: 'Scroll reading pane down / up' },
    { group: 'Navigation', key: 'Esc', desc: 'Close reading pane' },
    { group: 'Entry actions', key: 'm', desc: 'Toggle read / unread' },
    { group: 'Entry actions', key: 'f', desc: 'Toggle star' },
    { group: 'Entry actions', key: 'v', desc: 'Open original in new tab' },
    { group: 'Entry actions', key: 'd', desc: 'Fetch full content (toggle with original)' },
    { group: 'Entry actions', key: 's', desc: 'Save (Linkding)' },
    { group: 'Entry actions', key: 'a', desc: 'Summarize / dismiss summary (Kagi)' },
    { group: 'Batch read', key: 'A', desc: 'Mark loaded entries as read (asks to confirm)' },
    { group: 'Go to', key: 'g u', desc: 'Unread inbox' },
    { group: 'Go to', key: 'g a', desc: 'All entries' },
    { group: 'Go to', key: 'g r', desc: 'Read' },
    { group: 'Go to', key: 'g s', desc: 'Starred' },
    { group: 'Go to', key: 'g m', desc: 'Summarized' },
    { group: 'Go to', key: 'g f', desc: 'Selected entry’s feed' },
    { group: 'Go to', key: 'g c', desc: 'Selected entry’s category (parent category on a feed page)' },
    { group: 'Go to', key: '[ / ]', desc: 'Previous / next sidebar row (categories + the open category’s feeds)' },
    { group: 'Go to', key: '{ / }', desc: 'Previous / next sidebar row with unread' },
    { group: 'Feed / category pages', key: '1-4', desc: 'Status filter: All / Unread / Read / Starred' },
    { group: 'Other', key: '/', desc: 'Open the search box (searches the current list; on /search, focuses it)' },
    { group: 'Other', key: '?', desc: 'Toggle this help' },
];

// Search-syntax tab of the help overlay; the shape of KB_SHORTCUTS. Keep in step
// with `entry::query`. Shown only on pages with a search box (`[data-search-help]`).
const SEARCH_SYNTAX = [
    { group: 'Status', key: 'is:unread', desc: 'Unread entries' },
    { group: 'Status', key: 'is:read', desc: 'Read entries' },
    { group: 'Status', key: 'is:starred', desc: 'Starred entries' },
    { group: 'Source', key: 'feed:name', desc: 'By feed (fuzzy, case-insensitive)' },
    { group: 'Source', key: 'category:name', desc: 'By category (fuzzy, case-insensitive)' },
    { group: 'Field', key: 'title:word', desc: 'Title contains word' },
    { group: 'Field', key: 'author:name', desc: 'By author' },
    { group: 'Date (UTC)', key: 'before:2026-01-01', desc: 'Published before' },
    { group: 'Date (UTC)', key: 'after:2026-01-01', desc: 'Published after' },
    { group: 'Combine', key: 'AND OR NOT', desc: 'Boolean operators; adjacent words imply AND' },
    { group: 'Combine', key: '( )', desc: 'Group sub-expressions' },
    { group: 'Exclude / quote', key: '-term', desc: 'Exclude a term' },
    { group: 'Exclude / quote', key: '"exact phrase"', desc: 'Quote; also for values with spaces, e.g. feed:"Rust Blog"' },
];

function showHelp(help, tab) {
    help.show(KB_SHORTCUTS, document.querySelector('[data-search-help]') ? { syntax: SEARCH_SYNTAX, tab } : {});
}

// Delegated: list-pane swaps replace the inline search's button.
document.addEventListener('click', (e) => {
    if (!e.target.closest('[data-search-help]')) return;
    const help = document.querySelector('rdrs-kb-help');
    if (!help) return;
    e.preventDefault();
    showHelp(help, 'syntax');
});

// ── "g" go-to sequences ──────────────────────────────────────────────
// `g` arms the namespace and times out; captured so `g s` never triggers Save.
const GO_PAGES = {
    u: '/',
    a: '/entries',
    r: '/entries/read',
    s: '/entries/starred',
    m: '/entries/summarized',
};
const GO_TIMEOUT_MS = 2000;
let goPending = false;
let goTimer = null;

// Which-key hint shown while the `g` namespace is pending.
const GO_HINT_ITEMS = [
    ['u', 'Unread'], ['a', 'All'], ['r', 'Read'], ['s', 'Starred'],
    ['m', 'Summarized'], ['f', 'Feed'], ['c', 'Category'],
];

function showGoHint() {
    if (document.querySelector('.kbd-hint')) return;
    const hint = document.createElement('div');
    hint.className = 'kbd-hint';
    const items = GO_HINT_ITEMS
        .map(([k, label]) => `<span><kbd>${k}</kbd> ${label}</span>`)
        .join('');
    hint.innerHTML = `<span class="kbd-hint-prefix"><kbd>g</kbd> go to…</span>`
        + `<div class="kbd-hint-items">${items}</div>`;
    document.body.appendChild(hint);
}

function hideGoHint() {
    document.querySelector('.kbd-hint')?.remove();
}

function clearGoPending() {
    goPending = false;
    if (goTimer) { clearTimeout(goTimer); goTimer = null; }
    hideGoHint();
}

function goToEntryRelative(key) {
    const row = document.querySelector('[data-entry-row].selected');
    if (key === 'f') {
        const link = row?.querySelector('.entry-item-meta a[href^="/feeds/"]');
        if (link) deps.swapListPane(link.getAttribute('href'), { categoryId: row?.dataset.categoryId });
        return;
    }
    // 'c': the selected entry's category, else the page's parent category.
    const rowCatId = row?.dataset.categoryId;
    if (rowCatId) { deps.swapListPane(`/categories/${rowCatId}/entries`); return; }
    if (!window.location.pathname.startsWith('/feeds/')) return;
    const sb = document.querySelector('rdrs-sidebar');
    const catId = sb && sb.getAttribute('active-category-id');
    if (catId) deps.swapListPane(`/categories/${catId}/entries`);
}

function installGoNavigation() {
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        if (goPending) {
            const key = e.key;
            clearGoPending();
            // Always consumed so a mistyped sequence can't fire a single-key binding.
            e.preventDefault();
            e.stopPropagation();
            const url = GO_PAGES[key];
            if (url) { window.location.href = url; return; }
            if (key === 'f' || key === 'c') goToEntryRelative(key);
            return;
        }
        if (e.key === 'g') {
            e.preventDefault();
            goPending = true;
            showGoHint();
            goTimer = setTimeout(clearGoPending, GO_TIMEOUT_MS);
        }
    }, true);
}

// On `document`, so it works on every logged-in page, not only entries routes.
function installHelpKeyboard() {
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        if (e.key !== '?') return;
        const help = document.querySelector('rdrs-kb-help');
        if (!help) return;
        e.preventDefault();
        if (help.isVisible) help.hide();
        else showHelp(help, 'shortcuts');
    });
}

// Gated on `[data-entries-list]` so other pages don't bind these keys.
function installEntriesKeyboard() {
    if (!document.querySelector('[data-entries-list]')) return;
    // By id: a swap can replace the row node, and an orphan's indexOf is -1.
    let activeId = null;
    // Cached row node for `activeId`, validated on each read.
    let activeNode = null;
    const rows = () => Array.from(document.querySelectorAll('[data-entry-row]'));
    const activeRow = () => {
        if (!activeId) return null;
        if (activeNode?.isConnected && activeNode.getAttribute('data-entry-id') === activeId) {
            return activeNode;
        }
        activeNode = document.querySelector(`[data-entry-row][data-entry-id="${activeId}"]`);
        return activeNode;
    };
    const focusRow = (row) => {
        if (!row) return;
        const prev = activeRow();
        if (prev && prev !== row) prev.classList.remove('selected');
        row.classList.add('selected');
        row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        activeId = row.getAttribute('data-entry-id');
        activeNode = row;
    };
    const move = (delta) => {
        const all = rows();
        if (all.length === 0) return;
        const current = activeRow();
        const idx = current ? all.indexOf(current) : -1;
        const next = Math.max(0, Math.min(all.length - 1, idx + delta));
        focusRow(all[next]);
    };
    // Re-apply `.selected` after each swap, following the open pane (which may
    // be past the loaded rows); Load More re-runs this once the row arrives.
    document.addEventListener('rdrs:swap-complete', () => {
        const paneId = deps.currentPaneEntryId();
        if (paneId != null && paneId !== activeId) {
            // Clear first, or two rows are highlighted.
            activeRow()?.classList.remove('selected');
            activeId = paneId;
        }
        const row = activeRow();
        if (row) row.classList.add('selected');
    });
    // Sync `activeId` on click so `j`/`k` continue from the clicked row.
    document.addEventListener('click', (e) => {
        const link = e.target.closest('[data-entry-row] a[data-swap="#reading-pane"]');
        if (!link) return;
        const row = link.closest('[data-entry-row]');
        if (row) focusRow(row);
    });
    // Null when no entry is loaded or the submit button is disabled.
    const paneForm = (actionSuffix) => {
        const pane = document.getElementById('reading-pane');
        if (!pane || pane.classList.contains('reading-pane-empty')) return null;
        const form = pane.querySelector(`form[action$="${actionSuffix}"]`);
        if (!form) return null;
        const btn = form.querySelector('button[type="submit"], button:not([type])');
        if (btn && btn.disabled) return null;
        return form;
    };
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        switch (e.key) {
            case 'j':
                e.preventDefault();
                // With the pane open, j/k navigate the whole filter.
                if (deps.currentPaneEntryId() != null) deps.navigateNeighbor('next');
                else move(1);
                break;
            case 'k':
                e.preventDefault();
                if (deps.currentPaneEntryId() != null) deps.navigateNeighbor('prev');
                else move(-1);
                break;
            case 'o':
            case 'Enter': {
                const current = activeRow();
                if (!current) return;
                e.preventDefault();
                const link = current.querySelector('a[data-swap]');
                if (link) link.click();
                break;
            }
            case 'f': {
                // The row form's action is state-dependent, so match either.
                const current = activeRow();
                if (!current) return;
                const form = current.querySelector('form[action$="/star"], form[action$="/unstar"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case 'm': {
                // No read form in the row; drive the swap directly. The in-flight
                // guard prevents double-POSTs.
                const current = activeRow();
                if (!current) return;
                const id = current.getAttribute('data-entry-id');
                if (!id) return;
                e.preventDefault();
                if (pendingRowToggles.has(id)) return;
                const isRead = current.classList.contains('entry-read');
                const url = `/entries/${id}/${isRead ? 'unread' : 'read'}`;
                pendingRowToggles.add(id);
                deps.performSwap(url, { method: 'POST' }, `#entry-row-${id}`)
                    .finally(() => pendingRowToggles.delete(id));
                break;
            }
            case '1':
            case '2':
            case '3':
            case '4': {
                // `[data-status-filter]` options: All / Unread / Read / Starred.
                const options = document.querySelectorAll('[data-status-filter] option');
                if (options.length === 0) return;
                const idx = parseInt(e.key, 10) - 1;
                if (idx < 0 || idx >= options.length) return;
                e.preventDefault();
                window.location.href = options[idx].value;
                break;
            }
            case 'A': {
                // Works under scoped search too, where the button is hidden.
                const btn = document.getElementById('mark-above-read');
                const searching = !!document.querySelector('[data-entries-search] input[name="q"]')?.value;
                if (!btn && !searching) return;
                e.preventDefault();
                deps.markLoadedEntriesAsRead(btn);
                break;
            }
            case 'v': {
                // Absent when the entry has no link.
                const current = activeRow();
                const url = current?.getAttribute('data-entry-link');
                if (!url) return;
                e.preventDefault();
                // A features string would drop noreferrer and force a popup.
                const a = document.createElement('a');
                a.href = url;
                a.target = '_blank';
                a.rel = 'noopener noreferrer';
                a.click();
                break;
            }
            case 'd': {
                // After fetching, the button becomes a "Show Original" link.
                const form = paneForm('/fetch-full-content');
                if (form) { e.preventDefault(); form.requestSubmit(); break; }
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                const showOriginal = pane.querySelector('a[data-swap="#reading-pane"]');
                if (showOriginal) { e.preventDefault(); showOriginal.click(); }
                break;
            }
            case 's': {
                // Rendered only when a save target is configured.
                const form = paneForm('/save');
                if (!form) return;
                e.preventDefault();
                form.requestSubmit();
                break;
            }
            case 'a': {
                // Same toggle the action-bar Summarize button performs.
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                // Ignored while in flight; Cancel lives in the summary box.
                if (deps.summaryInFlight()) { e.preventDefault(); break; }
                if (deps.dismissVisibleSummary()) { e.preventDefault(); break; }
                const form = paneForm('/summarize');
                if (!form) return;
                e.preventDefault();
                form.requestSubmit();
                break;
            }
            case '[':
            case ']':
            case '{':
            case '}': {
                // Start from the current feed/category, else the list's first
                // (or last) row; wraps within this list.
                const targets = deps.sidebarNavTargets();
                if (targets.length === 0) return;
                const path = window.location.pathname;
                const feedPage = path.match(/^\/feeds\/(\d+)\/entries/);
                const catPage = path.match(/^\/categories\/(\d+)\/entries/);
                const isCurrent = (t) => (feedPage
                    ? t.kind === 'feed' && t.id === parseInt(feedPage[1], 10)
                    : catPage
                        ? t.kind === 'category' && t.id === parseInt(catPage[1], 10)
                        : false);
                const len = targets.length;
                const forward = e.key === ']' || e.key === '}';
                const step = forward ? 1 : -1;
                const unreadOnly = e.key === '{' || e.key === '}';
                // Virtual start so the first probe lands on the first/last target.
                let idx = targets.findIndex(isCurrent);
                if (idx === -1) idx = forward ? -1 : len;
                let target = null;
                for (let i = 1; i <= len; i++) {
                    const probe = targets[((idx + i * step) % len + len) % len];
                    if (isCurrent(probe)) continue;
                    if (unreadOnly && probe.unread <= 0) continue;
                    target = probe;
                    break;
                }
                if (!target) return;
                e.preventDefault();
                deps.swapListPane(target.href, { categoryId: target.categoryId });
                break;
            }
            case 'Escape': {
                // The help overlay owns Esc while open, in its own shadow root.
                const help = document.querySelector('rdrs-kb-help');
                if (help && help.isVisible) return;
                if (deps.closeReadingPane()) e.preventDefault();
                break;
            }
            case ' ': {
                // No fallback action when the pane is empty.
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                e.preventDefault();
                const dir = e.shiftKey ? -1 : 1;
                pane.scrollBy({ top: dir * pane.clientHeight * 0.85, behavior: 'smooth' });
                break;
            }
        }
    });
}

export function installKeyboard(injected) {
    deps = injected;
    installGoNavigation();
    installHelpKeyboard();
    installEntriesKeyboard();
}
