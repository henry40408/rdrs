// Shared module for the logged-in surface: swaps, theme, shortcuts, mark-as-read.

// `?v=` is substituted at serve time; unversioned, this import would go stale
// forever under the `immutable` cache header.
import { debounce } from './utils.js?v=__RDRS_ASSET_VERSION__';

/**
 * Intercept forms/links tagged `data-swap="<selector>"` and swap in the response:
 * a bare fragment replaces the target; `<template data-swap-target>` blocks each
 * replace their own target; `<template data-class-target data-class-add|remove>`
 * toggles classes on an element that is not replaced. Non-2xx falls back to a
 * native submit / navigation.
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
        event.preventDefault();
        // Only the action-bar Summarize form carries this; Retry still regenerates.
        if (form.hasAttribute('data-summary-toggle')) {
            if (summaryInFlight()) return; // Cancel lives in the summary box.
            if (dismissVisibleSummary()) return;
        }
        if (form.matches('[data-cancel-swap][aria-busy="true"]')) {
            abortFormSwap(form);
            return;
        }
        const target = form.getAttribute('data-swap');
        const method = (form.method || 'GET').toUpperCase();
        const init = { method };
        const controller = form.hasAttribute('data-cancel-swap') ? new AbortController() : null;
        if (controller) {
            init.signal = controller.signal;
            formSwapAborts.set(form, controller);
        }
        let url = form.action;
        if (method === 'GET') {
            // Otherwise hidden inputs (e.g. Load More's `after=`) are dropped.
            const params = new URLSearchParams(new FormData(form));
            const sep = url.includes('?') ? '&' : '?';
            url = url + sep + params.toString();
        } else {
            init.body = new FormData(form);
        }
        setFormBusy(form, { cancellable: !!controller });
        try {
            await performSwap(url, init, target);
            // Mirror the search into the address bar so refresh/share reproduce it
            // and clearing removes a stale `?q=`.
            if (form.matches('[data-entries-search]')) {
                syncScopedSearchParam(form);
            }
        } finally {
            // On a POST error the form is still mounted and gets its button back.
            formSwapAborts.delete(form);
            clearFormBusy(form);
        }
    });
}

const formSwapAborts = new WeakMap();

// Entry ids with an 'm' toggle POST in flight; stops double-press double-POSTs.
const pendingRowToggles = new Set();

function abortFormSwap(form) {
    const controller = formSwapAborts.get(form);
    if (!controller) return;
    controller.abort();
}

// Busy-state labels for slow form-swap actions; others just get `disabled`.
const BUSY_LABELS = {
    save: 'Saving…',
    'fetch-full-content': 'Fetching…',
    summarize: 'Summarizing…',
};

function deriveBusyLabel(actionUrl) {
    const m = (actionUrl || '').match(/\/entries\/\d+\/([\w-]+)/);
    return m ? BUSY_LABELS[m[1]] : null;
}

function setFormBusy(form, options = {}) {
    form.setAttribute('aria-busy', 'true');
    const btn = form.querySelector('button[type="submit"], button:not([type])');
    if (!btn) return;
    const label = options.cancellable
        ? btn.dataset.cancelLabel
        : deriveBusyLabel(form.action);
    btn.dataset.busyOriginalAriaLabel = btn.getAttribute('aria-label') || '';
    if (options.cancellable) {
        btn.classList.add('is-cancel');
        btn.setAttribute('aria-label', btn.dataset.cancelAriaLabel || label || 'Cancel');
        const defaultIcon = btn.querySelector('.action-icon-default');
        const cancelIcon = btn.querySelector('.action-icon-cancel');
        if (defaultIcon && cancelIcon) {
            defaultIcon.hidden = true;
            cancelIcon.hidden = false;
        }
    } else {
        btn.disabled = true;
    }
    if (label) {
        // Write `.action-label`, not `textContent`, or the icon is lost for good.
        const labelEl = btn.querySelector('.action-label') || btn;
        btn.dataset.busyOriginalLabel = labelEl.textContent;
        labelEl.textContent = label;
    }
}

function clearFormBusy(form) {
    form.removeAttribute('aria-busy');
    const btn = form.querySelector('button[type="submit"], button:not([type])');
    if (!btn) return;
    btn.disabled = false;
    btn.classList.remove('is-cancel');
    if (btn.dataset.busyOriginalAriaLabel != null) {
        if (btn.dataset.busyOriginalAriaLabel) {
            btn.setAttribute('aria-label', btn.dataset.busyOriginalAriaLabel);
        } else {
            btn.removeAttribute('aria-label');
        }
        delete btn.dataset.busyOriginalAriaLabel;
    }
    const defaultIcon = btn.querySelector('.action-icon-default');
    const cancelIcon = btn.querySelector('.action-icon-cancel');
    if (defaultIcon && cancelIcon) {
        defaultIcon.hidden = false;
        cancelIcon.hidden = true;
    }
    if (btn.dataset.busyOriginalLabel != null) {
        const labelEl = btn.querySelector('.action-label') || btn;
        labelEl.textContent = btn.dataset.busyOriginalLabel;
        delete btn.dataset.busyOriginalLabel;
    }
}

// Abort image downloads in the outgoing pane: detached `<img>`s aren't reliably
// cancelled and slow proxy downloads starve the HTTP/1.1 connection slots.
// Content images only; cancelling the favicon would make it flash.
function cancelPaneImages(pane) {
    if (!pane) return;
    for (const img of pane.querySelectorAll('.reading-pane-article img[src]')) {
        img.removeAttribute('src');
    }
}

function markBrokenImage(img) {
    const box = document.createElement('div');
    box.className = 'rp-broken-image';
    box.innerHTML =
        '<svg class="ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
        'stroke-width="1.5" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/>' +
        '<path d="M3 16l5-5 4 4"/><circle cx="8.5" cy="9" r="1.3"/><path d="M4 4l16 16"/></svg>';
    const cap = document.createElement('span');
    cap.className = 'rp-broken-cap';
    const alt = (img.getAttribute('alt') || '').trim();
    // textContent, never innerHTML, so alt text can't inject markup.
    cap.textContent = alt ? `Image unavailable — ${alt}` : 'Image unavailable';
    box.appendChild(cap);
    const w = img.getAttribute('width');
    const h = img.getAttribute('height');
    if (w && h) box.style.aspectRatio = `${w} / ${h}`;
    img.replaceWith(box);
}

function initPaneImages() {
    const pane = document.getElementById('reading-pane');
    if (!pane) return;
    for (const img of pane.querySelectorAll('.reading-pane-article img:not([data-img-init])')) {
        img.setAttribute('data-img-init', '');
        if (img.complete) {
            if (img.naturalWidth > 0) img.setAttribute('data-img-state', 'loaded');
            else markBrokenImage(img);
            continue;
        }
        img.addEventListener('load', () => img.setAttribute('data-img-state', 'loaded'), { once: true });
        img.addEventListener('error', () => {
            // Dropped `src` = aborted by cancelPaneImages(), not a failure.
            if (!img.getAttribute('src')) return;
            markBrokenImage(img);
        }, { once: true });
    }
}

// Token + abort handle for reading-pane navigation fetches, so a slow earlier
// entry can't overwrite the one just opened. Action swaps stay outside the guard.
let paneNavSeq = 0;
let paneNavAbort = null;

// Swap targets inside the reading pane; see the staleness check in performSwap().
const PANE_REGION_TARGETS = new Set(['#reading-pane', '#rp-summary-container']);

/// The server's last markup per swap target: an identical response means the
/// DOM already shows it, so skip the swap (WebKit blinks images on repaint).
/// Compared to the previous response, not the DOM, which carries client-only
/// state. Morph targets are exempt: other swaps edit their DOM.
const lastServerMarkup = new Map();

/// The single element a swap template carries, or null (e.g. Load More's N rows).
function soleSwapElement(tpl) {
    const nodes = Array.from(tpl.content.childNodes)
        .filter((n) => n.nodeType !== Node.TEXT_NODE || n.textContent.trim() !== '');
    if (nodes.length !== 1 || nodes[0].nodeType !== Node.ELEMENT_NODE) return null;
    return nodes[0];
}

/// Targets morphed rather than replaced, so surviving rows keep their nodes and
/// favicons. Others replace on purpose (e.g. `#reading-pane` resets scroll).
function isMorphTarget(selector) {
    return selector === '[data-entries-list]' || selector.startsWith('#entry-row-');
}

/// Client-written attributes a morph must keep. Losing `data-…-bound` would
/// bind a second listener: one click, two POSTs.
const CLIENT_OWNED_ATTR = /^(data-.+-bound|data-img-.+|data-localized|data-tooltip-at|title)$/;

/// `.selected` is the `j`/`k` cursor, which the server never renders.
const CLIENT_OWNED_CLASSES = ['selected'];

function morphAttributes(from, to) {
    const mine = CLIENT_OWNED_CLASSES.filter((c) => from.classList.contains(c));
    for (const { name, value } of to.attributes) {
        if (from.getAttribute(name) !== value) from.setAttribute(name, value);
    }
    for (const name of from.getAttributeNames()) {
        if (to.hasAttribute(name) || CLIENT_OWNED_ATTR.test(name)) continue;
        from.removeAttribute(name);
    }
    for (const c of mine) from.classList.add(c);
}

function morphCompatible(from, to) {
    if (!from || from.nodeType !== to.nodeType) return false;
    if (from.nodeType !== Node.ELEMENT_NODE) return true;
    if (from.tagName !== to.tagName) return false;
    // An id is a key: differently-keyed elements are different elements.
    return (from.id || '') === (to.id || '');
}

/// Reshape `from`'s children into `to`'s, reusing nodes: matched by `id` when
/// present, otherwise positionally.
function morphChildren(from, to) {
    const keyed = new Map();
    for (const el of from.children) if (el.id) keyed.set(el.id, el);

    let cursor = from.firstChild;
    for (const next of Array.from(to.childNodes)) {
        const key = next.nodeType === Node.ELEMENT_NODE && next.id ? next.id : null;
        const existing = key ? keyed.get(key) : null;
        if (existing) {
            keyed.delete(key);
            if (existing === cursor) cursor = cursor.nextSibling;
            else from.insertBefore(existing, cursor);
            morphNode(existing, next);
            continue;
        }
        // Never consume a keyed node positionally; it may match a later one.
        const reusable = cursor && !(cursor.nodeType === Node.ELEMENT_NODE && cursor.id)
            && morphCompatible(cursor, next) ? cursor : null;
        if (reusable) {
            cursor = cursor.nextSibling;
            morphNode(reusable, next);
            continue;
        }
        from.insertBefore(document.importNode(next, true), cursor);
    }
    while (cursor) {
        const spent = cursor;
        cursor = cursor.nextSibling;
        spent.remove();
    }
    for (const orphan of keyed.values()) orphan.remove();
}

function morphNode(from, to) {
    if (from.nodeType !== Node.ELEMENT_NODE) {
        if (from.nodeValue !== to.nodeValue) from.nodeValue = to.nodeValue;
        return;
    }
    morphAttributes(from, to);
    morphChildren(from, to);
}

/// Morph `dst` into `incoming`'s shape. Re-inserted `<img>`s blink in WebKit;
/// morphed ones are untouched.
function morphSwap(dst, incoming) {
    if (!morphCompatible(dst, incoming)) return false;
    morphNode(dst, incoming);
    return true;
}

/// Attributes re-stamped every render with no visible effect, ignored when
/// comparing (`data-snapshot-at` changes every second).
const VOLATILE_SERVER_ATTRS = ['data-snapshot-at'];

function comparableServerMarkup(el) {
    const clone = el.cloneNode(true);
    for (const name of VOLATILE_SERVER_ATTRS) {
        for (const n of clone.querySelectorAll(`[${name}]`)) n.removeAttribute(name);
        clone.removeAttribute(name);
    }
    return clone.outerHTML;
}

/// Copy those attributes onto the kept DOM, or a skipped swap freezes the
/// snapshot boundary and `j`/`k` mistreats entries as unread.
function syncVolatileAttrs(incoming, live) {
    for (const name of VOLATILE_SERVER_ATTRS) {
        const from = incoming.querySelectorAll(`[${name}]`);
        const onto = live.querySelectorAll(`[${name}]`);
        for (let i = 0; i < Math.min(from.length, onto.length); i++) {
            onto[i].setAttribute(name, from[i].getAttribute(name));
        }
        if (incoming.hasAttribute(name) && live.hasAttribute(name)) {
            live.setAttribute(name, incoming.getAttribute(name));
        }
    }
}

/// Fetch `url` and apply the response. Resolves `false` when superseded, aborted
/// or handed to a full navigation; callers must then skip follow-ups.
/// `options.fallbackUrl` is the error-path destination (`?pane=1` is bare
/// templates, which would render blank).
async function performSwap(url, init, defaultTarget, options) {
    const method = (init.method || 'GET').toUpperCase();
    const fallbackUrl = options?.fallbackUrl || url;
    // popstate passes `skipHistory`: the browser already moved the address bar.
    const skipHistory = options?.skipHistory === true;
    const isPaneNav = method === 'GET' && defaultTarget === '#reading-pane';
    let navSeq = null;
    if (isPaneNav) {
        navSeq = ++paneNavSeq;
        paneNavAbort?.abort();
        paneNavAbort = new AbortController();
        init.signal = paneNavAbort.signal;
    }
    // Only for a different entry; action swaps keep the same images.
    if (defaultTarget === '#reading-pane') {
        const incoming = entryIdFromSwapUrl(url);
        if (incoming && incoming !== currentPaneEntryId()) {
            cancelPaneImages(document.getElementById('reading-pane'));
        }
    }
    let response;
    try {
        response = await fetch(url, init);
    } catch {
        if (init.signal?.aborted) return false;
        // Don't hard-navigate to a fragment the user has moved past.
        if (isPaneNav && navSeq !== paneNavSeq) return false;
        // A thrown request is the first real evidence of a dropped connection
        // (`navigator.onLine` is unreliable); tell offline.js.
        const offline = window.rdrsOffline?.networkFailed?.() === true;
        // Saved offline pane, looked up here (not in the SW) so this stays a
        // page request. Absent without offline.js.
        response = method === 'GET' ? await savedFragment(url) : null;
        if (!response) {
            // Once offline, stay on the list and say so rather than navigating
            // away. Without offline.js, a real navigation surfaces the error.
            if (window.flash && (method !== 'GET' || offline)) {
                // Not "you are offline": a dead server looks the same from here.
                window.flash.error('Could not reach the server — that will have to wait for the connection.');
            } else {
                window.location.href = fallbackUrl;
            }
            return false;
        }
        // A saved pane goes through the same response handling below.
    }
    // Superseded while headers were in flight (abort misses buffered replies).
    if (isPaneNav && navSeq !== paneNavSeq) return false;
    if (!response.ok) {
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return false;
        }
        window.location.href = fallbackUrl;
        return false;
    }
    let text;
    try {
        text = await response.text();
    } catch {
        if (init.signal?.aborted) return false;
        if (isPaneNav && navSeq !== paneNavSeq) return false;
        window.location.href = fallbackUrl;
        return false;
    }
    if (isPaneNav && navSeq !== paneNavSeq) return false;
    const parsed = new DOMParser().parseFromString(text, 'text/html');

    // Decided before mutating: opening from empty pushes (so back closes the
    // pane), switching entries replaces.
    const paneBefore = document.getElementById('reading-pane');
    const paneWasEmpty = !!paneBefore?.classList.contains('reading-pane-empty');
    // Pre-mutation snapshot: a different entry id means navigation and clears
    // stale flashes; an action swap keeps its toast.
    const paneEntryIdBefore = currentPaneEntryId();
    const incomingEntryId = entryIdFromSwapUrl(url);

    // Don't apply an action response to a different entry than it was fired on
    // (e.g. a late SSE summary). Navigation and row targets are exempt.
    if (!isPaneNav && PANE_REGION_TARGETS.has(defaultTarget) &&
        incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
        // The action still happened server-side; only the markup is stale.
        applyFlashTemplates(parsed);
        return false;
    }

    let swappedReadingPane = false;
    const templates = parsed.querySelectorAll('template[data-swap-target]');
    if (templates.length > 0) {
        for (const tpl of templates) {
            const sel = tpl.getAttribute('data-swap-target');
            if (sel === '#reading-pane') swappedReadingPane = true;
            const dst = document.querySelector(sel);
            if (!dst) continue;
            // Never replace the reading pane here: that resets its scroll.
            const sole = sel === '#reading-pane' ? null : soleSwapElement(tpl);
            if (sole && isMorphTarget(sel)) {
                if (morphSwap(dst, sole)) continue;
            } else if (sole) {
                const markup = comparableServerMarkup(sole);
                if (lastServerMarkup.get(sel) === markup) {
                    syncVolatileAttrs(sole, dst);
                    continue;
                }
                lastServerMarkup.set(sel, markup);
            }
            const parent = dst.parentNode;
            // Child-by-child for multi-element payloads (Load More).
            const nodes = Array.from(tpl.content.childNodes);
            for (const node of nodes) {
                parent.insertBefore(node, dst);
            }
            parent.removeChild(dst);
        }
        if (swappedReadingPane && incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
            window.flash?.clear?.();
        }
        if (swappedReadingPane && !skipHistory) syncEntryParamFromSwapUrl(url, { push: paneWasEmpty });
        applyClassTemplates(parsed);
        applyFlashTemplates(parsed);
        document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
        return true;
    }

    const dst = document.querySelector(defaultTarget);
    if (!dst) return false;
    const incoming = parsed.body.firstElementChild;
    if (!incoming) return false;
    dst.outerHTML = incoming.outerHTML;
    if (defaultTarget === '#reading-pane' && incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
        window.flash?.clear?.();
    }
    if (defaultTarget === '#reading-pane' && !skipHistory) syncEntryParamFromSwapUrl(url, { push: paneWasEmpty });
    applyClassTemplates(parsed);
    applyFlashTemplates(parsed);
    document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
    return true;
}

/**
 * The pane offline.js saved for `url`, or `null`. Any error means the same
 * thing: no saved copy.
 */
async function savedFragment(url) {
    try {
        return (await window.rdrsOffline?.fragment(url)) || null;
    } catch {
        return null;
    }
}

function entryIdFromSwapUrl(url) {
    const m = (url || '').match(/\/entries\/(\d+)(?:\/|$|\?)/);
    return m ? m[1] : null;
}

// Mirror the entry into `?entry={id}` so refresh/share/back reproduce the pane.
function syncEntryParamFromSwapUrl(swapUrl, options) {
    const id = entryIdFromSwapUrl(swapUrl);
    if (!id) return;
    setEntryParam(id, options);
}

function writeEntryParam(entryId, push) {
    const u = new URL(window.location.href);
    if (entryId == null) u.searchParams.delete('entry');
    else u.searchParams.set('entry', String(entryId));
    if (push) window.history.pushState({}, '', u);
    else window.history.replaceState({}, '', u);
}

let pendingEntryParam;
let entryParamFrame = 0;

/// Mirror the open entry into `?entry=`. Replaces are coalesced per frame
/// (`replaceState` is costly while holding `j`); pushes stay synchronous to keep
/// history in order.
function setEntryParam(entryId, options) {
    if (options?.push) {
        if (entryParamFrame) {
            cancelAnimationFrame(entryParamFrame);
            entryParamFrame = 0;
            pendingEntryParam = undefined;
        }
        writeEntryParam(entryId, true);
        return;
    }
    pendingEntryParam = entryId;
    if (entryParamFrame) return;
    entryParamFrame = requestAnimationFrame(() => {
        entryParamFrame = 0;
        const id = pendingEntryParam;
        pendingEntryParam = undefined;
        writeEntryParam(id, false);
    });
}

// replaceState, never push: typing refines a filter, not history.
function syncScopedSearchParam(form) {
    const input = form.querySelector('input[name="q"]');
    if (!input) return;
    const u = new URL(window.location.href);
    const q = input.value.trim();
    if (q) u.searchParams.set('q', q);
    else u.searchParams.delete('q');
    window.history.replaceState({}, '', u);
}

// The entry id currently mounted in the reading pane, or null when it is empty.
function currentPaneEntryId() {
    const pane = document.getElementById('reading-pane');
    if (!pane || pane.classList.contains('reading-pane-empty')) return null;
    // The pane's stamped id is the fast path; the form scan is a fallback for
    // other templates.
    const stamped = pane.getAttribute('data-entry-id');
    if (stamped) return stamped;
    const form = pane.querySelector('form[action*="/entries/"]');
    const m = form?.action.match(/\/entries\/(\d+)\//);
    return m ? m[1] : null;
}

// Sync the pane to the URL on back/forward. One slot is pushed per list visit,
// so back closes the pane and forward re-opens it.
window.addEventListener('popstate', () => {
    // Up front: performSwap's mismatch clear misses the close-pane branch.
    window.flash?.clear?.();
    // Sidebar swaps push their own slots; paths outside the entries family must
    // reload or a stale list sits under the new URL.
    if (window.location.pathname !== renderedListPath) {
        const href = window.location.pathname + window.location.search;
        const swappable = categoryIdFromHref(href) || feedIdFromHref(href);
        if (swappable && document.querySelector('[data-list-pane]')) {
            swapListPane(href, { skipHistory: true, restoreEntry: true });
        } else {
            window.location.reload();
        }
        return;
    }
    const u = new URL(window.location.href);
    const entryId = u.searchParams.get('entry');
    if (!entryId) {
        closeReadingPane();
        return;
    }
    if (currentPaneEntryId() === entryId) return;
    performSwap(`/entries/${entryId}/fragment`, { method: 'GET' }, '#reading-pane', { skipHistory: true });
});

// Reset `#reading-pane` to empty and drop `.reading-pane-active`, or mobile
// traps the reader on a blank overlay. False if already empty.
function closeReadingPane() {
    const pane = document.getElementById('reading-pane');
    if (!pane || pane.classList.contains('reading-pane-empty')) return false;
    pane.classList.remove('reading-pane-active');
    pane.classList.add('reading-pane-empty');
    pane.innerHTML = '<p>Select an entry to read.</p>';
    setEntryParam(null);
    return true;
}

// ── Sidebar navigation (in-place list-pane swap) ─────────────────────
// `?pane=1` swaps the left column and empties the pane without reloading, which
// would reset `.sidebar-nav` scroll. Unswappable links navigate normally.
const CATEGORY_PATH_RE = /^\/categories\/(\d+)\/entries\/?$/;
const FEED_PATH_RE = /^\/feeds\/(\d+)\/entries\/?$/;

function categoryIdFromHref(href) {
    const path = new URL(href, window.location.origin).pathname;
    const m = path.match(CATEGORY_PATH_RE);
    return m ? m[1] : null;
}

function feedIdFromHref(href) {
    const path = new URL(href, window.location.origin).pathname;
    const m = path.match(FEED_PATH_RE);
    return m ? m[1] : null;
}

// popstate compares against this to tell a `?entry=` toggle from a real change.
let renderedListPath = window.location.pathname;

/// Swap the list pane to `href`; `restoreEntry` re-opens its `?entry=`.
async function swapListPane(href, options = {}) {
    const catId = categoryIdFromHref(href);
    const feedId = feedIdFromHref(href);
    if ((!catId && !feedId) || !document.querySelector('[data-list-pane]')) {
        window.location.href = href;
        return;
    }
    const target = new URL(href, window.location.origin);
    const fetchUrl = new URL(target);
    fetchUrl.searchParams.set('pane', '1');
    fetchUrl.searchParams.delete('entry');
    // Same reasoning as the entry-switch path.
    cancelPaneImages(document.getElementById('reading-pane'));
    window.flash?.clear?.();
    const applied = await performSwap(
        fetchUrl.toString(),
        { method: 'GET' },
        '[data-list-pane]',
        { fallbackUrl: href }, // `?pane=1` answers with markup, not a page.
    );
    if (!applied) return;
    if (!options.skipHistory) window.history.pushState({}, '', target);
    renderedListPath = target.pathname;
    const sb = document.querySelector('rdrs-sidebar');
    // Match server-rendered category/feed pages: no top-level item stays lit.
    sb?.setAttribute('active', '');
    if (feedId) {
        sb?.setAttribute('active-feed-id', feedId);
        // The caller's hint, else the loaded feed lists.
        const parent = options.categoryId || sb?.categoryIdOfFeed?.(feedId);
        if (parent) sb.setAttribute('active-category-id', String(parent));
    } else {
        sb?.setAttribute('active-category-id', catId);
        sb?.removeAttribute('active-feed-id');
    }
    sb?.closeDrawer?.();
    // On mobile the document scrolls and would keep the old offset.
    window.scrollTo({ top: 0 });
    const entryId = options.restoreEntry ? target.searchParams.get('entry') : null;
    if (entryId) {
        performSwap(`/entries/${entryId}/fragment`, { method: 'GET' }, '#reading-pane',
            { skipHistory: true });
    }
}

/// One handler for every in-page link to a category or feed list.
const LIST_NAV_LINKS = [
    '#sidebar-categories a[data-category-id]',
    '#sidebar-categories a[data-feed-id]',
    '[data-entry-row] a.entry-feed',
    '.breadcrumb a',
].join(', ');

function installListNav() {
    document.addEventListener('click', (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey ||
            event.shiftKey || event.altKey) return;
        const link = event.target.closest(LIST_NAV_LINKS);
        if (!link) return;
        const href = link.getAttribute('href');
        if (!href) return;
        if (!categoryIdFromHref(href) && !feedIdFromHref(href)) return;
        if (!document.querySelector('[data-list-pane]')) return;
        event.preventDefault();
        // An entry row knows its category; otherwise swapListPane resolves it.
        const row = link.closest('[data-entry-row]');
        swapListPane(href, { categoryId: row?.dataset.categoryId });
    });
}
installListNav();

/// Rows `[` / `]` / `{` / `}` walk, in on-screen order: categories with the open
/// category's feeds spliced in.
function sidebarNavTargets() {
    const sb = document.querySelector('rdrs-sidebar');
    const cats = sb?.categories || [];
    const activeCatId = sb?.activeCategoryId || 0;
    const feeds = sb?.activeFeeds || [];
    const targets = [];
    for (const cat of cats) {
        targets.push({
            kind: 'category',
            id: cat.id,
            unread: cat.unread_count,
            href: `/categories/${cat.id}/entries`,
        });
        if (cat.id === activeCatId) {
            for (const feed of feeds) {
                targets.push({
                    kind: 'feed',
                    id: feed.id,
                    unread: feed.unread_count,
                    href: `/feeds/${feed.id}/entries`,
                    categoryId: cat.id,
                });
            }
        }
    }
    return targets;
}

// Mobile back button; `.reading-pane-back` is hidden above 1024px.
document.addEventListener('click', (event) => {
    if (event.button !== 0) return;
    if (!event.target.closest('[data-pane-back]')) return;
    event.preventDefault();
    closeReadingPane();
});

// ── Reading-pane prev/next ("neighbors") navigation ──────────────────
// Buttons render disabled; `GET /api/entries/{id}/neighbors` resolves them
// (crossing unloaded pages). "Previous" = newer, "Next" = older, as `k`/`j`.
let neighborState = { entryId: null, prevId: null, nextId: null };

// The page's filter as `NeighborsQuery` params, mirroring handlers/pages.rs.
function currentEntryFilterParams() {
    const { pathname, search } = window.location;
    const out = new URLSearchParams();
    const status = new URLSearchParams(search).get('status');
    const applyStatus = (s) => {
        // Feed/category default view is Unread when no status is present.
        if (s === 'read') out.set('read_only', 'true');
        else if (s === 'starred') out.set('starred_only', 'true');
        else if (s === 'all') { /* no flag */ }
        else out.set('unread_only', 'true');
    };
    const feed = pathname.match(/^\/feeds\/(\d+)\/entries/);
    const cat = pathname.match(/^\/categories\/(\d+)\/entries/);
    if (pathname === '/') out.set('unread_only', 'true');
    else if (pathname === '/entries') { /* All — no flag */ }
    else if (pathname === '/entries/read') out.set('read_only', 'true');
    else if (pathname === '/entries/starred') out.set('starred_only', 'true');
    else if (pathname === '/entries/summarized') out.set('has_summary', 'true');
    else if (feed) { out.set('feed_id', feed[1]); applyStatus(status); }
    else if (cat) { out.set('category_id', cat[1]); applyStatus(status); }
    // Echo `data-snapshot-at` as `read_after` so entries read during this view
    // stay navigable.
    if (out.get('unread_only') === 'true') {
        const snapshotAt = document
            .querySelector('[data-entries-list]')
            ?.getAttribute('data-snapshot-at');
        if (snapshotAt) out.set('read_after', snapshotAt);
    }
    return out.toString();
}

// Applied only if still for the open entry; stale results leave buttons disabled.
function applyNeighborButtons() {
    // Scoped to the pane to avoid walking every entry row.
    const pane = document.getElementById('reading-pane');
    const prevBtn = pane?.querySelector('[data-pane-prev]');
    const nextBtn = pane?.querySelector('[data-pane-next]');
    const open = currentPaneEntryId();
    const valid = open != null && neighborState.entryId === open;
    if (prevBtn) prevBtn.disabled = !(valid && neighborState.prevId != null);
    if (nextBtn) nextBtn.disabled = !(valid && neighborState.nextId != null);
}

// Resolve prev/next from the DOM to skip a round trip, or null to use the
// server. Interior rows only (ends can't prove `null`), and not under a scoped
// search, since the server ignores `q` here.
function neighborsFromLoadedList(entryId) {
    if (document.querySelector('[data-entries-search] input[name="q"]')?.value) return null;
    const rows = document.querySelectorAll('[data-entries-list] [data-entry-row]');
    const wanted = String(entryId);
    for (let i = 1; i < rows.length - 1; i++) {
        if (rows[i].getAttribute('data-entry-id') !== wanted) continue;
        return {
            prevId: Number(rows[i - 1].getAttribute('data-entry-id')),
            nextId: Number(rows[i + 1].getAttribute('data-entry-id')),
        };
    }
    return null;
}

async function resolveNeighbors(entryId) {
    const local = neighborsFromLoadedList(entryId);
    if (local) {
        neighborState = { entryId, prevId: local.prevId, nextId: local.nextId };
        applyNeighborButtons();
        return;
    }
    const params = currentEntryFilterParams();
    const url = `/api/entries/${entryId}/neighbors${params ? `?${params}` : ''}`;
    try {
        const resp = await fetch(url, { credentials: 'same-origin' });
        if (!resp.ok) return;
        const data = await resp.json();
        neighborState = { entryId, prevId: data.prev_id, nextId: data.next_id };
        applyNeighborButtons();
    } catch {}
}

// Disabled up front so a slow fetch never leaves a stale direction.
let lastResolvedPaneId = null;
function maybeResolveNeighbors() {
    const id = currentPaneEntryId();
    if (id === lastResolvedPaneId) {
        // An action swap re-renders the buttons disabled; re-apply, or mobile
        // prev/next dies (disabled buttons swallow taps).
        applyNeighborButtons();
        return;
    }
    lastResolvedPaneId = id;
    if (id == null) {
        neighborState = { entryId: null, prevId: null, nextId: null };
        applyNeighborButtons();
        return;
    }
    applyNeighborButtons();
    resolveNeighbors(id);
}

// Submit Load More once per cursor so the list catches up with the pane.
// Keyed on the cursor value (plus action), not an in-flight flag, so holding `j`
// no-ops; one page per call.
let requestedLoadMoreKey = null;
function loadMoreOnce() {
    const form = document.getElementById('load-more');
    const cursor = form?.querySelector('input[name="after"]')?.value;
    if (!cursor) return;
    const key = `${form.action}|${cursor}`;
    if (key === requestedLoadMoreKey) return;
    requestedLoadMoreKey = key;
    form.requestSubmit();
}

// Open the neighbor: click the loaded row (keeps selection in sync), else swap
// the fragment directly.
function navigateNeighbor(direction) {
    const open = currentPaneEntryId();
    if (open == null) return;
    if (neighborState.entryId !== open) {
        resolveNeighbors(open).then(() => doNavigateNeighbor(direction));
        return;
    }
    doNavigateNeighbor(direction);
}

function doNavigateNeighbor(direction) {
    const id = direction === 'next' ? neighborState.nextId : neighborState.prevId;
    if (id == null) return;
    const link = document.querySelector(
        `[data-entry-row][data-entry-id="${id}"] a[data-swap="#reading-pane"]`
    );
    if (link) { link.click(); return; }
    performSwap(`/entries/${id}/fragment`, { method: 'GET' }, '#reading-pane');
    // Safe alongside the pane swap: different nodes, and only pane GETs are guarded.
    loadMoreOnce();
}

function installNeighborNav() {
    document.addEventListener('click', (event) => {
        if (event.button !== 0) return;
        if (event.target.closest('[data-pane-prev]')) {
            event.preventDefault();
            navigateNeighbor('prev');
        } else if (event.target.closest('[data-pane-next]')) {
            event.preventDefault();
            navigateNeighbor('next');
        }
    });
    document.addEventListener('rdrs:swap-complete', maybeResolveNeighbors);
    // Resolve once on load so a `?entry=` deep-link gets live buttons too.
    maybeResolveNeighbors();
}
installNeighborNav();

// Turn `<template data-flash>` blocks into toasts, for actions with no DOM change.
function applyFlashTemplates(parsed) {
    const flashes = parsed.querySelectorAll('template[data-flash]');
    for (const tpl of flashes) {
        const level = tpl.getAttribute('data-level') || 'info';
        // `<template>` children live in `.content`; `tpl.textContent` is ''.
        const message = (tpl.content?.textContent || '').trim();
        if (!message) continue;
        if (window.flash && typeof window.flash.show === 'function') {
            window.flash.show(level, message);
        }
    }
}

// Class directives from swap responses (e.g. row `entry-read`). add/remove, not
// overwrite, to keep client-only classes like `.selected`.
function applyClassTemplates(parsed) {
    for (const tpl of parsed.querySelectorAll('template[data-class-target]')) {
        const dst = document.querySelector(tpl.getAttribute('data-class-target'));
        if (!dst) continue;
        const add = tpl.getAttribute('data-class-add');
        const remove = tpl.getAttribute('data-class-remove');
        if (add) dst.classList.add(...add.split(/\s+/).filter(Boolean));
        if (remove) dst.classList.remove(...remove.split(/\s+/).filter(Boolean));
    }
}

// The mobile drawer is owned entirely by <rdrs-sidebar>.

installSwap();

// Live updates over SSE: `sidebar` refetches /api/sidebar, `summary` updates the
// badge and pane. Each reconnect resyncs the sidebar.
const SUMMARY_ICON_FILLED =
    '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></svg>';
const SUMMARY_ICON_OUTLINE =
    '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><g transform="translate(1.2 1.2) scale(0.9)"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></g></svg>';
// status -> [badge class, title, filled?]; null clears the badge.
const SUMMARY_BADGE = {
    completed:  ['summary-badge', 'Has Summary', true],
    pending:    ['summary-badge-pending', 'Pending', false],
    processing: ['summary-badge-processing', 'Processing', false],
    failed:     ['summary-badge-failed', 'Failed', true],
};
const BADGE_SELECTOR =
    '.summary-badge, .summary-badge-pending, .summary-badge-processing, .summary-badge-failed';

function renderSummaryBadge(row, status) {
    const existing = row.querySelector(BADGE_SELECTOR);
    if (!status || !SUMMARY_BADGE[status]) { existing?.remove(); return; }
    const [cls, title, filled] = SUMMARY_BADGE[status];
    const svg = filled ? SUMMARY_ICON_FILLED : SUMMARY_ICON_OUTLINE;
    if (existing) {
        existing.className = cls;
        existing.title = title;
        existing.innerHTML = svg;
        return;
    }
    // Before <time>, matching the SSR badge order.
    const span = document.createElement('span');
    span.className = cls;
    span.title = title;
    span.setAttribute('aria-hidden', 'true');
    span.innerHTML = svg;
    const statusCluster = row.querySelector('.entry-status');
    const time = statusCluster?.querySelector('.entry-time');
    if (statusCluster && time) statusCluster.insertBefore(span, time);
    else statusCluster?.appendChild(span);
}

// Announce that sidebar state may have changed; <rdrs-sidebar> refetches.
function refreshSidebar() {
    document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
}

function onSummaryEvent(data) {
    const { entry_id, status } = data;
    const row = document.querySelector(`[data-entry-row][data-entry-id="${entry_id}"]`);
    if (row) renderSummaryBadge(row, status);
    if (String(currentPaneEntryId()) === String(entry_id)) {
        performSwap(`/entries/${entry_id}/summary/fragment`, { method: 'GET' }, '#rp-summary-container');
    }
}

function installSse() {
    // Only on the logged-in surface (the sidebar element is the marker).
    if (!document.querySelector('rdrs-sidebar')) return;
    let es;
    try {
        es = new EventSource('/events', { withCredentials: true });
    } catch {
        return; // EventSource unavailable — no live updates, page still works.
    }
    // Skip the first `open`: the sidebar already fetched. Later ones are reconnects.
    let sseHasConnected = false;
    es.addEventListener('open', () => {
        if (sseHasConnected) refreshSidebar();
        sseHasConnected = true;
    });
    es.addEventListener('sidebar', () => refreshSidebar());
    es.addEventListener('summary', (e) => {
        try { onSummaryEvent(JSON.parse(e.data)); } catch {}
    });
    // EventSource auto-reconnects on transient errors; nothing to do here.
}
installSse();

// Refetch on every swap: slightly over-fetches, but cheap and simpler than an allowlist.
document.addEventListener('rdrs:swap-complete', () => {
    refreshSidebar();
});

// Add a local-time `title` to every `<time datetime>` (server emits UTC).
// `data-local-text` also replaces the text; UTC stays as the no-JS fallback.
function applyTimeTooltips(root) {
    const scope = root || document;
    for (const el of scope.querySelectorAll('time[datetime]')) {
        const iso = el.getAttribute('datetime');
        if (!iso) continue;
        // Skip already-formatted timestamps, since this runs after every swap.
        // Not `data-localized`: rdrs-flash.js owns that marker.
        if (el.getAttribute('data-tooltip-at') === iso) continue;
        const d = new Date(iso);
        if (isNaN(d.getTime())) continue;
        const local = d.toLocaleString();
        el.title = local;
        if (el.hasAttribute('data-local-text')) {
            el.textContent = local;
        }
        el.setAttribute('data-tooltip-at', iso);
    }
}
applyTimeTooltips();
initPaneImages();
document.addEventListener('rdrs:swap-complete', () => applyTimeTooltips());
document.addEventListener('rdrs:swap-complete', () => initPaneImages());

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
    { group: 'Other', key: '/', desc: 'Open the search box (scoped search on feed / category pages)' },
    { group: 'Other', key: '?', desc: 'Toggle this help' },
];

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
        if (link) swapListPane(link.getAttribute('href'), { categoryId: row?.dataset.categoryId });
        return;
    }
    // 'c': the selected entry's category, else the page's parent category.
    const rowCatId = row?.dataset.categoryId;
    if (rowCatId) { swapListPane(`/categories/${rowCatId}/entries`); return; }
    if (!window.location.pathname.startsWith('/feeds/')) return;
    const sb = document.querySelector('rdrs-sidebar');
    const catId = sb && sb.getAttribute('active-category-id');
    if (catId) swapListPane(`/categories/${catId}/entries`);
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
installGoNavigation();

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
        else help.show(KB_SHORTCUTS);
    });
}
installHelpKeyboard();

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
        const paneId = currentPaneEntryId();
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
                if (currentPaneEntryId() != null) navigateNeighbor('next');
                else move(1);
                break;
            case 'k':
                e.preventDefault();
                if (currentPaneEntryId() != null) navigateNeighbor('prev');
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
                performSwap(url, { method: 'POST' }, `#entry-row-${id}`)
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
                markLoadedEntriesAsRead(btn);
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
                if (summaryInFlight()) { e.preventDefault(); break; }
                if (dismissVisibleSummary()) { e.preventDefault(); break; }
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
                const targets = sidebarNavTargets();
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
                swapListPane(target.href, { categoryId: target.categoryId });
                break;
            }
            case 'Escape': {
                // The help overlay owns Esc while open, in its own shadow root.
                const help = document.querySelector('rdrs-kb-help');
                if (help && help.isVisible) return;
                if (closeReadingPane()) e.preventDefault();
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
installEntriesKeyboard();

// Shared by the Summarize button and 'a': dismiss a showing summary via its
// Dismiss control. False when none is showing.
function dismissVisibleSummary() {
    const dismiss = document.querySelector('#reading-pane [data-summary-dismiss]');
    if (!dismiss) return false;
    dismiss.click();
    return true;
}

// True mid-generation, when only Cancel should act.
function summaryInFlight() {
    return !!document.querySelector('#reading-pane [data-summary-pending]');
}

// Keep the toggle's label/icon/aria-label in sync; skipped mid-request so the
// "Summarizing…" label survives.
function syncSummarizeToggleLabel() {
    const form = document.querySelector('#reading-pane [data-summary-toggle]');
    if (!form || form.getAttribute('aria-busy') === 'true') return;
    const btn = form.querySelector('button');
    if (!btn) return;
    const showing = !!document.querySelector('#reading-pane [data-summary-dismiss]');
    // Matches the server render; handler gates cover the gap.
    btn.disabled = summaryInFlight();
    const labelEl = btn.querySelector('.action-label');
    if (labelEl) labelEl.textContent = showing ? 'Dismiss' : 'Summarize';
    btn.setAttribute('aria-label', showing ? 'Dismiss summary' : 'Summarize');
    const summarizeIcon = btn.querySelector('.action-icon-summarize');
    const dismissIcon = btn.querySelector('.action-icon-dismiss');
    if (summarizeIcon) summarizeIcon.hidden = showing;
    if (dismissIcon) dismissIcon.hidden = !showing;
}
document.addEventListener('rdrs:swap-complete', syncSummarizeToggleLabel);

// Copy to clipboard; Dismiss DELETEs the summary and strips it and the row badge.
function installSummaryActions() {
    document.addEventListener('click', async (e) => {
        const copyBtn = e.target.closest('[data-summary-copy]');
        if (copyBtn) {
            // Scoped to the box, so what is copied matches what is visible.
            const box = copyBtn.closest('.summary-box');
            if (!box) return;
            const summaryEl = box.querySelector('.rp-summary-content');
            if (!summaryEl) return;
            const title = (box.querySelector('[data-summary-title]')?.textContent || '').trim();
            const link = (box.querySelector('[data-summary-link]')?.getAttribute('href') || '').trim();
            const summary = summaryEl.textContent.trim();
            const parts = [];
            if (title) parts.push(title);
            if (link) parts.push(link);
            parts.push(summary);
            const text = parts.join('\n\n');
            try {
                await navigator.clipboard.writeText(text);
                // Not textContent, which would drop the icon span.
                const label = copyBtn.querySelector('.action-label') || copyBtn;
                const original = label.textContent;
                label.textContent = 'Copied!';
                setTimeout(() => { label.textContent = original; }, 2000);
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
            // Skip if the reader switched entries meanwhile (as in performSwap()).
            if (String(currentPaneEntryId()) === String(entryId)) {
                // Keep the wrapper: it is the target for a later summarize.
                const container = document.querySelector('[data-summary-container]');
                if (container) container.replaceChildren();
                syncSummarizeToggleLabel();
            }
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

// "Mark as Read..." posts to the GReader bulk-mark endpoint (optional `ts=`) and
// swaps the list in place; a form POST would land on a JSON response.
const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};
const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';

// Row count from `X-RDRS-Affected`, or null so callers use their own estimate.
function affectedCount(resp) {
    const raw = resp.headers.get('X-RDRS-Affected');
    if (raw === null) return null;
    const n = Number.parseInt(raw, 10);
    return Number.isNaN(n) ? null : n;
}

// Rebound after swaps (the header may be replaced); a guard prevents duplicates.
function installMarkAsReadDropdown() {
    const select = document.getElementById('mark-read-age');
    if (!select || select.dataset.markReadBound) return;
    select.dataset.markReadBound = '1';
    select.addEventListener('change', async () => {
        const age = select.value;
        select.selectedIndex = 0;
        if (!age) return;
        const ageLabel = AGE_LABELS[age] || age;
        if (!confirm(`Mark ${ageLabel} entries as read?`)) return;
        // GReader stream ID for the current page, e.g. `feed/<url>`.
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
            const n = affectedCount(resp);
            const scopeSuffix = age === 'all' ? '' : ` ${ageLabel}`;
            const message = n === null
                ? `Marked${scopeSuffix || ' all'} entries as read.`
                : `Marked ${n} ${n === 1 ? 'entry' : 'entries'}${scopeSuffix} as read.`;
            await finishBulkMarkRead(message);
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
document.addEventListener('rdrs:swap-complete', installMarkAsReadDropdown);

// Option values are URLs; keys 1-4 pick by position. Rebound after swaps.
function installStatusFilterSelect() {
    const select = document.getElementById('status-filter');
    if (!select || select.dataset.statusFilterBound) return;
    select.dataset.statusFilterBound = '1';
    select.addEventListener('change', () => {
        const url = select.value;
        if (url) window.location.href = url;
    });
}
installStatusFilterSelect();
document.addEventListener('rdrs:swap-complete', installStatusFilterSelect);

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
installSearchDrawer();

// The form sits outside the swapped list, so it keeps focus while typing;
// `installSwap()` performs the actual swap.
function installEntriesSearch() {
    const form = document.querySelector('form[data-entries-search]');
    if (!form || form.dataset.searchBound) return;
    form.dataset.searchBound = '1';
    const input = form.querySelector('input[name="q"]');
    if (!input) return;
    const submit = debounce(() => form.requestSubmit(), 250);
    input.addEventListener('input', submit);
}
installEntriesSearch();
document.addEventListener('rdrs:swap-complete', installEntriesSearch);

/// Re-render the current list in place from `?fragment=1` (page 1). Resolves
/// `false` if there is nothing to swap, so callers can reload instead.
async function refreshEntriesList() {
    if (!document.querySelector('[data-entries-list]')) return false;
    const url = new URL(window.location.href);
    url.searchParams.set('fragment', '1');
    // Drop `after` (append fragment) and `entry` (pane is left alone).
    url.searchParams.delete('after');
    url.searchParams.delete('entry');
    const applied = await performSwap(url.toString(), { method: 'GET' }, '[data-entries-list]',
        { fallbackUrl: window.location.href }); // `?fragment=1` is not a page.
    if (applied) scrollEntriesListToTop();
    return applied;
}

/// After a bulk mark-as-read: re-render the list and show `message`, or pass it
/// to the next document.
async function finishBulkMarkRead(message) {
    const refreshed = await refreshEntriesList();
    if (!refreshed) {
        // No swap possible: pass the message via the cookie.
        window.flash?.set('success', message);
        window.location.reload();
        return;
    }
    // Shown rather than `set()`: the page it belongs to is still up.
    window.flash?.success(message);
    document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
}

/// Scroll the list back to the top after a bulk mark-as-read (both the desktop
/// list scroller and the mobile document).
function scrollEntriesListToTop() {
    document.querySelector('[data-entries-list]')?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
}

// Mark every rendered entry read via GReader edit-tag. Separate from the button
// so `A` still works when scoped search hides it; `btn` only carries busy state.
async function markLoadedEntriesAsRead(btn) {
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
    if (btn) {
        btn.disabled = true;
        btn.setAttribute('aria-busy', 'true');
    }
    try {
        const resp = await fetch('/reader/api/0/edit-tag', {
            method: 'POST',
            body,
            credentials: 'same-origin',
        });
        if (!resp.ok) throw new Error('Failed to mark entries as read');
        // Excludes already-read rows, so usually smaller than posted.
        const n = affectedCount(resp) ?? ids.length;
        const message = `Marked ${n} ${n === 1 ? 'entry' : 'entries'} as read.`;
        await finishBulkMarkRead(message);
    } catch (err) {
        const message = err.message || 'Failed to mark entries as read';
        if (window.flash) { window.flash.error(message); } else { alert(message); }
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.removeAttribute('aria-busy');
        }
    }
}

function installMarkAboveButton() {
    const btn = document.getElementById('mark-above-read');
    if (!btn || btn.dataset.markAboveBound) return;
    // Inside the swapped container; the guard prevents duplicate listeners.
    btn.dataset.markAboveBound = '1';
    btn.addEventListener('click', () => markLoadedEntriesAsRead(btn));
}
installMarkAboveButton();
document.addEventListener('rdrs:swap-complete', installMarkAboveButton);

// Row click opens the entry via the title link, so `installSwap()` handles it.
function installRowClickToOpen() {
    document.addEventListener('click', (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey ||
            event.shiftKey || event.altKey) return;
        const row = event.target.closest('[data-entry-row]');
        if (!row) return;
        // Already handled: row action forms and the title link.
        if (event.target.closest('form')) return;
        if (event.target.closest('a[data-swap="#reading-pane"]')) return;
        // Any other link opens its own destination.
        if (event.target.closest('a')) return;
        const link = row.querySelector('a[data-swap="#reading-pane"]');
        if (!link) return;
        event.preventDefault();
        link.click();
    });
}
installRowClickToOpen();
