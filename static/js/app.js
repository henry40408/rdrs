// static/js/app.js — shared module for the logged-in surface.
//
// Ships: swap() partial-swap helper, sidebar polling, flash dismiss,
// theme controller, entries-family keyboard shortcuts, Mark-as-Read
// dropdown, Mark Above as Read, row-click-to-open delegation.

// The `?v=` cache-buster is substituted at serve time (see
// handlers/static_assets.rs). Without it this nested import resolves to a bare,
// unversioned URL that goes stale forever under the `immutable` cache header —
// an old cached utils.js missing an export silently breaks this whole module.
import { debounce } from './utils.js?v=__RDRS_ASSET_VERSION__';

/**
 * Intercept form / link interactions tagged with `data-swap="<selector>"`
 * and replace the matching element with HTML returned by the request.
 *
 * Response format:
 *   - HTML fragment: replaces the target element via outerHTML.
 *   - Multi-target: response containing one or more
 *     `<template data-swap-target="<selector>">…</template>` blocks.
 *     Each template's content replaces its target via outerHTML.
 *   - Class directive: `<template data-class-target="<selector>"
 *     data-class-add|data-class-remove="a b"></template>` toggles classes on
 *     an element that is *not* being replaced. Lets a response update a
 *     container's state class while swapping only its sub-elements.
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
        event.preventDefault();
        // The action-bar Summarize button mirrors the 'a' shortcut. Only the
        // action-bar form is tagged `data-summary-toggle`; the error-state
        // Retry form is not, so Retry still regenerates.
        if (form.hasAttribute('data-summary-toggle')) {
            // In-flight: inert — Cancel lives in the summary box.
            if (summaryInFlight()) return;
            // Completed: dismiss instead of regenerating.
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
        setFormBusy(form, { cancellable: !!controller });
        try {
            await performSwap(url, init, target);
            // Mirror the scoped-search box into the address bar so a
            // refresh / share reproduces the filtered list — and, crucially,
            // clearing the box removes the stale `?q=` instead of leaving it
            // behind. The form lives outside the swapped list container, so
            // it's still mounted here. `fragment=1` and other hidden inputs
            // stay out of the visible URL.
            if (form.matches('[data-entries-search]')) {
                syncScopedSearchParam(form);
            }
        } finally {
            // On success the form has been replaced by the swap so the
            // call below is a no-op on the detached node. On failure
            // (POST error → flash) the original form is still mounted
            // and gets its button restored.
            formSwapAborts.delete(form);
            clearFormBusy(form);
        }
    });
}

const formSwapAborts = new WeakMap();

// Entry ids with a keyboard-driven read/unread toggle ('m') POST in flight.
// Guards against rapid double-press double-POSTing stale state on the same row.
const pendingRowToggles = new Set();

function abortFormSwap(form) {
    const controller = formSwapAborts.get(form);
    if (!controller) return;
    controller.abort();
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
        // Update only the `.action-label` span when present so the sibling
        // `.action-icon` SVG survives. Writing `btn.textContent` would replace
        // every child node (icon included) — and the icon-over-label buttons
        // whose swap target is a *sibling* (e.g. Summarize → #rp-summary-container)
        // are not re-rendered by the swap, so the wiped icon never comes back.
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

// Abort in-flight image downloads in the outgoing reading pane. The swap
// removes the old pane from the DOM, but browsers do NOT reliably cancel an
// `<img>`'s in-flight request when the element is detached — and image-proxy
// requests are slow (each re-fetches from origin). On HTTP/1.1 those stale
// downloads keep occupying the ~6 per-origin connection slots, so the next
// entry's fragment `fetch` stalls behind them (measured: hundreds of ms to
// >1s of pure connection-queue wait). Dropping `src` cancels them up front.
//
// Scoped to `.reading-pane-article` — the slow image-proxied *content* images.
// The meta-row favicon (`/api/feeds/{id}/icon`) is small, local and cached, so
// cancelling it buys nothing but blanks a still-visible pane while the next
// fragment loads (a visible favicon flash on every entry switch).
function cancelPaneImages(pane) {
    if (!pane) return;
    for (const img of pane.querySelectorAll('.reading-pane-article img[src]')) {
        img.removeAttribute('src');
    }
}

// Reading-pane content images: flip the server-set data-img-state="loading"
// skeleton to "loaded" on load, or replace the image with a dashed-box
// fallback on error. Idempotent per image via data-img-init.
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
    // textContent — never innerHTML — so alt text can't inject markup.
    cap.textContent = alt ? `Image unavailable — ${alt}` : 'Image unavailable';
    box.appendChild(cap);
    // Preserve reserved height for dimensioned images.
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
        // Already settled (e.g. cached) before we attached handlers.
        if (img.complete) {
            if (img.naturalWidth > 0) img.setAttribute('data-img-state', 'loaded');
            else markBrokenImage(img);
            continue;
        }
        img.addEventListener('load', () => img.setAttribute('data-img-state', 'loaded'), { once: true });
        img.addEventListener('error', () => {
            // cancelPaneImages() drops `src` to abort in-flight downloads when
            // navigating away; that also fires `error` on the outgoing pane's
            // images. A dropped `src` means cancellation, not a load failure —
            // skip it so we don't build a throwaway broken-box (and flash it)
            // on a pane that's about to be replaced.
            if (!img.getAttribute('src')) return;
            markBrokenImage(img);
        }, { once: true });
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

// Swap targets that live inside the reading pane, i.e. that only make sense
// for the entry currently open. See the staleness check in performSwap().
const PANE_REGION_TARGETS = new Set(['#reading-pane', '#rp-summary-container']);

/// The markup the server last delivered for each swap target.
///
/// When the next response for that target is byte-identical, the DOM already
/// shows it and replacing the node is pure churn — a layout and a repaint of
/// everything under it, which on WebKit is where images blink. The path that
/// hits this constantly is clicking the sidebar feed that is *already* open: it
/// re-fetches and rebuilds the entire list pane from markup identical to what
/// it replaces.
///
/// Comparing the server's answer against its own previous answer, rather than
/// against the DOM, is what makes this reliable: the live DOM is covered in
/// things the server never sent — `.selected` from `j`/`k`, `data-…-bound`
/// listener markers, `title` attributes a client-side localizer adds to `<time>`
/// — and every one of them made a DOM-to-response comparison differ.
///
/// Morph targets are exempt, because for them that same reasoning is unsound:
/// the DOM under `[data-entries-list]` and `#entry-row-…` is edited by swaps
/// answering for *other* targets — a row action, a Load-More append, a
/// `[data-list-pane]` feed switch — none of which touch this map, so two equal
/// answers no longer imply the DOM still matches them. Marking one feed read,
/// switching feeds and marking that one read is the case that broke: both
/// answers are the same empty list, so the second swap was skipped and the rows
/// just marked stayed on screen unread until a reload, under a flash saying
/// they had been marked. Morphing an identical tree writes no attribute and
/// moves no node, so the skip was buying those targets nothing to begin with.
///
/// A target is only known after the first swap that fills it, so a pane that
/// arrived with the document still gets replaced once before this can settle.
const lastServerMarkup = new Map();

/// The single element a swap template carries, or null when it carries anything
/// else (Load More returns N rows plus a form). The list-pane template indents
/// its include, so the whitespace the parser hands back is ignored.
function soleSwapElement(tpl) {
    const nodes = Array.from(tpl.content.childNodes)
        .filter((n) => n.nodeType !== Node.TEXT_NODE || n.textContent.trim() !== '');
    if (nodes.length !== 1 || nodes[0].nodeType !== Node.ELEMENT_NODE) return null;
    return nodes[0];
}

/// Swap targets whose subtree is morphed into shape rather than replaced.
///
/// Both are pure entry-row markup. `[data-entries-list]` is the row container
/// that "Mark Above as Read" and the Mark-as-Read dropdown re-render: the rows
/// survive, they only gain `entry-read`, yet replacing the container rebuilt
/// every one of them and every favicon inside — measured at none of six images
/// preserved. `#entry-row-…` targets are the marker form a row action returns.
///
/// The rest stay on replacement, and not only out of caution. `[data-list-pane]`
/// carries the filter bar's form controls, whose live value is state the markup
/// does not describe, and a scroller whose offset is supposed to reset when the
/// reader switches view. `#reading-pane` resets its scroll on purpose too. And a
/// category switch replaces the rows with a different feed's anyway, so morphing
/// would preserve almost nothing.
function isMorphTarget(selector) {
    return selector === '[data-entries-list]' || selector.startsWith('#entry-row-');
}

/// Attributes the client writes onto server-rendered markup as its own
/// bookkeeping, which a morph must leave alone.
///
/// `data-…-bound` is the load-bearing one: it marks a control whose listeners
/// are installed, so stripping it from an element that *survives* the morph
/// invites the installer to bind a second copy to the same node — one click,
/// two POSTs. `data-img-…` tracks a content image's load state, and `title` on
/// a `<time>` is the tooltip `applyTimeTooltips()` writes.
const CLIENT_OWNED_ATTR = /^(data-.+-bound|data-img-.+|data-localized|data-tooltip-at|title)$/;

/// Classes the client owns for the same reason: `.selected` is the `j`/`k`
/// cursor, which the server has never heard of.
const CLIENT_OWNED_CLASSES = ['selected'];

/// Bring `from`'s attributes in line with `to`, keeping what the client owns.
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

/// Whether two nodes are similar enough to morph one into the other rather than
/// swapping one for the other.
function morphCompatible(from, to) {
    if (!from || from.nodeType !== to.nodeType) return false;
    if (from.nodeType !== Node.ELEMENT_NODE) return true;
    if (from.tagName !== to.tagName) return false;
    // An id is a key; two differently-keyed elements are different elements
    // even when they look alike.
    return (from.id || '') === (to.id || '');
}

/// Reshape `from`'s children into `to`'s, reusing the nodes already there.
///
/// Elements carrying an `id` are matched by it — entry rows have one, so a list
/// that lost a row in the middle keeps every surviving row's node instead of
/// shifting everything up by one. Everything else matches positionally.
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
        // Never consume a keyed node positionally: it may be the match for an
        // incoming node further down the list.
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

/// Morph one node into another, in place.
function morphNode(from, to) {
    if (from.nodeType !== Node.ELEMENT_NODE) {
        if (from.nodeValue !== to.nodeValue) from.nodeValue = to.nodeValue;
        return;
    }
    morphAttributes(from, to);
    morphChildren(from, to);
}

/// Morph the live `dst` into the shape of `incoming`, leaving every node that
/// survives — an `<img>` above all — exactly where it was. A rebuilt `<img>` has
/// no bitmap yet and a re-inserted one sends WebKit back through its load and
/// decode path, so the icons blink; a morphed one is never touched at all.
function morphSwap(dst, incoming) {
    if (!morphCompatible(dst, incoming)) return false;
    morphNode(dst, incoming);
    return true;
}

/// Attributes the server re-stamps on every render that change nothing the
/// reader can see. `data-snapshot-at` is the render-time boundary the neighbor
/// API echoes back as `read_after`; it moves every second, so leaving it in the
/// comparison would make two responses for the same view never equal and the
/// skip above could never fire.
const VOLATILE_SERVER_ATTRS = ['data-snapshot-at'];

/// `el`'s markup with those attributes removed, for comparison only.
function comparableServerMarkup(el) {
    const clone = el.cloneNode(true);
    for (const name of VOLATILE_SERVER_ATTRS) {
        for (const n of clone.querySelectorAll(`[${name}]`)) n.removeAttribute(name);
        clone.removeAttribute(name);
    }
    return clone.outerHTML;
}

/// Copy those attributes from the response onto the DOM that is being kept.
/// Skipping a swap must not freeze the snapshot boundary at whatever the reader
/// first loaded — `j`/`k` would then treat a widening set of entries as unread.
/// The two trees are identical apart from these attributes, so the elements
/// carrying them line up one for one.
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

/// Fetch `url` and apply the response to `defaultTarget` (or to whatever
/// `<template data-swap-target>` blocks it carries). Resolves `true` when a
/// swap was actually applied, `false` when the call bailed out (superseded,
/// aborted, or handed off to a full navigation) — callers that follow a swap
/// with side effects (history, sidebar state) must not run them on a bail-out.
///
/// `options.fallbackUrl` is where the non-2xx / network-error path navigates
/// instead of `url`. Fragment-only URLs need it: `?pane=1` returns bare
/// `<template>` markup, so hard-navigating to the fetched URL would leave the
/// user on a blank page rather than the real one.
async function performSwap(url, init, defaultTarget, options) {
    const method = (init.method || 'GET').toUpperCase();
    const fallbackUrl = options?.fallbackUrl || url;
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
        if (init.signal?.aborted) return false;
        // A newer pane navigation aborted this fetch — drop it silently.
        // Falling through to `location.href` would hard-navigate the page
        // to a fragment URL the user has already moved past.
        if (isPaneNav && navSeq !== paneNavSeq) return false;
        if (method !== 'GET' && window.flash) {
            window.flash.error('Action failed — please try again.');
            return false;
        }
        window.location.href = fallbackUrl;
        return false;
    }
    // Superseded while the headers were in flight (abort loses this race
    // when the reply was already buffered): discard before acting on it.
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
        // Aborted mid-body by a newer navigation (fetch itself had already
        // resolved) — same silent drop as above.
        if (isPaneNav && navSeq !== paneNavSeq) return false;
        window.location.href = fallbackUrl;
        return false;
    }
    if (isPaneNav && navSeq !== paneNavSeq) return false;
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

    // An *action* response belongs to the entry it was fired on, so applying
    // it once the reader has moved to another entry would paint one entry's
    // summary (or article) into another's pane. The window is small but real:
    // an SSE `summary` event for the outgoing entry can pass its
    // `currentPaneEntryId()` pre-check and still land after the switch, and
    // Save / Fetch-Full-Content have the same shape. Re-check here, against
    // the DOM as it is now rather than as it was when the fetch started.
    //
    // Pane *navigation* is exempt: landing a different entry is its whole
    // purpose, and `paneNavSeq` above already discards superseded ones.
    // Row-scoped targets (`#entry-row-N`) are exempt too — a row action is
    // valid whatever the pane is showing.
    if (!isPaneNav && PANE_REGION_TARGETS.has(defaultTarget) &&
        incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
        // The action itself did happen server-side, so let its toast through;
        // only the markup is stale.
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
            // The reading pane is exempt from both paths below: replacing that
            // node also resets its scroll offset, so keeping it would quietly
            // change what re-opening the open entry does to a half-read article.
            const sole = sel === '#reading-pane' ? null : soleSwapElement(tpl);
            if (sole && isMorphTarget(sel)) {
                // Entry-row markup is morphed into place instead of replaced,
                // so the rows and favicons that survive the change keep their
                // nodes. See `isMorphTarget`.
                if (morphSwap(dst, sole)) continue;
            } else if (sole) {
                // Unchanged since the last time the server answered for this
                // target? Then the DOM already shows it — see `lastServerMarkup`.
                const markup = comparableServerMarkup(sole);
                if (lastServerMarkup.get(sel) === markup) {
                    syncVolatileAttrs(sole, dst);
                    continue;
                }
                lastServerMarkup.set(sel, markup);
            }
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

// Extract the entry id embedded in a swap URL like `/entries/123/fragment`
// or `/entries/123/save`. Returns null when the URL doesn't address an
// entry (e.g. `/entries?after=…`).
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

function writeEntryParam(entryId, push) {
    const u = new URL(window.location.href);
    if (entryId == null) u.searchParams.delete('entry');
    else u.searchParams.set('entry', String(entryId));
    if (push) window.history.pushState({}, '', u);
    else window.history.replaceState({}, '', u);
}

// A pending replace-mode write, coalesced to one per frame.
let pendingEntryParam;
let entryParamFrame = 0;

/// Mirror the open entry into `?entry=`.
///
/// The replace-mode write is deferred to the next frame: `history.replaceState`
/// is one of the more expensive things on the swap's synchronous path, and
/// holding `j` down issues one per keypress only for the last to matter. The
/// address bar still settles on the right entry within a frame, and a refresh
/// or share reproduces the pane exactly as before.
///
/// Pushes are not deferred — that single push is what makes browser-back close
/// the pane, and it must land in the same task as the navigation that caused it
/// or the history slot lands out of order.
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

// Mirror the scoped-search box's `q` into the address bar. Sets it when the
// box has text, deletes it when the box is empty so clearing the search truly
// resets the URL. replaceState (never push) — typing is a filter refinement,
// not a distinct history entry.
function syncScopedSearchParam(form) {
    const input = form.querySelector('input[name="q"]');
    if (!input) return;
    const u = new URL(window.location.href);
    const q = input.value.trim();
    if (q) u.searchParams.set('q', q);
    else u.searchParams.delete('q');
    window.history.replaceState({}, '', u);
}

// Resolve the entry id currently mounted in the reading pane. The pane's
// outer element has no entry id of its own, but every inner action form
// targets `/entries/{id}/...`, so reading the first form's action is the
// reliable way to identify the loaded entry. Returns null when the pane
// is empty or has no form (e.g. an error-state render).
function currentPaneEntryId() {
    const pane = document.getElementById('reading-pane');
    if (!pane || pane.classList.contains('reading-pane-empty')) return null;
    // `_reading_pane.html` stamps the id on the pane element itself. Reading an
    // attribute costs nothing; the form scan below is a substring-match
    // selector over the whole article subtree, and this is called several times
    // per swap. Kept as a fallback so a pane rendered by any other template
    // (error states, fragments that predate the attribute) still resolves.
    const stamped = pane.getAttribute('data-entry-id');
    if (stamped) return stamped;
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
    // Sidebar navigation is an in-place swap that pushes its own history
    // slot, so back/forward can now land on a different *path* within the
    // same document. Re-render the list for it — and for a destination we
    // can't swap (the inbox, /entries*, anything outside the entries family),
    // reload so the user gets the real page instead of a stale list under a
    // new URL.
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

// ── Sidebar navigation (in-place list-pane swap) ─────────────────────
//
// Sidebar category and feed links, the `[` / `]` / `{` / `}` shortcuts and
// `g c` / `g f` land here instead of hard-navigating. The server's `?pane=1`
// response carries the whole left column plus an emptied reading pane, so one
// swap:
//   * leaves the sidebar untouched — a document reload resets `.sidebar-nav`'s
//     internal scroll to the top, which is the jump this exists to avoid (same
//     story for the document scroll on mobile);
//   * closes the open entry, which belonged to the list being left.
// Anything we can't swap (no list pane on the page, a modified click) falls
// back to a normal navigation.
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

// Path the list pane currently renders. popstate compares against it to tell
// a same-page `?entry=` toggle from a real category change.
let renderedListPath = window.location.pathname;

/// Swap the list pane over to `href` — a `/categories/{id}/entries` or
/// `/feeds/{id}/entries` URL. `skipHistory` is for popstate restores (the
/// browser already moved the address bar); `restoreEntry` re-opens the
/// `?entry=` the restored URL names, since the fragment always ships an empty
/// pane.
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
    // Same reasoning as the entry-switch path: the outgoing pane's proxied
    // images keep occupying connection slots long after the pane is detached.
    cancelPaneImages(document.getElementById('reading-pane'));
    window.flash?.clear?.();
    const applied = await performSwap(
        fetchUrl.toString(),
        { method: 'GET' },
        '[data-list-pane]',
        // Never fall back to the `?pane=1` URL: it answers with bare
        // `<template>` markup, which is not a page.
        { fallbackUrl: href },
    );
    if (!applied) return;
    if (!options.skipHistory) window.history.pushState({}, '', target);
    renderedListPath = target.pathname;
    const sb = document.querySelector('rdrs-sidebar');
    // Category and feed pages both server-render `active=""`; mirror that so no
    // top-level nav item stays lit next to the highlighted row. A feed keeps
    // its parent category active (and therefore expanded), which is where the
    // feed link the reader just clicked lives.
    sb?.setAttribute('active', '');
    if (feedId) {
        sb?.setAttribute('active-feed-id', feedId);
        // `options.categoryId` is the caller's hint (an entry row carries its
        // own category); otherwise fall back to the loaded feed lists, which
        // cover the case that matters — a feed clicked in the sidebar.
        const parent = options.categoryId || sb?.categoryIdOfFeed?.(feedId);
        if (parent) sb.setAttribute('active-category-id', String(parent));
    } else {
        sb?.setAttribute('active-category-id', catId);
        sb?.removeAttribute('active-feed-id');
    }
    sb?.closeDrawer?.();
    // Desktop panes scroll internally and the freshly inserted list starts at
    // its top; on mobile the document is the scroller and would otherwise keep
    // the previous category's offset.
    window.scrollTo({ top: 0 });
    const entryId = options.restoreEntry ? target.searchParams.get('entry') : null;
    if (entryId) {
        performSwap(`/entries/${entryId}/fragment`, { method: 'GET' }, '#reading-pane',
            { skipHistory: true });
    }
}

/// Every in-page anchor that lands on a category or feed list. One handler
/// rather than one per surface: they all want the same swap, and the surfaces
/// kept being discovered one bug report at a time (the entry row's feed name,
/// then the breadcrumb's category).
///
///   * sidebar rows — the case the swap was built for;
///   * the feed name in an entry row, which `g f` already swapped to;
///   * the breadcrumb, whose middle crumb on a feed page is that feed's
///     category. Its outer crumbs (`/categories`, `/feeds`) are ordinary pages
///     and fail the swappable-href test below, so they navigate as before.
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
        // Not a list URL (`/categories`, `/feeds`, …), or no list pane to swap
        // into (e.g. this row rendered on a page without one): plain navigation.
        if (!categoryIdFromHref(href) && !feedIdFromHref(href)) return;
        if (!document.querySelector('[data-list-pane]')) return;
        event.preventDefault();
        // Same hint `g f` passes: an entry row knows its own category, which is
        // what keeps the sidebar expanded on the group the feed belongs to.
        // Absent (sidebar, breadcrumb), swapListPane resolves it itself.
        const row = link.closest('[data-entry-row]');
        swapListPane(href, { categoryId: row?.dataset.categoryId });
    });
}
installListNav();

/// The sidebar rows `[` / `]` / `{` / `}` walk, in the order they appear on
/// screen: every category, with the open category's feeds spliced in right
/// after it. Feeds are only listed for the open category, so the flat list
/// grows and shrinks as the reader moves — which is the point: the shortcuts
/// step through exactly what is visible.
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
// (on unread views the filter is widened by the page's render-time
// snapshot, so entries read during this page view stay reachable),
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
    // Unread views get snapshot semantics: the server stamps the page with
    // data-snapshot-at at render time, and echoing it back as read_after
    // makes entries read *during* this page view (read_at >= snapshot) still
    // count as unread for neighbor navigation — so j/k and Prev/Next can
    // return to the entry the reader just finished. Entries read before the
    // page loaded stay skipped, matching what the list rendered.
    if (out.get('unread_only') === 'true') {
        const snapshotAt = document
            .querySelector('[data-entries-list]')
            ?.getAttribute('data-snapshot-at');
        if (snapshotAt) out.set('read_after', snapshotAt);
    }
    return out.toString();
}

// Reflect the resolved neighbor ids onto the buttons, but only while they
// still describe the entry currently in the pane (guards against a stale
// fetch landing after the user moved on). Anything else leaves both
// buttons disabled.
function applyNeighborButtons() {
    // Scoped to the pane, not the document. Both buttons live inside
    // `#reading-pane`, which sits *after* the list in document order, so a
    // document-wide attribute selector walks every rendered entry row before
    // reaching them — and this runs on every swap and every neighbor resolve.
    const pane = document.getElementById('reading-pane');
    const prevBtn = pane?.querySelector('[data-pane-prev]');
    const nextBtn = pane?.querySelector('[data-pane-next]');
    const open = currentPaneEntryId();
    const valid = open != null && neighborState.entryId === open;
    if (prevBtn) prevBtn.disabled = !(valid && neighborState.prevId != null);
    if (nextBtn) nextBtn.disabled = !(valid && neighborState.nextId != null);
}

// Answer prev/next from the list already in the DOM, for an entry whose row
// has a loaded row on *both* sides. Returns null when it can't, and the
// caller falls back to the server.
//
// The rows under `[data-entries-list]` are flat siblings rendered in the same
// order, under the same filter, that `find_neighbors` resolves server-side,
// and nothing removes one once rendered — marking an entry read only restyles
// its row, and on unread views the page's `read_after` snapshot keeps the
// server counting it too. So an interior row's DOM neighbours are its real
// neighbours, and the ~7-query round trip per entry opened is pure latency.
//
// Interior rows only, deliberately. The first row is the head of the list *as
// rendered*, and the last row is never the end of the set — Load More may
// still have pages to append. Neither end can prove a `null`, so both fall
// through rather than guess one.
//
// Skipped entirely while a scoped search is active: `currentEntryFilterParams`
// does not forward `q` (and `NeighborsQuery` has no field for it), so the
// server resolves neighbours across the *unsearched* set. Answering those from
// a searched DOM would quietly change which entry j/k lands on.
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

// Submit the Load-More form once for its current cursor, so the list can
// catch up with a pane that navigated past the loaded page.
//
// Guarding on the cursor *value* rather than an in-flight flag is what makes
// repeat calls safe: a successful append replaces the form with a fresh one
// carrying the next cursor, so holding `j` down re-enters here with the same
// cursor until the response lands and is a no-op — no duplicate rows. A flag
// cleared on `rdrs:swap-complete` could not do this; the pane fragment swap
// fired alongside us usually completes first and would clear it too early.
//
// One page per call, deliberately. Stepping through entries lands exactly one
// past the loaded page, so one append is always enough; a far-away `?entry=`
// deep-link stays out of reach rather than firing a burst of requests, which
// is no worse than before this existed.
//
// The key carries the form action as well, so navigating to another list whose
// next page happens to start at the same cursor still auto-loads.
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
    // The neighbor is past the loaded page, so nothing in the list points at
    // what the reader is now on. Pull the next page in behind the pane swap:
    // the two target different nodes and only `#reading-pane` GETs go through
    // the pane-nav abort guard, so they don't cancel each other.
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

// Apply `<template data-class-target="<selector>" data-class-add|remove="a b">`
// directives from a swap response. This exists so an action response can update
// a *container's* state class without shipping the whole container back: the
// entry-row actions re-render only the marker/star forms, but the row itself
// still has to gain or lose `entry-read`.
//
// Deliberately add/remove rather than setting `class` wholesale — the list
// keeps client-only classes on the same element (`.selected` from j/k
// navigation), and a full overwrite would silently drop them.
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

// The mobile drawer (hamburger, close button, tap-outside-to-close) lives
// entirely inside <rdrs-sidebar> — it owns the markup, so it owns the
// behaviour and the listener lifecycle.

installSwap();

// Live updates over a single SSE stream (replaces the old 20s sidebar poll).
// `sidebar` → refetch /api/sidebar (notify-and-fetch). `summary` → update the
// row badge from the event's status and, if the entry is open, swap the
// reading pane's summary container. EventSource reconnects natively; on
// (re)connect we resync the sidebar to catch anything missed while offline.
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
    // Insert before the <time> element so badge ordering matches the SSR row.
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

// Announce that sidebar-backed state (unread counts, categories) may have
// moved. <rdrs-sidebar> subscribes while connected and refetches /api/sidebar;
// pages without a sidebar simply have no listener.
function refreshSidebar() {
    document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
}

function onSummaryEvent(data) {
    const { entry_id, status } = data;
    const row = document.querySelector(`[data-entry-row][data-entry-id="${entry_id}"]`);
    if (row) renderSummaryBadge(row, status);
    // If the affected entry is the one open in the reading pane, swap its
    // summary container to reflect the new state (replaces "refresh to see").
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
    // `open` fires on the first connect too, where <rdrs-sidebar> has already
    // fetched from its own connectedCallback — refreshing there is pure
    // duplication. Every later `open` is a reconnect, and those do need a
    // resync to pick up whatever changed while the stream was down.
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

// After every successful partial swap (mark-read, mark-unread, mark-all,
// batch read, OPML import surface, etc.), ask <rdrs-sidebar> to refetch.
// `rdrs:swap-complete` fires from performSwap() on both single- and
// multi-target swaps. Refreshing on every swap over-fetches slightly —
// star/save/fetch-full-content don't change sidebar state — but the
// server-side per-user sidebar cache means each /api/sidebar call costs
// roughly nothing on a hit, so a single broad hook beats a fragile
// per-action allowlist.
document.addEventListener('rdrs:swap-complete', () => {
    refreshSidebar();
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
        // The tooltip is a pure function of `datetime`, and this runs after
        // *every* swap over the whole document — a list paged to 500 rows would
        // otherwise re-format 500 instants per keypress. `data-tooltip-at`
        // records the instant already rendered; the server owns `datetime`, so
        // a row whose timestamp really changed still re-formats. The attribute
        // is in CLIENT_OWNED_ATTR, so a morph preserves it alongside `title`.
        //
        // Deliberately *not* `data-localized`: that name belongs to
        // rdrs-flash.js, which uses it as a valueless "already rewritten"
        // marker and selects on `:not([data-localized])`.
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

// Which-key style hint shown at the bottom-right while the `g` namespace
// is pending. Lifecycle is tied to goPending: shown when `g` arms it,
// removed when the sequence completes, is cancelled, or times out.
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
        // The row knows its own category, which is what keeps the sidebar
        // expanded on the group the feed actually belongs to.
        if (link) swapListPane(link.getAttribute('href'), { categoryId: row?.dataset.categoryId });
        return;
    }
    // key === 'c' — prefer the selected entry's own category (carried as
    // data-category-id since the visible category link was removed from the
    // row); fall back to the page-parent category on /feeds/{id}/entries (the
    // sidebar exposes it as `active-category-id`).
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
            showGoHint();
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
    // The node behind `activeId`, when we still hold a live one. Rows are
    // morphed rather than replaced (see `isMorphTarget`), so the node the
    // reader is sitting on normally survives every swap and re-resolving it by
    // attribute selector — several times per keypress, against a list that can
    // be hundreds of rows long — is pure repeat work. Validated on every read,
    // so a row that *was* replaced falls back to the query.
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
    // Multi-target swaps (open entry, toggle star, toggle read) replace
    // the active row's DOM node — the server-rendered replacement has no
    // way to carry over the client-side `.selected` class. Re-apply it
    // after every swap so the list highlights stay aligned with `activeId`.
    //
    // The open pane is also the authority on *which* entry that is. A
    // neighbor past the loaded page swaps only `#reading-pane` — there is no
    // row to click, so neither `focusRow` nor the click listener below runs —
    // and without adopting its id here `activeId` would stay on the last
    // loaded row: that row keeps `.selected` while the reader is elsewhere,
    // and closing the pane sends `j`/`k` back to it. On the click path this
    // is a no-op (`focusRow` already stored the same id). The row is allowed
    // to be missing: Load More appends it moments later and that swap runs
    // this handler again, highlighting it then.
    document.addEventListener('rdrs:swap-complete', () => {
        const paneId = currentPaneEntryId();
        if (paneId != null && paneId !== activeId) {
            // Clear the outgoing row *before* reassigning, or its `.selected`
            // is orphaned and the list shows two highlights.
            activeRow()?.classList.remove('selected');
            activeId = paneId;
        }
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
                // Toggle read/unread on the active row. The Wire Room redesign
                // removed the row's read form, so drive the swap machinery
                // directly with a POST (identical outcome to the old inline
                // form: the row fragment is swapped in) — no throwaway <form>
                // appended to the row. An in-flight guard keeps a rapid
                // double-press from reading stale state and double-POSTing.
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
                // Mark loaded rows as read — only on pages that offer it
                // (feed / category / inbox). The button is also hidden while a
                // scoped search is active, since "Mark N matching as Read"
                // owns that case visually; the shortcut still works there, so
                // detect the search box rather than the button alone.
                const btn = document.getElementById('mark-above-read');
                const searching = !!document.querySelector('[data-entries-search] input[name="q"]')?.value;
                if (!btn && !searching) return;
                e.preventDefault();
                markLoadedEntriesAsRead(btn);
                break;
            }
            case 'v': {
                // Open Original in a new tab. The visible row link was removed
                // in the Wire Room redesign; the external URL now rides on the
                // row's `data-entry-link` (absent when the entry has no link,
                // so absence = no-op).
                const current = activeRow();
                const url = current?.getAttribute('data-entry-link');
                if (!url) return;
                e.preventDefault();
                // Open a real tab with full noopener+noreferrer, matching the
                // removed row-link exactly. `window.open(url, '_blank', <features>)`
                // drops noreferrer and the non-empty features string forces a
                // popup window instead of a tab — a temporary anchor click avoids
                // both regressions.
                const a = document.createElement('a');
                a.href = url;
                a.target = '_blank';
                a.rel = 'noopener noreferrer';
                a.click();
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
                // Summarize via Kagi — or, when a summary is already on
                // screen, dismiss it. Same toggle the action-bar Summarize
                // button performs; dismissVisibleSummary() is the shared path.
                const pane = document.getElementById('reading-pane');
                if (!pane || pane.classList.contains('reading-pane-empty')) return;
                // In-flight: inert (Cancel lives in the summary box). Swallow
                // the key so requestSubmit() can't bypass the disabled button.
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
                // Sidebar navigation over the rows as displayed: categories,
                // with the open category's feeds spliced in after it. Starting
                // point: the current feed on /feeds/{id}/entries, the current
                // category on /categories/{id}/entries; on every other list
                // page (inbox, /entries*) `]`/`}` enter at the first row and
                // `[`/`{` at the last. Wrapping stays inside this list — it
                // never cycles back out to Unread/All.
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
                // Virtual start index when nothing here is current: forward
                // starts just before the first row, backward just after the
                // last, so the first probe lands on targets[0] / the last one.
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

// Shared by the action-bar Summarize button and the 'a' shortcut: when a
// summary is already on screen, its Dismiss control is mounted inside the
// .summary-box (only the completed state renders it). Click it — reusing the
// DELETE + clear flow in installSummaryActions — and report that we handled
// the toggle. Returns false when no summary is showing, so callers fall
// through to kicking off summarization.
function dismissVisibleSummary() {
    const dismiss = document.querySelector('#reading-pane [data-summary-dismiss]');
    if (!dismiss) return false;
    dismiss.click();
    return true;
}

// True when the reading pane's summary is mid-generation (pending/processing).
// The in-flight box carries `data-summary-pending` (and no Dismiss control);
// its Cancel button is the only intended action, so the Summarize/Dismiss
// toggle stays inert while it's showing.
function summaryInFlight() {
    return !!document.querySelector('#reading-pane [data-summary-pending]');
}

// Keep the action-bar toggle button's presentation in sync with whether a
// completed summary is on screen. The button flips behavior in the submit
// handler (see dismissVisibleSummary), so its label / icon / aria-label must
// follow: "Dismiss" (close icon) when a summary box is mounted, "Summarize"
// (sparkle) otherwise. Skipped while the form is mid-request so it doesn't
// clobber the transient "Summarizing…" busy label — the SSE completion swap
// fires another sync once busy clears. Every add/remove path (initial server
// render, SSE swap, summarize form swap, dismiss) reaches here: swaps via the
// `rdrs:swap-complete` event below, dismiss via a direct call in its handler.
function syncSummarizeToggleLabel() {
    const form = document.querySelector('#reading-pane [data-summary-toggle]');
    if (!form || form.getAttribute('aria-busy') === 'true') return;
    const btn = form.querySelector('button');
    if (!btn) return;
    const showing = !!document.querySelector('#reading-pane [data-summary-dismiss]');
    // Keep the button disabled while a summary is generating so it visually
    // matches the server render and can't be clicked; the handler gates are
    // the safety net for the brief window before this sync catches up.
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

// Reading-pane summary controls (Kagi Universal Summarizer output).
// Copy is a clipboard write; Dismiss DELETEs the cached summary and
// strips the summary block + the entry row's summary badge.
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
                // Only swap the label span's text — writing to the button's
                // textContent would clobber the icon span too, dropping the
                // copy glyph until the container re-renders.
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
            // Only touch the pane if it still shows the entry we dismissed:
            // the reader can switch entries while the DELETE is in flight,
            // and clearing the container then would blank the *new* entry's
            // summary. Same staleness rule performSwap() applies to
            // `#rp-summary-container` swaps.
            if (String(currentPaneEntryId()) === String(entryId)) {
                // Clear the inner `.summary-box` but leave the wrapper in
                // place — the swap target for a later summarize click is
                // `#rp-summary-container`, so the wrapper has to stay.
                const container = document.querySelector('[data-summary-container]');
                if (container) container.replaceChildren();
                // The summary box (and its Dismiss control) is gone; flip the
                // action-bar toggle button back to "Summarize".
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

// "Mark as Read..." dropdown on /unread + /entries pages. Posts to the
// GReader bulk-mark endpoint with an optional `ts=` cutoff in
// microseconds, then swaps the refreshed list in place — same treatment
// as "Mark Above as Read", and for the same reason: a `location.reload()`
// would throw away the open entry, the sidebar's loaded feed lists and
// both scroll positions to redraw a list that only lost some rows. The
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

// Bulk writes keep the GReader-standard `OK` body and carry the number of rows
// they actually changed in `X-RDRS-Affected`. Returns null when the header is
// missing or unparseable so callers can fall back to their own estimate rather
// than reporting "Marked null entries as read."
function affectedCount(resp) {
    const raw = resp.headers.get('X-RDRS-Affected');
    if (raw === null) return null;
    const n = Number.parseInt(raw, 10);
    return Number.isNaN(n) ? null : n;
}

// Bound per element (not delegated) and re-run after every swap: the category
// swap replaces the whole list-pane header, so the listener-bearing <select>
// is discarded along with it. The guard keeps swaps that leave the header in
// place from stacking a second listener.
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
            const n = affectedCount(resp);
            const scopeSuffix = age === 'all' ? '' : ` ${ageLabel}`;
            const message = n === null
                ? `Marked${scopeSuffix || ' all'} entries as read.`
                : `Marked ${n} ${n === 1 ? 'entry' : 'entries'}${scopeSuffix} as read.`;
            const refreshed = await refreshEntriesList();
            if (!refreshed) {
                // No list pane to swap into, or the swap bailed: hand the
                // message to the next document via the cookie and reload.
                window.flash?.set('success', message);
                window.location.reload();
                return;
            }
            // Shown rather than `set()`: the page the message belongs to is
            // the one still on screen.
            window.flash?.success(message);
            // The rows are gone from the list, but the unread badges beside
            // it are still counting them.
            document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
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
document.addEventListener('rdrs:swap-complete', installMarkAsReadDropdown);

// Status-filter <select> on feed + category pages. Each option's value
// is the URL to navigate to; the active option is pre-selected by the
// server. The 1-4 keys hit the same options by position. Re-bound after
// swaps for the same reason as the Mark-as-Read dropdown above.
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
//
// The search box lives in a drawer above `.list-pane-header`, opened by the
// magnifier chip in the filter bar. The server renders it open when the
// request carried `?q=`, so deep links and list swaps arrive in the right
// state; everything below only handles the interactive transitions.
//
// Closing clears the search. A collapsed box that is still filtering turns a
// short list into a mystery ("where did my entries go?"), so close resets the
// query, which the debounced submit path mirrors back out of the URL.
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
    // Only re-submit when there was something to clear: an empty box means the
    // list is already unfiltered, and a needless swap would drop the reader's
    // scroll position in the list.
    if (input && input.value !== '') {
        input.value = '';
        form?.requestSubmit();
    }
    // Focus would otherwise stay on a control inside a collapsed, zero-height
    // container, which strands the keyboard user.
    toggle?.focus();
}

// Delegated on the document, and therefore installed exactly once: the toggle
// lives in the filter bar and the close button inside the drawer, and a
// list-pane swap replaces both — per-element binding would have to re-run on
// every swap and would stack duplicate document listeners if it did.
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
        // `/` opens the drawer from anywhere on a list page — the same key
        // /search binds (see static/js/search.js), now that these pages have a
        // search box to focus.
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

// Debounced auto-submit for the scoped-search box. The `<form
// data-entries-search>` lives in the search drawer, outside the swapped
// `[data-entries-list]` container, so it survives every swap and keeps
// input focus/caret while typing — only the installer's binding needs to
// be re-applied when the list is swapped out from under it for other
// reasons (e.g. status-filter changes re-render the whole layout).
// `installSwap()`'s submit handler does the actual GET → query-string →
// swap; this only triggers the debounced `requestSubmit()`.
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

/// Re-render the current list in place from the server's `?fragment=1`
/// response (page 1 of whatever the URL already asks for — status tab, scoped
/// search and all). The alternative for a bulk mark-as-read used to be
/// `location.reload()`, which throws away the open entry, the sidebar's loaded
/// feed lists and both scroll positions to redraw a list that only lost some
/// rows.
///
/// Resolves `false` when there is no list to swap (or the swap bailed out), so
/// callers can fall back to a reload rather than leave stale rows on screen.
async function refreshEntriesList() {
    if (!document.querySelector('[data-entries-list]')) return false;
    const url = new URL(window.location.href);
    url.searchParams.set('fragment', '1');
    // `after` would make the server answer with the Load-More *append*
    // fragment; `entry` only feeds the SSR reading pane, which this response
    // deliberately leaves alone.
    url.searchParams.delete('after');
    url.searchParams.delete('entry');
    const applied = await performSwap(url.toString(), { method: 'GET' }, '[data-entries-list]',
        // The fetched URL answers with bare `<template>` markup, so a failure
        // must land the user on the real page instead.
        { fallbackUrl: window.location.href });
    if (applied) scrollEntriesListToTop();
    return applied;
}

/// Send the list scroller back to the first row.
///
/// A bulk mark-as-read answers with page 1 again, so the rows the reader had
/// scrolled past are gone (on an unread list) or now read — the offset the
/// scroller kept points at unrelated rows, and after "Mark Above as Read" it
/// points past the end of a list that just lost everything above it. Both
/// mark-as-read paths want the reader looking at what is left, from the top.
///
/// Desktop scrolls `[data-entries-list]` (`.list-pane-body`) internally; on
/// mobile the pane is full-height and the document is the scroller, so both are
/// reset — the same pair `swapListPane()` handles.
function scrollEntriesListToTop() {
    document.querySelector('[data-entries-list]')?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
}

// "Mark Above as Read" button on feed + category pages. Sits at the
// bottom of the list (below Load More) and marks every entry currently
// rendered in the DOM — loaded rows + anything appended via Load More.
// Entries that haven't been loaded yet stay untouched. Posts to the
// GReader edit-tag endpoint with one `i=<id>` per visible row and
// `a=user/-/state/com.google/read`.
//
// Split from the button so the `A` shortcut can still reach it while a scoped
// search hides the button (see the `A` case in installEntriesKeyboard) — the
// same treatment `m` gets for the row read-toggle whose visible form is gone.
// `btn` is optional and only carries the busy state.
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
        // The server's count excludes rows that were already read, so it is
        // usually smaller than the number of rows we posted. Report its
        // number, not ours.
        const n = affectedCount(resp) ?? ids.length;
        const message = `Marked ${n} ${n === 1 ? 'entry' : 'entries'} as read.`;
        const refreshed = await refreshEntriesList();
        if (!refreshed) {
            // No list pane to swap into, or the swap bailed: hand the message
            // to the next document via the cookie and reload as before.
            window.flash?.set('success', message);
            window.location.reload();
            return;
        }
        // Shown rather than `set()`: the page the message belongs to is the one
        // still on screen.
        window.flash?.success(message);
        // The rows are gone from the list, but the unread badges beside it are
        // still counting them.
        document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
        return;
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
    // The button lives *inside* the swapped `[data-entries-list]` container,
    // so a scoped-search swap discards the listener-bearing element and drops
    // in a fresh one — re-run on `rdrs:swap-complete` to re-bind. The
    // per-element guard keeps unrelated swaps (which leave this same button in
    // place) from stacking a second listener and double-POSTing.
    btn.dataset.markAboveBound = '1';
    btn.addEventListener('click', () => markLoadedEntriesAsRead(btn));
}
installMarkAboveButton();
document.addEventListener('rdrs:swap-complete', installMarkAboveButton);

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
        // Already-handled targets: the row action forms (star + read/unread
        // toggle, which submit themselves) and the title link itself.
        if (event.target.closest('form')) return;
        if (event.target.closest('a[data-swap="#reading-pane"]')) return;
        // Defer to any other link the user clicked (the feed-title meta link
        // and the open-original ↗ action, which open their own destinations).
        if (event.target.closest('a')) return;
        const link = row.querySelector('a[data-swap="#reading-pane"]');
        if (!link) return;
        event.preventDefault();
        link.click();
    });
}
installRowClickToOpen();
