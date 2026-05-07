// static/js/router.js
// SPA router — intercepts internal-link clicks and swaps the page element
// in place instead of triggering a full document reload. Loaded by
// app_shell.html after the page-module script.
//
// First paint is still server-rendered (handler returns the shell with
// element_tag + script_path). The router takes over from there:
//   - Document-level click handler intercepts internal <a> clicks.
//   - history.pushState updates the URL.
//   - Dynamic import() loads the matching page module (cached after first).
//   - The fresh page element replaces #page-host's contents; its
//     connectedCallback runs and the page initialises normally.
//
// Page-element modules do NOT import this file. They emit plain
// <a href="/..."> links. The router intercepts at the document level.

const ROUTES = [
    { pattern: /^\/$/,                                      element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/entries$/,                               element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/entries\/(?:read|starred|summarized)$/,  element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/feeds\/\d+\/entries$/,                   element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/categories\/\d+\/entries$/,              element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/search$/,                                element: 'rdrs-entries-page',       script: '/static/js/pages/entries.js' },
    { pattern: /^\/feeds$/,                                 element: 'rdrs-feeds-page',         script: '/static/js/pages/feeds.js' },
    { pattern: /^\/categories$/,                            element: 'rdrs-categories-page',    script: '/static/js/pages/categories.js' },
    { pattern: /^\/admin$/,                                 element: 'rdrs-admin-page',         script: '/static/js/pages/admin.js' },
    { pattern: /^\/settings$/,                              element: 'rdrs-settings-page',      script: '/static/js/pages/settings.js' },
    { pattern: /^\/user-settings$/,                         element: 'rdrs-user-settings-page', script: '/static/js/pages/user-settings.js' },
    { pattern: /^\/statistics$/,                            element: 'rdrs-statistics-page',    script: '/static/js/pages/statistics.js' },
];

function matchRoute(pathname) {
    return ROUTES.find(r => r.pattern.test(pathname)) ?? null;
}

let navSeq = 0;

async function navigateTo(path, opts = {}) {
    const url = new URL(path, location.origin);
    const route = opts.route ?? matchRoute(url.pathname);
    if (!route) {
        location.href = path;
        return;
    }

    if (!opts.skipPushState) {
        history.pushState(null, '', path);
    }

    const mySeq = ++navSeq;

    // Skip the dynamic import when the page-element constructor is already
    // in the registry. The shell loads the current page's module via a
    // versioned URL (?v=GIT_VERSION); calling import() with the unversioned
    // URL here would create a SECOND module instance and re-trigger
    // customElements.define, which throws NotSupportedError. Once any page
    // module has run, its element is registered and we can mount it
    // without re-importing.
    if (!customElements.get(route.element)) {
        try {
            await import(route.script);
        } catch {
            location.href = path;
            return;
        }
    }
    if (mySeq !== navSeq) return; // superseded by a later nav

    const host = document.getElementById('page-host');
    if (!host) {
        location.href = path;
        return;
    }
    const newEl = document.createElement(route.element);
    host.replaceChildren(newEl);

    if (!opts.skipPushState) {
        window.scrollTo(0, 0);
    }
    // popstate-driven nav inherits the browser's auto scroll restoration.
}

function shouldIntercept(event, anchor) {
    if (event.defaultPrevented) return false;
    if (event.button !== 0) return false; // right/middle click
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return false;
    if (anchor.target && anchor.target !== '' && anchor.target !== '_self') return false;
    if (anchor.hasAttribute('download')) return false;
    const rel = anchor.getAttribute('rel');
    if (rel && rel.split(/\s+/).includes('external')) return false;

    let url;
    try {
        url = new URL(anchor.href, location.origin);
    } catch {
        return false;
    }
    if (url.origin !== location.origin) return false;

    return matchRoute(url.pathname) !== null;
}

document.addEventListener('click', (event) => {
    const anchor = event.target.closest('a');
    if (!anchor) return;
    if (!shouldIntercept(event, anchor)) return;

    const url = new URL(anchor.href, location.origin);
    event.preventDefault();
    navigateTo(url.pathname + url.search);
});

window.addEventListener('popstate', () => {
    navigateTo(location.pathname + location.search, { skipPushState: true });
});

// Programmatic entry point for in-app navigation. Page modules call
// `window.rdrsNavigate('/path')` from keyboard handlers, dropdown
// onchange, etc. — anywhere they used to do `window.location.href = ...`
// for an in-app destination. Falls back to a full reload for paths
// outside the route table (e.g. /login).
window.rdrsNavigate = (path, opts) => navigateTo(path, opts);
