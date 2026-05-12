// static/js/app.js — shared module for the logged-in surface.
//
// Currently ships:
//   - swap(): partial-swap helper used by per-page SSR PRs to replace
//     a target element via fetch + outerHTML. Not yet used by any
//     consumer in PR-2.
//   - window.rdrsNavigate: full-reload stub. Replaces the SPA router's
//     export of the same name so existing CSR call sites in
//     keyboard.js / page modules continue to work after router.js is
//     removed. Each call falls through to a full document load.
//
// Per-page SSR PRs (PR-3+) extend this module with keyboard shortcuts,
// sidebar polling, flash dismiss, and theme controller code. Those
// sections are intentionally absent here.

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
        return;
    }

    const dst = document.querySelector(defaultTarget);
    if (!dst) return;
    const incoming = parsed.body.firstElementChild;
    if (!incoming) return;
    dst.outerHTML = incoming.outerHTML;
    applyFlashTemplates(parsed);
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

// Keyboard shortcuts for SSR entries-family pages. Only active when a
// `[data-entries-list]` is present so we don't conflict with the
// legacy `keyboard.js` running on PR-11 CSR routes.
function installEntriesKeyboard() {
    if (!document.querySelector('[data-entries-list]')) return;
    let active = null; // currently focused entry row
    const rows = () => Array.from(document.querySelectorAll('[data-entry-row]'));
    const focusRow = (row) => {
        if (!row) return;
        if (active) active.classList.remove('selected');
        active = row;
        row.classList.add('selected');
        row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    };
    const move = (delta) => {
        const all = rows();
        if (all.length === 0) return;
        const idx = active ? all.indexOf(active) : -1;
        const next = Math.max(0, Math.min(all.length - 1, idx + delta));
        focusRow(all[next]);
    };
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        switch (e.key) {
            case 'j': e.preventDefault(); move(1); break;
            case 'k': e.preventDefault(); move(-1); break;
            case 'Enter':
            case 'o': {
                if (!active) return;
                e.preventDefault();
                const link = active.querySelector('a[data-swap]');
                if (link) link.click();
                break;
            }
            case 's': {
                if (!active) return;
                // Row form's action is state-dependent now (`/star` or
                // `/unstar`) — match either so the keystroke still flips
                // the entry's starred state in one binding.
                const form = active.querySelector('form[action$="/star"], form[action$="/unstar"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case '1':
            case '2':
            case '3':
            case '4': {
                // Status-filter quick-nav on feed/category pages. The
                // `[data-status-filter]` tab-bar carries 4 anchors in
                // order: All / Unread / Read / Starred. `1`-`4` click
                // the nth tab. On pages without filter tabs the keys
                // are a no-op.
                const tabs = document.querySelectorAll('[data-status-filter] [data-status-tab]');
                if (tabs.length === 0) return;
                const idx = parseInt(e.key, 10) - 1;
                if (idx < 0 || idx >= tabs.length) return;
                e.preventDefault();
                window.location.href = tabs[idx].getAttribute('href');
                break;
            }
            case 'A': {
                // Mark-Above-as-Read — only fires on pages that render
                // the button (feed/category). Delegates to the button's
                // click handler so the prompt + fetch flow stays in one
                // place.
                const btn = document.getElementById('mark-above-read');
                if (!btn) return;
                e.preventDefault();
                btn.click();
                break;
            }
            case 'c': {
                // On `/feeds/{id}/entries`, jump to the feed's parent
                // category page. The category id is already on the
                // sidebar via `active-category-id` so we reuse it.
                if (!window.location.pathname.startsWith('/feeds/')) return;
                const sb = document.querySelector('rdrs-sidebar');
                const catId = sb && sb.getAttribute('active-category-id');
                if (!catId) return;
                e.preventDefault();
                window.location.href = `/categories/${catId}/entries`;
                break;
            }
            case 'x': {
                // On `/categories/{id}/entries`, jump to the unread inbox.
                if (!window.location.pathname.startsWith('/categories/')) return;
                e.preventDefault();
                window.location.href = '/';
                break;
            }
            case ' ': {
                if (!active) return;
                // Row form is now state-dependent: `/read` when unread,
                // `/unread` when already read. Either way, this single
                // keyboard binding flips the entry's read state.
                const form = active.querySelector('form[action$="/read"], form[action$="/unread"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
        }
    });
}
installEntriesKeyboard();

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

// "Mark Above as Read" button on feed + category pages. Posts to the
// GReader edit-tag endpoint with one `i=<id>` per entry above the
// currently-focused row + `a=user/-/state/com.google/read`. Matches the
// pre-SSR `<rdrs-entry-list>` behaviour (mark-by-id, not by timestamp —
// only entries currently in the DOM get marked).
function installMarkAboveButton() {
    const btn = document.getElementById('mark-above-read');
    if (!btn) return;
    btn.addEventListener('click', async () => {
        const selected = document.querySelector('[data-entry-row].selected');
        if (!selected) {
            const msg = 'Select an entry first (press j or click a row).';
            if (window.flash) { window.flash.error(msg); } else { alert(msg); }
            return;
        }
        const allRows = Array.from(document.querySelectorAll('[data-entry-row]'));
        const idx = allRows.indexOf(selected);
        if (idx <= 0) {
            const msg = 'No entries above the selected one.';
            if (window.flash) { window.flash.info(msg); } else { alert(msg); }
            return;
        }
        const aboveIds = allRows.slice(0, idx)
            .map(r => r.dataset.entryId)
            .filter(Boolean);
        if (aboveIds.length === 0) return;
        if (!confirm(`Mark ${aboveIds.length} entries above as read?`)) return;
        const body = new URLSearchParams();
        for (const id of aboveIds) body.append('i', id);
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
                window.flash.set('success', `Marked ${aboveIds.length} entries above as read.`);
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
// title link) opens the entry — matches the pre-SSR `<rdrs-entry-list>`
// UX. We delegate to the title's `<a data-swap="#reading-pane">` so the
// existing `installSwap()` handler picks it up (multi-target response,
// auto-mark-as-read, sidebar update).
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
