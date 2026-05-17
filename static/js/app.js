// static/js/app.js — shared module for the logged-in surface.
//
// Ships: swap() partial-swap helper, sidebar polling, flash dismiss,
// theme controller, entries-family keyboard shortcuts, Mark-as-Read
// dropdown, Mark Above as Read, row-click-to-open delegation.

/**
 * Intercept form / link interactions tagged with `data-swap="<selector>"`
 * and replace the matching element with HTML returned by the request.
 *
 * Response format:
 *   - HTML fragment: replaces the target element via outerHTML.
 *   - Multi-target: response containing one or more
 *     `<template data-swap-target="<selector>">…</template>` blocks.
 *     Each template's content replaces its target via outerHTML.
 *
 * On a non-2xx response the helper falls back to native form submit /
 * link navigation so the user always sees a real page.
 */
function installSwap() {
    document.addEventListener('click', async (event) => {
        const anchor = event.target.closest('a[data-swap]');
        if (!anchor) return;
        if (event.button !== 0 || event.metaKey || event.ctrlKey ||
            event.shiftKey || event.altKey) return;
        const target = anchor.getAttribute('data-swap');
        event.preventDefault();
        await performSwap(anchor.href, { method: 'GET' }, target);
    });

    document.addEventListener('submit', async (event) => {
        const form = event.target.closest('form[data-swap]');
        if (!form) return;
        const target = form.getAttribute('data-swap');
        event.preventDefault();
        const method = (form.method || 'GET').toUpperCase();
        const init = { method };
        let url = form.action;
        if (method === 'GET') {
            // GET requests carry form data in the query string, not the
            // body. Without this, hidden inputs like `after=…` on the
            // Load-More form silently drop and the server falls through
            // to the full-page render.
            const params = new URLSearchParams(new FormData(form));
            const sep = url.includes('?') ? '&' : '?';
            url = url + sep + params.toString();
        } else {
            init.body = new FormData(form);
        }
        setFormBusy(form);
        try {
            await performSwap(url, init, target);
        } finally {
            // On success the form has been replaced by the swap so the
            // call below is a no-op on the detached node. On failure
            // (POST error → flash) the original form is still mounted
            // and gets its button restored.
            clearFormBusy(form);
        }
    });
}

// Map slow form-swap actions to their busy-state button label. Anything not
// listed keeps its existing label and just gets `disabled` while in-flight.
const BUSY_LABELS = {
    save: 'Saving…',
    'fetch-full-content': 'Fetching…',
    summarize: 'Summarizing…',
};

function deriveBusyLabel(actionUrl) {
    const m = (actionUrl || '').match(/\/entries\/\d+\/([\w-]+)/);
    return m ? BUSY_LABELS[m[1]] : null;
}

function setFormBusy(form) {
    form.setAttribute('aria-busy', 'true');
    const btn = form.querySelector('button[type="submit"], button:not([type])');
    if (!btn) return;
    btn.disabled = true;
    const label = deriveBusyLabel(form.action);
    if (label) {
        btn.dataset.busyOriginalLabel = btn.textContent;
        btn.textContent = label;
    }
}

function clearFormBusy(form) {
    form.removeAttribute('aria-busy');
    const btn = form.querySelector('button[type="submit"], button:not([type])');
    if (!btn) return;
    btn.disabled = false;
    if (btn.dataset.busyOriginalLabel != null) {
        btn.textContent = btn.dataset.busyOriginalLabel;
        delete btn.dataset.busyOriginalLabel;
    }
}

async function performSwap(url, init, defaultTarget) {
    const method = (init.method || 'GET').toUpperCase();
    let response;
    try {
        response = await fetch(url, init);
    } catch {
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return;
        }
        window.location.href = url;
        return;
    }
    if (!response.ok) {
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return;
        }
        window.location.href = url;
        return;
    }
    const text = await response.text();
    const parsed = new DOMParser().parseFromString(text, 'text/html');

    const templates = parsed.querySelectorAll('template[data-swap-target]');
    if (templates.length > 0) {
        for (const tpl of templates) {
            const sel = tpl.getAttribute('data-swap-target');
            const dst = document.querySelector(sel);
            if (!dst) continue;
            const parent = dst.parentNode;
            // Insert every child of the template content (including
            // multi-element payloads — e.g. Load-More returns N rows + a
            // new load-more form) before the swap target, then remove
            // the target. Single-element templates collapse to the same
            // outcome as the previous outerHTML-replace behaviour, so
            // existing call sites stay correct.
            const nodes = Array.from(tpl.content.childNodes);
            for (const node of nodes) {
                parent.insertBefore(node, dst);
            }
            parent.removeChild(dst);
        }
        applyFlashTemplates(parsed);
        document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
        return;
    }

    const dst = document.querySelector(defaultTarget);
    if (!dst) return;
    const incoming = parsed.body.firstElementChild;
    if (!incoming) return;
    dst.outerHTML = incoming.outerHTML;
    applyFlashTemplates(parsed);
    document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
}

// Process `<template data-flash data-level="success|error|info|warning">message</template>`
// blocks in a swap response. Each one becomes a toast on the page-level
// `<rdrs-flash>` element (mounted by _entries_layout.html). Used for
// post-action feedback that doesn't have a corresponding DOM state
// change — e.g. Save / Fetch Full Content.
function applyFlashTemplates(parsed) {
    const flashes = parsed.querySelectorAll('template[data-flash]');
    for (const tpl of flashes) {
        const level = tpl.getAttribute('data-level') || 'info';
        // `<template>` contents live in `.content` DocumentFragment, not in
        // direct children — `tpl.textContent` returns '' here. Read from
        // `tpl.content.textContent` instead.
        const message = (tpl.content?.textContent || '').trim();
        if (!message) continue;
        if (window.flash && typeof window.flash.show === 'function') {
            window.flash.show(level, message);
        }
    }
}

// Full-reload stub. The SPA router's `window.rdrsNavigate(path)` API
// is preserved here as a thin wrapper around `location.href = path`,
// letting existing CSR keyboard / dropdown / page-module code keep
// working after router.js is removed. Per-page PRs delete each call
// site as they migrate to SSR.
window.rdrsNavigate = function(path) {
    window.location.href = path;
};

// Sidebar mobile-toggle helpers. <rdrs-sidebar>'s render emits
// inline `onclick="toggleSidebar()"` / `onclick="closeSidebar()"`,
// which require global functions — assign to `window` because
// module-scope declarations are not visible to inline event
// attributes.
window.toggleSidebar = function() {
    const sidebar = document.getElementById('sidebar');
    const toggle = document.querySelector('.sidebar-toggle');
    if (sidebar) {
        sidebar.classList.toggle('open');
        if (toggle) toggle.style.display = sidebar.classList.contains('open') ? 'none' : '';
    }
};

window.closeSidebar = function() {
    const sidebar = document.getElementById('sidebar');
    const toggle = document.querySelector('.sidebar-toggle');
    if (sidebar) sidebar.classList.remove('open');
    if (toggle) toggle.style.display = '';
};

installSwap();

// Sidebar unread polling — fires every 20s on pages that mount the
// SSR sidebar-unread block (the 5 entries-family routes in PR-10).
// The payload is JSON in the `data-payload` attribute; we dispatch a
// custom event so `<rdrs-sidebar>` can apply the new counts.
//
// NOTE: as of PR-10, `<rdrs-sidebar>` reads from `#rdrs-sidebar-bootstrap`
// (a <script> tag) and has no listener for `rdrs:sidebar-unread`. The
// dispatch is a forward-compatible hook — PR-12 will wire up the listener
// so the sidebar live-updates counts without a page reload. For now the
// live-apply of the sidebar UI is deferred; the data is still refreshed
// in the DOM `data-payload` attribute so it is available to future code.
function installSidebarPolling() {
    const host = document.getElementById('sidebar-unread');
    if (!host) return;
    const tick = async () => {
        try {
            const resp = await fetch('/sidebar/unread', { credentials: 'same-origin' });
            if (!resp.ok) return;
            const html = await resp.text();
            const doc = new DOMParser().parseFromString(html, 'text/html');
            const node = doc.getElementById('sidebar-unread');
            if (!node) return;
            const payload = node.getAttribute('data-payload') || '[]';
            const target = document.getElementById('sidebar-unread');
            if (target) target.setAttribute('data-payload', payload);
            document.dispatchEvent(new CustomEvent('rdrs:sidebar-unread', {
                detail: JSON.parse(payload),
            }));
        } catch {}
    };
    setInterval(tick, 20000);
}
installSidebarPolling();

// After every successful partial swap (mark-read, mark-unread, mark-all,
// batch read, OPML import surface, etc.), ask <rdrs-sidebar> to refetch.
// `rdrs:swap-complete` fires from performSwap() on both single- and
// multi-target swaps. Refreshing on every swap over-fetches slightly —
// star/save/fetch-full-content don't change sidebar state — but the
// server-side per-user sidebar cache means each /api/sidebar call costs
// roughly nothing on a hit, so a single broad hook beats a fragile
// per-action allowlist.
document.addEventListener('rdrs:swap-complete', () => {
    document.querySelector('rdrs-sidebar')?.refresh();
});

// Single source of truth for the in-app shortcut help. Pages don't
// register additional entries — every shortcut the keyboard handler
// recognizes is listed here, grouped by where it applies.
const KB_SHORTCUTS = [
    { group: 'Entry list', key: 'j', desc: 'Next entry' },
    { group: 'Entry list', key: 'k', desc: 'Previous entry' },
    { group: 'Entry list', key: 'Enter', desc: 'Open selected entry' },
    { group: 'Entry list', key: 's', desc: 'Toggle star' },
    { group: 'Entry list', key: 'u / r', desc: 'Toggle read / unread' },
    { group: 'Entry list', key: 'Space', desc: 'Scroll reading pane (toggle read when pane is empty)' },
    { group: 'Entry list', key: 'Shift+Space', desc: 'Scroll reading pane up (when pane is open)' },
    { group: 'Entry actions', key: 'b', desc: 'Open original in new tab' },
    { group: 'Entry actions', key: 'Shift+B', desc: 'Save (Linkding)' },
    { group: 'Entry actions', key: 'Shift+F', desc: 'Fetch full content (toggle with original)' },
    { group: 'Entry actions', key: 'Shift+M', desc: 'Summarize (Kagi, when pane is open)' },
    { group: 'Batch read', key: 'o', desc: 'Mark loaded rows as read' },
    { group: 'Batch read', key: 'Shift+K', desc: 'Mark all as read (incl. unloaded)' },
    { group: 'List filters', key: '1', desc: 'All' },
    { group: 'List filters', key: '2', desc: 'Unread' },
    { group: 'List filters', key: '3', desc: 'Read' },
    { group: 'List filters', key: '4', desc: 'Starred' },
    { group: 'Navigation', key: 'f', desc: 'Go to selected entry’s feed' },
    { group: 'Navigation', key: 'c', desc: 'Go to selected entry’s category (parent category as fallback)' },
    { group: 'Navigation', key: 'x', desc: 'Go to Unread (on category page)' },
    { group: 'Navigation', key: '[', desc: 'Previous category (on category page)' },
    { group: 'Navigation', key: ']', desc: 'Next category (on category page)' },
    { group: 'Navigation', key: 'Shift+[', desc: 'Previous category with unread' },
    { group: 'Navigation', key: 'Shift+]', desc: 'Next category with unread' },
    { group: 'Help', key: 'Esc', desc: 'Close reading pane (when pane is open)' },
    { group: 'Help', key: '?', desc: 'Toggle this help' },
];

// `?` toggles the shortcut help overlay. Bound on `document` so it
// works on every logged-in page, not only the entries-family routes.
function installHelpKeyboard() {
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        if (e.key !== '?') return;
        const help = document.querySelector('rdrs-kb-help');
        if (!help) return;
        e.preventDefault();
        if (help.isVisible) help.hide();
        else help.show(KB_SHORTCUTS);
    });
}
installHelpKeyboard();

// Keyboard shortcuts for SSR entries-family pages. Only active when a
// `[data-entries-list]` is present so other pages don't bind these keys.
function installEntriesKeyboard() {
    if (!document.querySelector('[data-entries-list]')) return;
    // Track the active row by entry id, not by DOM node. Opening an
    // entry runs a multi-target swap that replaces the row element, so
    // a cached node reference becomes orphaned — subsequent `j`/`k`
    // would then fall back to the top of the list because `indexOf`
    // returns -1 for the detached node.
    let activeId = null;
    const rows = () => Array.from(document.querySelectorAll('[data-entry-row]'));
    const activeRow = () => activeId
        ? document.querySelector(`[data-entry-row][data-entry-id="${activeId}"]`)
        : null;
    const focusRow = (row) => {
        if (!row) return;
        const prev = activeRow();
        if (prev && prev !== row) prev.classList.remove('selected');
        row.classList.add('selected');
        row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        activeId = row.getAttribute('data-entry-id');
    };
    const move = (delta) => {
        const all = rows();
        if (all.length === 0) return;
        const current = activeRow();
        const idx = current ? all.indexOf(current) : -1;
        const next = Math.max(0, Math.min(all.length - 1, idx + delta));
        focusRow(all[next]);
    };
    // Multi-target swaps (open entry, toggle star, toggle read) replace
    // the active row's DOM node — the server-rendered replacement has no
    // way to carry over the client-side `.selected` class. Re-apply it
    // after every swap so the list highlights stay aligned with `activeId`.
    document.addEventListener('rdrs:swap-complete', () => {
        const row = activeRow();
        if (row) row.classList.add('selected');
    });
    // Clicking an entry's title link is the mouse equivalent of pressing
    // `o`/`Enter`. Sync `activeId` here so subsequent `j`/`k` continue
    // from the clicked row instead of jumping back to whatever was last
    // selected via keyboard.
    document.addEventListener('click', (e) => {
        const link = e.target.closest('[data-entry-row] a[data-swap="#reading-pane"]');
        if (!link) return;
        const row = link.closest('[data-entry-row]');
        if (row) focusRow(row);
    });
    // Resolve a reading-pane action form by URL suffix, skipping it if
    // the form's submit button is disabled (e.g. Summarize while a
    // request is in-flight). Returns null when no entry is loaded.
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
            case 'j': e.preventDefault(); move(1); break;
            case 'k': e.preventDefault(); move(-1); break;
            case 'Enter': {
                const current = activeRow();
                if (!current) return;
                e.preventDefault();
                const link = current.querySelector('a[data-swap]');
                if (link) link.click();
                break;
            }
            case 's': {
                const current = activeRow();
                if (!current) return;
                // Row form's action is state-dependent now (`/star` or
                // `/unstar`) — match either so the keystroke still flips
                // the entry's starred state in one binding.
                const form = current.querySelector('form[action$="/star"], form[action$="/unstar"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case '1':
            case '2':
            case '3':
            case '4': {
                // Status-filter quick-nav on feed/category pages. The
                // `[data-status-filter] <select>` has 4 options in
                // order: All / Unread / Read / Starred. `1`-`4`
                // navigate to the nth option's URL. No-op on pages
                // without a filter select.
                const options = document.querySelectorAll('[data-status-filter] option');
                if (options.length === 0) return;
                const idx = parseInt(e.key, 10) - 1;
                if (idx < 0 || idx >= options.length) return;
                e.preventDefault();
                window.location.href = options[idx].value;
                break;
            }
            case 'o': {
                // Mark Above as Read — only fires on pages that render
                // the button (feed/category). Delegates to the button's
                // click handler so the confirm + fetch flow lives in
                // one place.
                const btn = document.getElementById('mark-above-read');
                if (!btn) return;
                e.preventDefault();
                btn.click();
                break;
            }
            case 'K': {
                // Mark All as Read — drives the list-pane "Mark as Read"
                // dropdown's "All entries" option so the confirm + POST
                // flow lives in one place. Only fires on pages that render
                // the dropdown (feed / category / inbox).
                const select = document.getElementById('mark-read-age');
                if (!select) return;
                e.preventDefault();
                select.value = 'all';
                select.dispatchEvent(new Event('change'));
                break;
            }
            case 'c': {
                // Prefer the selected entry's own category. The row's
                // meta row already renders `<a href="/categories/{id}/…">`
                // so we just follow it.
                const current = activeRow();
                const fromRow = current?.querySelector('.entry-item-meta a[href^="/categories/"]');
                if (fromRow) {
                    e.preventDefault();
                    window.location.href = fromRow.getAttribute('href');
                    break;
                }
                // No selection — fall back to the page-parent shortcut
                // that only fires on `/feeds/{id}/entries`.
                if (!window.location.pathname.startsWith('/feeds/')) return;
                const sb = document.querySelector('rdrs-sidebar');
                const catId = sb && sb.getAttribute('active-category-id');
                if (!catId) return;
                e.preventDefault();
                window.location.href = `/categories/${catId}/entries`;
                break;
            }
            case 'f': {
                // Jump to the selected entry's feed page.
                const current = activeRow();
                const link = current?.querySelector('.entry-item-meta a[href^="/feeds/"]');
                if (!link) return;
                e.preventDefault();
                window.location.href = link.getAttribute('href');
                break;
            }
            case 'u':
            case 'r': {
                // Toggle read/unread on the active row. Mirrors the `s`
                // (toggle star) and Space-when-pane-empty patterns —
                // the row form's action is state-dependent (`/read` vs
                // `/unread`), so matching either flips the entry in one
                // binding regardless of current state.
                const current = activeRow();
                if (!current) return;
                const form = current.querySelector('form[action$="/read"], form[action$="/unread"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case 'F': {
                // Toggle between feed-supplied and externally-fetched
                // article body. When the pane already shows the full
                // content the Fetch button is replaced by a "Show
                // Original" link — fall through to that.
                const form = paneForm('/fetch-full-content');
                if (form) { e.preventDefault(); form.requestSubmit(); break; }
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                const showOriginal = pane.querySelector('a[data-swap="#reading-pane"]');
                if (showOriginal) { e.preventDefault(); showOriginal.click(); }
                break;
            }
            case 'b': {
                // Open Original — open the row's external link in a new
                // tab. The row only renders the `<a target="_blank">`
                // when `r.link` is Some, so absence = no-op.
                const current = activeRow();
                const link = current?.querySelector('a[target="_blank"]');
                if (!link) return;
                e.preventDefault();
                link.click();
                break;
            }
            case 'B': {
                // Save (Linkding etc). Form is rendered only when the
                // user has a save target configured — absent = no-op.
                const form = paneForm('/save');
                if (!form) return;
                e.preventDefault();
                form.requestSubmit();
                break;
            }
            case 'M': {
                // Summarize via Kagi. Form is rendered only when Kagi is
                // configured (or a summary is in-flight, in which case
                // the button is disabled and paneForm() returns null).
                const form = paneForm('/summarize');
                if (!form) return;
                e.preventDefault();
                form.requestSubmit();
                break;
            }
            case 'x': {
                // On `/categories/{id}/entries`, jump to the unread inbox.
                if (!window.location.pathname.startsWith('/categories/')) return;
                e.preventDefault();
                window.location.href = '/';
                break;
            }
            case '[':
            case ']':
            case '{':
            case '}': {
                // Prev/Next category nav — only on /categories/{id}/entries
                // where "current category" is unambiguous. `[`/`]` walk the
                // full sidebar list (with wrap); `Shift+[`/`Shift+]` (which
                // come through as `{`/`}` on US layout) skip categories with
                // zero unread. Decide shift-vs-not from the resulting
                // character (`{`/`}`) rather than `e.shiftKey` so test
                // harnesses that synthesize the character without the
                // modifier still hit the unread-skipping branch.
                const m = window.location.pathname.match(/^\/categories\/(\d+)\/entries/);
                if (!m) return;
                const sb = document.querySelector('rdrs-sidebar');
                const cats = sb?._data?.categories || [];
                if (cats.length === 0) return;
                const currentId = parseInt(m[1], 10);
                const idx = cats.findIndex(c => c.id === currentId);
                if (idx === -1) return;
                const len = cats.length;
                const forward = e.key === ']' || e.key === '}';
                const step = forward ? 1 : -1;
                const unreadOnly = e.key === '{' || e.key === '}';
                let target = null;
                if (unreadOnly) {
                    for (let i = 1; i <= len; i++) {
                        const probe = cats[((idx + i * step) % len + len) % len];
                        if (probe.unread_count > 0 && probe.id !== currentId) {
                            target = probe;
                            break;
                        }
                    }
                } else if (len > 1) {
                    target = cats[((idx + step) % len + len) % len];
                }
                if (!target) return;
                e.preventDefault();
                window.location.href = `/categories/${target.id}/entries`;
                break;
            }
            case 'Escape': {
                // Esc closes the reading pane back to its empty state.
                // If the help overlay is open it owns the Esc handler
                // (in its own shadow root), so we yield to it.
                const help = document.querySelector('rdrs-kb-help');
                if (help && help.isVisible) return;
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                e.preventDefault();
                pane.classList.add('reading-pane-empty');
                pane.innerHTML = '<p>Select an entry to read.</p>';
                break;
            }
            case ' ': {
                // Classic feed-reader convention: when an entry is loaded
                // in the reading pane, Space pages the article down (and
                // Shift+Space pages up). Falls back to toggle-read on the
                // active list row when the pane is empty.
                const pane = document.getElementById('reading-pane');
                if (pane && !pane.classList.contains('reading-pane-empty')) {
                    e.preventDefault();
                    const dir = e.shiftKey ? -1 : 1;
                    pane.scrollBy({ top: dir * pane.clientHeight * 0.85, behavior: 'smooth' });
                    break;
                }
                const current = activeRow();
                if (!current) return;
                const form = current.querySelector('form[action$="/read"], form[action$="/unread"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
        }
    });
}
installEntriesKeyboard();

// Reading-pane summary controls (Kagi Universal Summarizer output).
// Copy is a clipboard write; Dismiss DELETEs the cached summary and
// strips the summary block + the entry row's summary badge so the
// state matches what `/sidebar/unread` will report on the next mount.
function installSummaryActions() {
    document.addEventListener('click', async (e) => {
        const copyBtn = e.target.closest('[data-summary-copy]');
        if (copyBtn) {
            const pane = document.getElementById('reading-pane');
            if (!pane) return;
            const titleEl = pane.querySelector('.reading-pane-title');
            const summaryEl = pane.querySelector('.rp-summary-content');
            if (!summaryEl) return;
            const title = (titleEl?.textContent || '').trim();
            const link = titleEl?.querySelector('a')?.getAttribute('href') || '';
            const summary = summaryEl.textContent.trim();
            const text = link
                ? `${title}\n\n${link}\n\n${summary}`
                : `${title}\n\n${summary}`;
            try {
                await navigator.clipboard.writeText(text);
                const original = copyBtn.textContent;
                copyBtn.textContent = 'Copied!';
                setTimeout(() => { copyBtn.textContent = original; }, 2000);
            } catch {
                window.flash?.error('Failed to copy to clipboard');
            }
            return;
        }
        const dismissBtn = e.target.closest('[data-summary-dismiss]');
        if (!dismissBtn) return;
        const entryId = dismissBtn.getAttribute('data-entry-id');
        if (!entryId) return;
        dismissBtn.disabled = true;
        try {
            const r = await fetch(`/api/entries/${entryId}/summary`, {
                method: 'DELETE',
                credentials: 'same-origin',
            });
            if (!r.ok) throw new Error('delete failed');
            // Clear the inner `.summary-box` but leave the wrapper in
            // place — the swap target for a later summarize click is
            // `#rp-summary-container`, so the wrapper has to stay.
            const container = document.querySelector('[data-summary-container]');
            if (container) container.replaceChildren();
            const row = document.querySelector(
                `[data-entry-row][data-entry-id="${entryId}"]`
            );
            row?.querySelector(
                '.summary-badge, .summary-badge-pending, .summary-badge-processing, .summary-badge-failed'
            )?.remove();
        } catch {
            window.flash?.error('Failed to dismiss summary');
            dismissBtn.disabled = false;
        }
    });
}
installSummaryActions();

// "Mark as Read..." dropdown on /unread + /entries pages. Posts to the
// GReader bulk-mark endpoint with an optional `ts=` cutoff in
// microseconds, then full-reloads so the SSR row list refreshes. The
// GReader API is permanent per the SSR-first spec, so this JS-glue is
// the long-term home for the dropdown (the alternative — a native
// form-POST — would navigate the user away to a JSON response).
const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};
const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';

function installMarkAsReadDropdown() {
    const select = document.getElementById('mark-read-age');
    if (!select) return;
    select.addEventListener('change', async () => {
        const age = select.value;
        select.selectedIndex = 0;
        if (!age) return;
        const ageLabel = AGE_LABELS[age] || age;
        if (!confirm(`Mark ${ageLabel} entries as read?`)) return;
        // `data-mark-read-scope` on the <select> carries the GReader stream
        // ID for the current page (e.g. `feed/<url>` or `user/-/label/<cat>`),
        // letting the same dropdown scope mark-as-read to whatever list the
        // user is currently viewing. Falls back to the global reading-list.
        const scope = select.dataset.markReadScope || READING_LIST_STREAM;
        const body = new URLSearchParams();
        body.set('s', scope);
        if (age !== 'all') {
            const days = parseInt(age, 10);
            const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
            body.set('ts', tsUsec.toString());
        }
        select.disabled = true;
        select.setAttribute('aria-busy', 'true');
        try {
            const resp = await fetch('/reader/api/0/mark-all-as-read', {
                method: 'POST',
                body,
                credentials: 'same-origin',
            });
            if (!resp.ok) throw new Error('Failed to mark as read');
            if (window.flash) {
                window.flash.set('success', `Marked ${ageLabel} entries as read.`);
            }
            window.location.reload();
            return;
        } catch (err) {
            const message = err.message || 'Failed to mark as read';
            if (window.flash) {
                window.flash.error(message);
            } else {
                alert(message);
            }
        } finally {
            select.disabled = false;
            select.removeAttribute('aria-busy');
        }
    });
}
installMarkAsReadDropdown();

// Status-filter <select> on feed + category pages. Each option's value
// is the URL to navigate to; the active option is pre-selected by the
// server. The 1-4 keys hit the same options by position.
function installStatusFilterSelect() {
    const select = document.getElementById('status-filter');
    if (!select) return;
    select.addEventListener('change', () => {
        const url = select.value;
        if (url) window.location.href = url;
    });
}
installStatusFilterSelect();

// "Mark Above as Read" button on feed + category pages. Sits at the
// bottom of the list (below Load More) and marks every entry currently
// rendered in the DOM — loaded rows + anything appended via Load More.
// Entries that haven't been loaded yet stay untouched. Posts to the
// GReader edit-tag endpoint with one `i=<id>` per visible row and
// `a=user/-/state/com.google/read`.
function installMarkAboveButton() {
    const btn = document.getElementById('mark-above-read');
    if (!btn) return;
    btn.addEventListener('click', async () => {
        const rows = Array.from(document.querySelectorAll('[data-entry-row]'));
        const ids = rows.map(r => r.dataset.entryId).filter(Boolean);
        if (ids.length === 0) {
            const msg = 'No entries to mark.';
            if (window.flash) { window.flash.info(msg); } else { alert(msg); }
            return;
        }
        if (!confirm(`Mark ${ids.length} loaded entries as read?`)) return;
        const body = new URLSearchParams();
        for (const id of ids) body.append('i', id);
        body.set('a', 'user/-/state/com.google/read');
        btn.disabled = true;
        btn.setAttribute('aria-busy', 'true');
        try {
            const resp = await fetch('/reader/api/0/edit-tag', {
                method: 'POST',
                body,
                credentials: 'same-origin',
            });
            if (!resp.ok) throw new Error('Failed to mark entries as read');
            if (window.flash) {
                window.flash.set('success', `Marked ${ids.length} loaded entries as read.`);
            }
            window.location.reload();
            return;
        } catch (err) {
            const message = err.message || 'Failed to mark entries as read';
            if (window.flash) { window.flash.error(message); } else { alert(message); }
        } finally {
            btn.disabled = false;
            btn.removeAttribute('aria-busy');
        }
    });
}
installMarkAboveButton();

// Entry-row click delegation. Clicking anywhere on a row (not just the
// title link) opens the entry. Delegates to the title's
// `<a data-swap="#reading-pane">` so `installSwap()` handles the
// multi-target response (auto-mark-as-read, sidebar update).
function installRowClickToOpen() {
    document.addEventListener('click', (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey ||
            event.shiftKey || event.altKey) return;
        const row = event.target.closest('[data-entry-row]');
        if (!row) return;
        // Already-handled targets: action buttons + the title link itself.
        if (event.target.closest('.entry-item-actions')) return;
        if (event.target.closest('a[data-swap="#reading-pane"]')) return;
        // Defer to any other link the user clicked (e.g. feed-title link
        // in the meta row, if/when one is added).
        if (event.target.closest('a')) return;
        const link = row.querySelector('a[data-swap="#reading-pane"]');
        if (!link) return;
        event.preventDefault();
        link.click();
    });
}
installRowClickToOpen();
