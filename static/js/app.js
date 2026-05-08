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
