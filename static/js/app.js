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
        const action = form.action;
        const init = { method };
        if (method !== 'GET') {
            init.body = new FormData(form);
        }
        await performSwap(action, init, target);
    });
}

async function performSwap(url, init, defaultTarget) {
    let response;
    try {
        response = await fetch(url, init);
    } catch {
        window.location.href = url;
        return;
    }
    if (!response.ok) {
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
            const incoming = tpl.content.firstElementChild;
            if (!incoming) continue;
            dst.outerHTML = incoming.outerHTML;
        }
        return;
    }

    const dst = document.querySelector(defaultTarget);
    if (!dst) return;
    const incoming = parsed.body.firstElementChild;
    if (!incoming) return;
    dst.outerHTML = incoming.outerHTML;
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
                const form = active.querySelector('form[action$="/star"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case ' ': {
                if (!active) return;
                const form = active.querySelector('form[action$="/read"]');
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
        const body = new URLSearchParams();
        body.set('s', READING_LIST_STREAM);
        if (age !== 'all') {
            const days = parseInt(age, 10);
            const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
            body.set('ts', tsUsec.toString());
        }
        try {
            const resp = await fetch('/reader/api/0/mark-all-as-read', {
                method: 'POST',
                body,
                credentials: 'same-origin',
            });
            if (!resp.ok) throw new Error('Failed to mark as read');
            window.location.reload();
        } catch (err) {
            alert(err.message || 'Failed to mark as read');
        }
    });
}
installMarkAsReadDropdown();
