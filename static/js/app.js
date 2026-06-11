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

// Abort in-flight image downloads in the outgoing reading pane. The swap
// removes the old pane from the DOM, but browsers do NOT reliably cancel an
// `<img>`'s in-flight request when the element is detached — and image-proxy
// requests are slow (each re-fetches from origin). On HTTP/1.1 those stale
// downloads keep occupying the ~6 per-origin connection slots, so the next
// entry's fragment `fetch` stalls behind them (measured: hundreds of ms to
// >1s of pure connection-queue wait). Dropping `src` cancels them up front.
function cancelPaneImages(pane) {
    if (!pane) return;
    for (const img of pane.querySelectorAll('img[src]')) {
        img.removeAttribute('src');
    }
}

// Monotonic token + abort handle for reading-pane *navigation* fetches
// (GET /entries/{id}/fragment — entry clicks, Show Original, popstate
// restores, prev/next fallbacks). Clicking entry A then quickly entry B
// used to leave both responses in flight: whichever arrived LAST won the
// pane, so a slow stale A response could overwrite the just-opened B (and
// replaceState the URL back to ?entry=A). Each new navigation bumps the
// token and aborts the previous fetch; a response whose token is stale by
// the time it lands is discarded instead of applied. Action swaps (POST
// Save / Fetch-Full-Content) re-target the same entry and stay outside
// the guard. Same stale-fetch discipline as applyNeighborButtons().
let paneNavSeq = 0;
let paneNavAbort = null;

async function performSwap(url, init, defaultTarget, options) {
    const method = (init.method || 'GET').toUpperCase();
    // popstate-driven restores pass `skipHistory: true` because the browser
    // has already moved the address bar via back/forward — we must not push
    // or replace on top of the slot the user just navigated into.
    const skipHistory = options?.skipHistory === true;
    const isPaneNav = method === 'GET' && defaultTarget === '#reading-pane';
    let navSeq = null;
    if (isPaneNav) {
        navSeq = ++paneNavSeq;
        paneNavAbort?.abort();
        paneNavAbort = new AbortController();
        init.signal = paneNavAbort.signal;
    }
    // Before fetching the next entry, cancel the current pane's image loads so
    // they stop starving the connection pool — but only when navigating to a
    // *different* entry (action swaps like Save / Fetch-Full-Content re-target
    // the same entry and would just reload the same images).
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
        // A newer pane navigation aborted this fetch — drop it silently.
        // Falling through to `location.href` would hard-navigate the page
        // to a fragment URL the user has already moved past.
        if (isPaneNav && navSeq !== paneNavSeq) return;
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return;
        }
        window.location.href = url;
        return;
    }
    // Superseded while the headers were in flight (abort loses this race
    // when the reply was already buffered): discard before acting on it.
    if (isPaneNav && navSeq !== paneNavSeq) return;
    if (!response.ok) {
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return;
        }
        window.location.href = url;
        return;
    }
    let text;
    try {
        text = await response.text();
    } catch {
        // Aborted mid-body by a newer navigation (fetch itself had already
        // resolved) — same silent drop as above.
        if (isPaneNav && navSeq !== paneNavSeq) return;
        window.location.href = url;
        return;
    }
    if (isPaneNav && navSeq !== paneNavSeq) return;
    const parsed = new DOMParser().parseFromString(text, 'text/html');

    // Decide pushState vs replaceState BEFORE the DOM mutates: opening an
    // entry from the empty placeholder pushes a history entry (so a back /
    // mobile edge-swipe closes the pane back to the list); switching to a
    // different entry while the pane is already open replaces in place so
    // history doesn't accumulate per click.
    const paneBefore = document.getElementById('reading-pane');
    const paneWasEmpty = !!paneBefore?.classList.contains('reading-pane-empty');
    // Captured pre-mutation so the navigation-vs-action detection below
    // can compare against what was in the pane, not what we just swapped
    // in. A swap that lands a different entry id is treated as navigation
    // and clears any pre-existing flash banners; action-result swaps
    // (Save / Fetch-Full-Content) re-target the same entry and keep their
    // own `<template data-flash>` toast.
    const paneEntryIdBefore = currentPaneEntryId();
    const incomingEntryId = entryIdFromSwapUrl(url);

    let swappedReadingPane = false;
    const templates = parsed.querySelectorAll('template[data-swap-target]');
    if (templates.length > 0) {
        for (const tpl of templates) {
            const sel = tpl.getAttribute('data-swap-target');
            if (sel === '#reading-pane') swappedReadingPane = true;
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
        if (swappedReadingPane && incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
            window.flash?.clear?.();
        }
        if (swappedReadingPane && !skipHistory) syncEntryParamFromSwapUrl(url, { push: paneWasEmpty });
        applyFlashTemplates(parsed);
        document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
        return;
    }

    const dst = document.querySelector(defaultTarget);
    if (!dst) return;
    const incoming = parsed.body.firstElementChild;
    if (!incoming) return;
    dst.outerHTML = incoming.outerHTML;
    if (defaultTarget === '#reading-pane' && incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
        window.flash?.clear?.();
    }
    if (defaultTarget === '#reading-pane' && !skipHistory) syncEntryParamFromSwapUrl(url, { push: paneWasEmpty });
    applyFlashTemplates(parsed);
    document.dispatchEvent(new CustomEvent('rdrs:swap-complete'));
}

// Extract the entry id embedded in a swap URL like `/entries/123/fragment`
// or `/entries/123/save`. Returns null when the URL doesn't address an
// entry (e.g. `/sidebar/unread`, `/entries?after=…`).
function entryIdFromSwapUrl(url) {
    const m = (url || '').match(/\/entries\/(\d+)(?:\/|$|\?)/);
    return m ? m[1] : null;
}

// Mirror the entry id from a `#reading-pane` swap URL into the address-bar
// `?entry={id}` query so a refresh / share / browser-back reproduces the
// current pane state (the SSR list handlers consume `?entry=` via
// `maybe_build_reading_pane`). The caller passes `push: true` exactly
// when the pane was empty before the swap — that single push is what
// makes browser back / mobile edge-swipe-back close the pane instead of
// leaving the list entirely. Subsequent entry switches replace in place
// so history doesn't accumulate one slot per click.
function syncEntryParamFromSwapUrl(swapUrl, options) {
    const id = entryIdFromSwapUrl(swapUrl);
    if (!id) return;
    setEntryParam(id, options);
}

function setEntryParam(entryId, options) {
    const u = new URL(window.location.href);
    if (entryId == null) u.searchParams.delete('entry');
    else u.searchParams.set('entry', String(entryId));
    if (options?.push) window.history.pushState({}, '', u);
    else window.history.replaceState({}, '', u);
}

// Resolve the entry id currently mounted in the reading pane. The pane's
// outer element has no entry id of its own, but every inner action form
// targets `/entries/{id}/...`, so reading the first form's action is the
// reliable way to identify the loaded entry. Returns null when the pane
// is empty or has no form (e.g. an error-state render).
function currentPaneEntryId() {
    const pane = document.getElementById('reading-pane');
    if (!pane || pane.classList.contains('reading-pane-empty')) return null;
    const form = pane.querySelector('form[action*="/entries/"]');
    const m = form?.action.match(/\/entries\/(\d+)\//);
    return m ? m[1] : null;
}

// Back/forward navigation within the same document needs to sync the
// reading-pane to match the URL. We push exactly one history slot per
// list visit (the "first-open from empty pane" transition); back from
// that slot lands on a URL without `?entry=` and we close the pane,
// while forward back to that slot needs the pane re-mounted. Both ends
// of the toggle are handled here. Cross-document navigation (sidebar
// links, status-filter, 1-4 keys) reloads the page and SSR consumes
// `?entry=` server-side, so popstate doesn't fire for those.
window.addEventListener('popstate', () => {
    // Back/forward is a navigation in user terms — clear any toasts left
    // over from the previous view so they don't follow the user across
    // history slots. performSwap below would clear too on entry mismatch,
    // but doing it upfront also covers the close-pane branch.
    window.flash?.clear?.();
    const u = new URL(window.location.href);
    const entryId = u.searchParams.get('entry');
    if (!entryId) {
        closeReadingPane();
        return;
    }
    if (currentPaneEntryId() === entryId) return;
    // `skipHistory` keeps performSwap from pushing/replacing on top of the
    // slot the browser just moved into.
    performSwap(`/entries/${entryId}/fragment`, { method: 'GET' }, '#reading-pane', { skipHistory: true });
});

// Reset `#reading-pane` to its empty placeholder. Mirrors the SSR-rendered
// empty state in `_entries_layout.html` so the @media-driven mobile overlay
// dismisses (the `.reading-pane-active` class is what reveals the pane at
// ≤1024px — leaving it on top of empty content traps users on a blank
// screen). Returns false if the pane was already empty.
function closeReadingPane() {
    const pane = document.getElementById('reading-pane');
    if (!pane || pane.classList.contains('reading-pane-empty')) return false;
    pane.classList.remove('reading-pane-active');
    pane.classList.add('reading-pane-empty');
    pane.innerHTML = '<p>Select an entry to read.</p>';
    setEntryParam(null);
    return true;
}

// Mobile back button inside the reading pane. The button is rendered in
// `_reading_pane.html` but hidden on desktop via `.reading-pane-back`'s
// default `display: none` — the @media (≤1024px) block flips it to flex.
document.addEventListener('click', (event) => {
    if (event.button !== 0) return;
    if (!event.target.closest('[data-pane-back]')) return;
    event.preventDefault();
    closeReadingPane();
});

// ── Reading-pane prev/next ("neighbors") navigation ──────────────────
//
// The reading pane renders disabled `[data-pane-prev]` / `[data-pane-next]`
// buttons. After the pane opens we resolve the adjacent entry ids from
// `GET /api/entries/{id}/neighbors`, scoped to the *current list filter*
// (so "Next" inside the Unread inbox only walks unread entries, etc.),
// enable whichever direction has a neighbor, and remember the ids so a
// click / keypress is an instant swap with no extra round-trip. The
// endpoint resolves order from the DB, so prev/next also crosses
// pagination boundaries the in-memory list hasn't loaded yet.
//
// "Previous" = newer entry (up the published-desc list); "Next" = older
// (down) — the same axis as the list's `k`/`j`.
let neighborState = { entryId: null, prevId: null, nextId: null };

// Translate the current page's list filter into the query params
// `NeighborsQuery` accepts, mirroring the server-side filter each route
// builds (see handlers/pages.rs). Returns a query string without the
// leading `?` (empty for the unfiltered `/entries` "All" view).
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
    return out.toString();
}

// Reflect the resolved neighbor ids onto the buttons, but only while they
// still describe the entry currently in the pane (guards against a stale
// fetch landing after the user moved on). Anything else leaves both
// buttons disabled.
function applyNeighborButtons() {
    const prevBtn = document.querySelector('[data-pane-prev]');
    const nextBtn = document.querySelector('[data-pane-next]');
    const open = currentPaneEntryId();
    const valid = open != null && neighborState.entryId === open;
    if (prevBtn) prevBtn.disabled = !(valid && neighborState.prevId != null);
    if (nextBtn) nextBtn.disabled = !(valid && neighborState.nextId != null);
}

async function resolveNeighbors(entryId) {
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

// Re-resolve whenever the pane settles on a different entry. Disable the
// buttons up-front so a slow fetch never leaves a stale direction live.
let lastResolvedPaneId = null;
function maybeResolveNeighbors() {
    const id = currentPaneEntryId();
    if (id === lastResolvedPaneId) {
        // Same entry, but the pane DOM may have just been re-rendered by an
        // action swap that re-targets the same entry (Fetch Full Content /
        // Save) — which resets prev/next to their default `disabled` state.
        // neighborState is still valid here, so re-apply it; otherwise the
        // freshly-rendered buttons stay permanently disabled and, because a
        // disabled button swallows taps, mobile prev/next navigation dies for
        // good (desktop j/k bypasses the buttons via navigateNeighbor()).
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

// Open the neighbor in `direction` ('prev' | 'next'). Prefers clicking the
// matching list row's link when that entry is already loaded — that path
// also keeps the keyboard list selection in sync — and falls back to a
// direct fragment swap for neighbors beyond the loaded page. If the
// pane's neighbors haven't resolved yet, resolve then navigate.
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

// Decorate every <time datetime="..."> with a `title` attribute showing
// the same instant formatted to the browser's locale + timezone. The
// server emits UTC and only the client knows the user's TZ, so the
// tooltip has to happen in JS. Runs on initial load and after every
// swap so server-rendered fragments inserted later also pick it up.
//
// Elements that also carry `data-local-text` (currently the user-
// settings + admin "registered / logged in / created" cells, which
// display absolute times rather than a relative "3h ago") have their
// textContent replaced with the same local format — the server-rendered
// UTC string remains as a no-JS fallback.
function applyTimeTooltips(root) {
    const scope = root || document;
    for (const el of scope.querySelectorAll('time[datetime]')) {
        const iso = el.getAttribute('datetime');
        if (!iso) continue;
        const d = new Date(iso);
        if (isNaN(d.getTime())) continue;
        const local = d.toLocaleString();
        el.title = local;
        if (el.hasAttribute('data-local-text')) {
            el.textContent = local;
        }
    }
}
applyTimeTooltips();
document.addEventListener('rdrs:swap-complete', () => applyTimeTooltips());

// Single source of truth for the in-app shortcut help. Pages don't
// register additional entries — every shortcut the keyboard handler
// recognizes is listed here, grouped by where it applies.
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
    { group: 'Entry actions', key: 'a', desc: 'Summarize (Kagi)' },
    { group: 'Batch read', key: 'A', desc: 'Mark loaded entries as read (asks to confirm)' },
    { group: 'Go to', key: 'g u', desc: 'Unread inbox' },
    { group: 'Go to', key: 'g a', desc: 'All entries' },
    { group: 'Go to', key: 'g r', desc: 'Read' },
    { group: 'Go to', key: 'g s', desc: 'Starred' },
    { group: 'Go to', key: 'g m', desc: 'Summarized' },
    { group: 'Go to', key: 'g f', desc: 'Selected entry’s feed' },
    { group: 'Go to', key: 'g c', desc: 'Selected entry’s category (parent category on a feed page)' },
    { group: 'Feed / category pages', key: '1-4', desc: 'Status filter: All / Unread / Read / Starred' },
    { group: 'Feed / category pages', key: '[ / ]', desc: 'Previous / next category' },
    { group: 'Feed / category pages', key: '{ / }', desc: 'Previous / next category with unread' },
    { group: 'Other', key: '/', desc: 'Focus search (on the search page)' },
    { group: 'Other', key: '?', desc: 'Toggle this help' },
];

// ── "g" go-to sequences (miniflux-style two-key namespace) ───────────
// A first `g` arms the namespace; the second key picks the target. Page
// jumps (g u/a/r/s/m) work on every logged-in page; entry-relative jumps
// (g f / g c) need a selected list row. The pending state times out so a
// stray `g` doesn't swallow the next keystroke forever. The listener runs
// in the CAPTURE phase and stops propagation when consuming the second
// key, so `g s` can never double as the single-key Save shortcut.
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

function clearGoPending() {
    goPending = false;
    if (goTimer) { clearTimeout(goTimer); goTimer = null; }
}

function goToEntryRelative(key) {
    const row = document.querySelector('[data-entry-row].selected');
    if (key === 'f') {
        const link = row?.querySelector('.entry-item-meta a[href^="/feeds/"]');
        if (link) window.location.href = link.getAttribute('href');
        return;
    }
    // key === 'c' — prefer the selected entry's own category; fall back
    // to the page-parent category on /feeds/{id}/entries (the sidebar
    // exposes it as `active-category-id`).
    const fromRow = row?.querySelector('.entry-item-meta a[href^="/categories/"]');
    if (fromRow) { window.location.href = fromRow.getAttribute('href'); return; }
    if (!window.location.pathname.startsWith('/feeds/')) return;
    const sb = document.querySelector('rdrs-sidebar');
    const catId = sb && sb.getAttribute('active-category-id');
    if (catId) window.location.href = `/categories/${catId}/entries`;
}

function installGoNavigation() {
    document.addEventListener('keydown', (e) => {
        if (e.target.matches('input, textarea, select')) return;
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        if (goPending) {
            const key = e.key;
            clearGoPending();
            // Consume the second key unconditionally: a mistyped sequence
            // must not fire that key's unrelated single-key binding.
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
            goTimer = setTimeout(clearGoPending, GO_TIMEOUT_MS);
        }
    }, true);
}
installGoNavigation();

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
            case 'j':
                e.preventDefault();
                // With the reading pane open, j/k navigate it (open the
                // next/previous entry across the current filter, even past
                // the loaded page) instead of only moving the list cursor;
                // the list selection follows when the neighbor is loaded.
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
                // Toggle star on the active row. The row form's action is
                // state-dependent (`/star` or `/unstar`) — match either so
                // one binding flips the state.
                const current = activeRow();
                if (!current) return;
                const form = current.querySelector('form[action$="/star"], form[action$="/unstar"]');
                if (form) { e.preventDefault(); form.requestSubmit(); }
                break;
            }
            case 'm': {
                // Toggle read/unread on the active row (state-dependent
                // action, same pattern as `f`).
                const current = activeRow();
                if (!current) return;
                const form = current.querySelector('form[action$="/read"], form[action$="/unread"]');
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
            case 'A': {
                // Mark loaded rows as read — only fires on pages that
                // render the button (feed/category/inbox). Delegates to
                // the button's click handler so the confirm + fetch flow
                // lives in one place.
                const btn = document.getElementById('mark-above-read');
                if (!btn) return;
                e.preventDefault();
                btn.click();
                break;
            }
            case 'v': {
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
            case 'd': {
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
            case 's': {
                // Save (Linkding etc). Form is rendered only when the
                // user has a save target configured — absent = no-op.
                const form = paneForm('/save');
                if (!form) return;
                e.preventDefault();
                form.requestSubmit();
                break;
            }
            case 'a': {
                // Summarize via Kagi. Form is rendered only when Kagi is
                // configured (or a summary is in-flight, in which case
                // the button is disabled and paneForm() returns null).
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
                if (closeReadingPane()) e.preventDefault();
                break;
            }
            case ' ': {
                // Classic feed-reader convention: when an entry is loaded
                // in the reading pane, Space pages the article down (and
                // Shift+Space pages up). One key, one meaning — no
                // fallback action when the pane is empty.
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

// Reading-pane summary controls (Kagi Universal Summarizer output).
// Copy is a clipboard write; Dismiss DELETEs the cached summary and
// strips the summary block + the entry row's summary badge so the
// state matches what `/sidebar/unread` will report on the next mount.
function installSummaryActions() {
    document.addEventListener('click', async (e) => {
        const copyBtn = e.target.closest('[data-summary-copy]');
        if (copyBtn) {
            // Read from inside the summary box only, so what the user copies
            // matches the visible content of the box (title + link + summary).
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
