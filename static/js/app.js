// static/js/app.js — shared module for the logged-in surface: partial swaps,
// sidebar polling, theme, entries keyboard shortcuts, mark-as-read menus.

// The `?v=` cache-buster is substituted at serve time (handlers/static_assets.rs).
// Without it this nested import resolves to an unversioned URL that goes stale
// forever under the `immutable` cache header.
import { debounce } from './utils.js?v=__RDRS_ASSET_VERSION__';

/**
 * Intercept form / link interactions tagged with `data-swap="<selector>"`
 * and replace the matching element with HTML returned by the request.
 *
 * Response format:
 *   - HTML fragment: replaces the target element via outerHTML.
 *   - `<template data-swap-target="<selector>">…</template>` blocks: each
 *     template's content replaces its own target.
 *   - `<template data-class-target="<selector>"
 *     data-class-add|data-class-remove="a b">`: toggles classes on an element
 *     that is *not* being replaced, so a response can update a container's
 *     state class while swapping only its sub-elements.
 *
 * On a non-2xx response the helper falls back to native form submit / link
 * navigation so the user always sees a real page.
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
        // Only the action-bar Summarize form is tagged `data-summary-toggle`;
        // the error-state Retry form is not, so Retry still regenerates.
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
            // Without this, hidden inputs like `after=…` on the Load-More form
            // silently drop and the server falls through to a full-page render.
            const params = new URLSearchParams(new FormData(form));
            const sep = url.includes('?') ? '&' : '?';
            url = url + sep + params.toString();
        } else {
            init.body = new FormData(form);
        }
        setFormBusy(form, { cancellable: !!controller });
        try {
            await performSwap(url, init, target);
            // Mirror the search box into the address bar so a refresh / share
            // reproduces the filtered list, and so clearing the box removes the
            // stale `?q=`. The form lives outside the swapped container, so it
            // is still mounted here.
            if (form.matches('[data-entries-search]')) {
                syncScopedSearchParam(form);
            }
        } finally {
            // No-op on success (the swap detached the form); on a POST error
            // the original form is still mounted and gets its button back.
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
        // Write to `.action-label` so the sibling `.action-icon` SVG survives:
        // `btn.textContent` would wipe the icon, and a button whose swap target
        // is a *sibling* is not re-rendered, so the icon never comes back.
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

// Abort in-flight image downloads in the outgoing reading pane. Browsers do NOT
// reliably cancel an `<img>`'s request when the element is detached, and slow
// image-proxy downloads then hold the ~6 HTTP/1.1 per-origin connection slots,
// stalling the next entry's fragment fetch behind them (measured: hundreds of
// ms to >1s of connection-queue wait).
//
// Scoped to the image-proxied `.reading-pane-article` content images. The
// meta-row favicon is small, local and cached, so cancelling it only blanks a
// still-visible pane — a favicon flash on every entry switch.
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
    // textContent — never innerHTML — so alt text can't inject markup.
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
            // A dropped `src` means cancelPaneImages() aborted the download,
            // not a load failure — no broken-box flash on an outgoing pane.
            if (!img.getAttribute('src')) return;
            markBrokenImage(img);
        }, { once: true });
    }
}

// Monotonic token + abort handle for reading-pane *navigation* fetches (entry
// clicks, Show Original, popstate restores, prev/next fallbacks). Without it,
// clicking entry A then quickly B leaves both in flight and whichever lands
// last wins the pane — a slow A can overwrite the just-opened B and
// replaceState the URL back to ?entry=A. Action swaps (Save, Fetch Full
// Content) re-target the same entry and stay outside the guard. Same discipline
// as applyNeighborButtons().
let paneNavSeq = 0;
let paneNavAbort = null;

// Swap targets that live inside the reading pane, i.e. that only make sense
// for the entry currently open. See the staleness check in performSwap().
const PANE_REGION_TARGETS = new Set(['#reading-pane', '#rp-summary-container']);

/// The markup the server last delivered for each swap target: a byte-identical
/// next response means the DOM already shows it, so replacing the node is pure
/// churn — a layout and a repaint, which on WebKit is where images blink.
/// Clicking the sidebar feed that is *already* open hits this constantly.
///
/// Compared against the server's own previous answer, never against the DOM:
/// the live DOM carries what the server never sent (`.selected` from `j`/`k`,
/// `data-…-bound` listener markers, `title` on a localized `<time>`), all of
/// which made a DOM-to-response comparison differ.
///
/// Morph targets are exempt: their DOM is edited by swaps answering for *other*
/// targets, so two equal answers no longer imply the DOM still matches them.
/// Marking two feeds read in a row broke on exactly that — both answers are the
/// same empty list, so the second swap was skipped and the rows stayed on
/// screen unread. Morphing an identical tree writes nothing anyway.
///
/// A target is only known after the first swap that fills it.
const lastServerMarkup = new Map();

/// The single element a swap template carries, or null when it carries anything
/// else (Load More returns N rows plus a form). Indentation whitespace ignored.
function soleSwapElement(tpl) {
    const nodes = Array.from(tpl.content.childNodes)
        .filter((n) => n.nodeType !== Node.TEXT_NODE || n.textContent.trim() !== '');
    if (nodes.length !== 1 || nodes[0].nodeType !== Node.ELEMENT_NODE) return null;
    return nodes[0];
}

/// Swap targets whose subtree is morphed into shape rather than replaced. Both
/// are pure entry-row markup: re-rendering `[data-entries-list]` for "Mark Above
/// as Read" only adds `entry-read` to rows that survive, yet replacing the
/// container rebuilt every row and favicon inside (measured: none of six images
/// preserved).
///
/// The rest stay on replacement deliberately — `[data-list-pane]` carries
/// filter-bar values the markup does not describe and a scroller meant to reset
/// on a view switch, `#reading-pane` resets scroll on purpose, and a category
/// switch replaces the rows wholesale anyway.
function isMorphTarget(selector) {
    return selector === '[data-entries-list]' || selector.startsWith('#entry-row-');
}

/// Attributes the client writes onto server-rendered markup, which a morph must
/// leave alone. `data-…-bound` is the load-bearing one: it marks a control whose
/// listeners are installed, so stripping it from a surviving element invites a
/// second copy bound to the same node — one click, two POSTs.
const CLIENT_OWNED_ATTR = /^(data-.+-bound|data-img-.+|data-localized|data-tooltip-at|title)$/;

/// `.selected` is the `j`/`k` cursor, which the server has never heard of.
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

/// Reshape `from`'s children into `to`'s, reusing the nodes already there.
/// Elements carrying an `id` are matched by it, so a list that lost a row in the
/// middle keeps every surviving row's node; everything else matches positionally.
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

function morphNode(from, to) {
    if (from.nodeType !== Node.ELEMENT_NODE) {
        if (from.nodeValue !== to.nodeValue) from.nodeValue = to.nodeValue;
        return;
    }
    morphAttributes(from, to);
    morphChildren(from, to);
}

/// Morph the live `dst` into the shape of `incoming`, leaving surviving nodes
/// where they are. A re-inserted `<img>` sends WebKit back through load and
/// decode, so the icons blink; a morphed one is never touched at all.
function morphSwap(dst, incoming) {
    if (!morphCompatible(dst, incoming)) return false;
    morphNode(dst, incoming);
    return true;
}

/// Attributes the server re-stamps on every render that change nothing visible.
/// `data-snapshot-at` moves every second, so leaving it in the comparison would
/// make two responses for the same view never equal and the skip never fire.
const VOLATILE_SERVER_ATTRS = ['data-snapshot-at'];

function comparableServerMarkup(el) {
    const clone = el.cloneNode(true);
    for (const name of VOLATILE_SERVER_ATTRS) {
        for (const n of clone.querySelectorAll(`[${name}]`)) n.removeAttribute(name);
        clone.removeAttribute(name);
    }
    return clone.outerHTML;
}

/// Copy those attributes from the response onto the DOM being kept: a skipped
/// swap must not freeze the snapshot boundary at whatever the reader first
/// loaded, or `j`/`k` treats a widening set of entries as unread. The two trees
/// are identical apart from these attributes, so they line up one for one.
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
/// `<template data-swap-target>` blocks it carries). Resolves `false` when the
/// call bailed out — superseded, aborted, or handed off to a full navigation —
/// so callers must not run follow-up side effects (history, sidebar state).
///
/// `options.fallbackUrl` is where the error path navigates instead of `url`:
/// `?pane=1` returns bare `<template>` markup, so hard-navigating to the fetched
/// URL would leave the user on a blank page.
async function performSwap(url, init, defaultTarget, options) {
    const method = (init.method || 'GET').toUpperCase();
    const fallbackUrl = options?.fallbackUrl || url;
    // popstate restores pass `skipHistory: true`: the browser already moved the
    // address bar, so writing on top of that slot would corrupt it.
    const skipHistory = options?.skipHistory === true;
    const isPaneNav = method === 'GET' && defaultTarget === '#reading-pane';
    let navSeq = null;
    if (isPaneNav) {
        navSeq = ++paneNavSeq;
        paneNavAbort?.abort();
        paneNavAbort = new AbortController();
        init.signal = paneNavAbort.signal;
    }
    // Only when navigating to a *different* entry: an action swap re-targets
    // the same entry and would just reload the same images.
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
        // Falling through to `location.href` would hard-navigate to a fragment
        // URL the user has already moved past.
        if (isPaneNav && navSeq !== paneNavSeq) return false;
        // A reading pane the reader saved for offline reading is still on this
        // device, and reaching for it here rather than in the service worker is
        // what keeps this fetch an ordinary page request. `offline.js` publishes
        // the lookup, exactly like `window.flash` above; without that module —
        // a scriptless reader, or the feature switched off — there is nothing
        // to fall back to and the branches below take over.
        response = method === 'GET' ? await savedFragment(url) : null;
        if (!response) {
            if (method !== 'GET' && window.flash) {
                // The fetch threw, so the request never got an answer: no
                // network, or no server. Both mean waiting rather than retrying,
                // and this is the reliable half of the offline story —
                // `offline.js` blocks the submit when `navigator.onLine` says
                // the connection is gone, but that flag is a hint the browser is
                // free to get wrong, while a request that actually threw is
                // evidence. Deliberately not phrased as "you are offline": from
                // here the two are indistinguishable.
                window.flash.error('Could not reach the server — that will have to wait for the connection.');
            } else {
                window.location.href = fallbackUrl;
            }
            return false;
        }
        // A saved pane falls through to the response handling below, so it is
        // swapped in by exactly the same code that swaps a fetched one.
    }
    // Superseded while the headers were in flight — abort loses this race when
    // the reply was already buffered.
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

    // Decided BEFORE the DOM mutates: opening from the empty placeholder pushes
    // a slot (so back / edge-swipe closes the pane), switching entries replaces.
    const paneBefore = document.getElementById('reading-pane');
    const paneWasEmpty = !!paneBefore?.classList.contains('reading-pane-empty');
    // Pre-mutation, so the checks below compare against what was in the pane
    // rather than what was just swapped in. A different entry id means
    // navigation and clears stale flashes; an action swap keeps its own toast.
    const paneEntryIdBefore = currentPaneEntryId();
    const incomingEntryId = entryIdFromSwapUrl(url);

    // An action response belongs to the entry it was fired on: applying it after
    // the reader moved on paints one entry's summary into another's pane. The
    // window is small but real — an SSE `summary` event can pass its
    // `currentPaneEntryId()` pre-check and still land after the switch — so
    // re-check against the DOM as it is now, not as it was at fetch time.
    // Navigation is exempt (`paneNavSeq` covers it), as are row-scoped targets.
    if (!isPaneNav && PANE_REGION_TARGETS.has(defaultTarget) &&
        incomingEntryId && incomingEntryId !== paneEntryIdBefore) {
        // The action did happen server-side; only the markup is stale.
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
            // The reading pane is exempt from both paths below: replacing it
            // resets the scroll offset, which re-opening an entry relies on.
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
            // Child-by-child rather than outerHTML, for the multi-element
            // payloads (Load More returns N rows plus a new form).
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
 * The reading pane `offline.js` saved for `url`, or `null` when there is none —
 * the feature is off, the module never loaded, or this entry was outside the
 * budget. Errors are swallowed for the same reason: every one of them means the
 * same thing here, which is that there is nothing to show but the fallback.
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

// Mirror the entry id into `?entry={id}` so a refresh / share / back reproduces
// the pane (the SSR list handlers consume it via `maybe_build_reading_pane`).
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

/// Mirror the open entry into `?entry=`.
///
/// Replace-mode writes are coalesced to one per frame: `history.replaceState` is
/// among the most expensive things on the swap's synchronous path, and holding
/// `j` down issues one per keypress for only the last to matter.
///
/// Pushes are not deferred — a push must land in the same task as the
/// navigation that caused it or the history slot lands out of order.
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

// replaceState, never push — typing is a filter refinement, not a history entry.
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
    // `_reading_pane.html` stamps the id on the pane. The form scan below is a
    // substring-match selector over the whole article subtree and this runs
    // several times per swap, so it is only a fallback for panes rendered by
    // another template (error states, fragments predating the attribute).
    const stamped = pane.getAttribute('data-entry-id');
    if (stamped) return stamped;
    const form = pane.querySelector('form[action*="/entries/"]');
    const m = form?.action.match(/\/entries\/(\d+)\//);
    return m ? m[1] : null;
}

// Sync the reading pane to the URL on back/forward. Exactly one history slot is
// pushed per list visit (the first open from an empty pane): back from it lands
// without `?entry=` and closes the pane, forward re-mounts it. Cross-document
// navigation reloads instead and SSR consumes `?entry=` server-side.
window.addEventListener('popstate', () => {
    // Upfront rather than leaving it to performSwap's entry-mismatch clear,
    // which does not cover the close-pane branch.
    window.flash?.clear?.();
    // Sidebar navigation swaps in place and pushes its own slot, so back/forward
    // can land on a different *path* in the same document. Anything outside the
    // entries family can't be swapped and must reload, or the user gets a stale
    // list under a new URL.
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

// Reset `#reading-pane` to the SSR empty state in `_entries_layout.html`, so
// the mobile overlay dismisses: `.reading-pane-active` is what reveals the pane
// at ≤1024px, and leaving it over empty content traps the reader on a blank
// screen. False if the pane was already empty.
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
// Sidebar links, `[` / `]` / `{` / `}` and `g c` / `g f` swap rather than
// navigate. The `?pane=1` response carries the left column plus an emptied
// reading pane, so one swap leaves the sidebar untouched — a document reload
// resets `.sidebar-nav`'s internal scroll (and the document scroll on mobile),
// which is the jump this exists to avoid. Anything unswappable falls back to a
// normal navigation.
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

/// Swap the list pane over to `href`. `restoreEntry` re-opens the `?entry=` the
/// URL names, since the fragment always ships an empty pane.
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
    // Mirrors what category and feed pages server-render, so no top-level nav
    // item stays lit next to the highlighted row.
    sb?.setAttribute('active', '');
    if (feedId) {
        sb?.setAttribute('active-feed-id', feedId);
        // The caller's hint (an entry row carries its own category), else the
        // loaded feed lists — which cover a feed clicked in the sidebar.
        const parent = options.categoryId || sb?.categoryIdOfFeed?.(feedId);
        if (parent) sb.setAttribute('active-category-id', String(parent));
    } else {
        sb?.setAttribute('active-category-id', catId);
        sb?.removeAttribute('active-feed-id');
    }
    sb?.closeDrawer?.();
    // On mobile the document is the scroller and would otherwise keep the
    // previous category's offset.
    window.scrollTo({ top: 0 });
    const entryId = options.restoreEntry ? target.searchParams.get('entry') : null;
    if (entryId) {
        performSwap(`/entries/${entryId}/fragment`, { method: 'GET' }, '#reading-pane',
            { skipHistory: true });
    }
}

/// Every in-page anchor that lands on a category or feed list — one handler
/// rather than one per surface, since the surfaces kept being discovered one bug
/// report at a time. The breadcrumb's outer crumbs (`/categories`, `/feeds`) are
/// ordinary pages and fail the swappable-href test below.
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
        // An entry row knows its own category, which keeps the sidebar expanded
        // on the right group; absent it, swapListPane resolves it itself.
        const row = link.closest('[data-entry-row]');
        swapListPane(href, { categoryId: row?.dataset.categoryId });
    });
}
installListNav();

/// The sidebar rows `[` / `]` / `{` / `}` walk, in on-screen order: every
/// category, with the open category's feeds spliced in after it. The list grows
/// and shrinks as the reader moves, so the shortcuts step through exactly what
/// is visible.
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

// Mobile back button. Rendered in `_reading_pane.html` on every viewport;
// `.reading-pane-back` is `display: none` until the ≤1024px media block.
document.addEventListener('click', (event) => {
    if (event.button !== 0) return;
    if (!event.target.closest('[data-pane-back]')) return;
    event.preventDefault();
    closeReadingPane();
});

// ── Reading-pane prev/next ("neighbors") navigation ──────────────────
//
// The pane renders both buttons disabled; once it opens, the adjacent ids are
// resolved from `GET /api/entries/{id}/neighbors` under the current list filter
// and cached, so a click is an instant swap. The endpoint resolves order from
// the DB, so prev/next crosses pagination boundaries the DOM hasn't loaded.
//
// "Previous" = newer (up the published-desc list), "Next" = older — the same
// axis as the list's `k`/`j`.
let neighborState = { entryId: null, prevId: null, nextId: null };

// The current page's list filter as `NeighborsQuery` params, mirroring the
// server-side filter each route builds (handlers/pages.rs).
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
    // Snapshot semantics: echoing the render-time `data-snapshot-at` back as
    // `read_after` keeps entries read *during* this page view navigable, so j/k
    // can return to the entry just finished. Entries read before the page
    // loaded stay skipped, matching what the list rendered.
    if (out.get('unread_only') === 'true') {
        const snapshotAt = document
            .querySelector('[data-entries-list]')
            ?.getAttribute('data-snapshot-at');
        if (snapshotAt) out.set('read_after', snapshotAt);
    }
    return out.toString();
}

// Applied only while the ids still describe the entry in the pane, so a stale
// fetch landing after the reader moved on leaves both buttons disabled.
function applyNeighborButtons() {
    // Scoped to the pane: `#reading-pane` sits after the list in document order,
    // so a document-wide selector walks every entry row to reach these two
    // buttons — on every swap and every neighbor resolve.
    const pane = document.getElementById('reading-pane');
    const prevBtn = pane?.querySelector('[data-pane-prev]');
    const nextBtn = pane?.querySelector('[data-pane-next]');
    const open = currentPaneEntryId();
    const valid = open != null && neighborState.entryId === open;
    if (prevBtn) prevBtn.disabled = !(valid && neighborState.prevId != null);
    if (nextBtn) nextBtn.disabled = !(valid && neighborState.nextId != null);
}

// Answer prev/next from the DOM, saving the ~7-query round trip per entry
// opened. Null when it can't, and the caller falls back to the server.
//
// The rows are flat siblings in the same order and filter `find_neighbors`
// resolves server-side, and nothing removes one once rendered — marking an
// entry read only restyles its row, and the `read_after` snapshot keeps the
// server counting it. So an interior row's DOM neighbours are its real ones.
//
// Interior rows only: the first row is the head of the list *as rendered* and
// the last is never the end of the set (Load More may have pages left), so
// neither end can prove a `null`.
//
// Skipped under a scoped search — `currentEntryFilterParams` does not forward
// `q`, so the server resolves across the *unsearched* set and answering from a
// searched DOM would change which entry j/k lands on.
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

// Buttons are disabled up-front so a slow fetch never leaves a stale direction.
let lastResolvedPaneId = null;
function maybeResolveNeighbors() {
    const id = currentPaneEntryId();
    if (id === lastResolvedPaneId) {
        // An action swap re-targeting the same entry re-renders the buttons back
        // to their default `disabled`. neighborState is still valid, so re-apply
        // it — otherwise they stay disabled, and since a disabled button swallows
        // taps, mobile prev/next dies for good (j/k bypasses them).
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

// Submit the Load-More form once for its current cursor, so the list catches up
// with a pane that navigated past the loaded page.
//
// Guarding on the cursor *value* rather than an in-flight flag is what makes
// repeat calls safe: an append replaces the form with one carrying the next
// cursor, so holding `j` re-enters with the same cursor and no-ops. A flag
// cleared on `rdrs:swap-complete` would be cleared too early by the pane swap
// firing alongside.
//
// One page per call: stepping through entries lands exactly one past the loaded
// page, so a far-away `?entry=` deep-link stays out of reach rather than firing
// a burst. The key includes the form action, so another list whose next page
// starts at the same cursor still auto-loads.
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

// Open the neighbor in `direction`. Clicking the loaded list row's link is
// preferred because it keeps the keyboard selection in sync; entries beyond the
// loaded page fall back to a direct fragment swap.
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
    // Safe to fire behind the pane swap: they target different nodes and only
    // `#reading-pane` GETs go through the pane-nav abort guard.
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

// Turn `<template data-flash data-level="…">` blocks in a swap response into
// toasts on the page-level `<rdrs-flash>`, for post-action feedback with no
// corresponding DOM state change (Save, Fetch Full Content).
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

// Lets an action response update a *container's* state class without shipping
// the container back: a row action re-renders only the marker form, but the row
// still has to gain or lose `entry-read`.
//
// add/remove rather than setting `class` wholesale — the same element carries
// client-only classes (`.selected` from j/k) a full overwrite would drop.
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

// The mobile drawer lives entirely inside <rdrs-sidebar>: it owns the markup,
// so it owns the behaviour and the listener lifecycle.

installSwap();

// Live updates over one SSE stream, replacing the old 20s sidebar poll:
// `sidebar` refetches /api/sidebar, `summary` updates the row badge and the open
// pane. EventSource reconnects natively; each reconnect resyncs the sidebar.
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
    // `open` also fires on the first connect, where <rdrs-sidebar> has already
    // fetched from connectedCallback. Only later ones are reconnects that need a
    // resync for whatever changed while the stream was down.
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

// Refetching on *every* swap over-fetches slightly — star/save/fetch-full-content
// don't change sidebar state — but a hit on the server-side per-user sidebar
// cache costs roughly nothing, and one broad hook beats a fragile per-action
// allowlist.
document.addEventListener('rdrs:swap-complete', () => {
    refreshSidebar();
});

// Decorate every `<time datetime>` with a `title` in the browser's locale and
// timezone: the server emits UTC and only the client knows the user's TZ.
//
// `data-local-text` elements display absolute rather than relative times, so
// their textContent is replaced too; the server-rendered UTC string stays as the
// no-JS fallback.
function applyTimeTooltips(root) {
    const scope = root || document;
    for (const el of scope.querySelectorAll('time[datetime]')) {
        const iso = el.getAttribute('datetime');
        if (!iso) continue;
        // This runs after *every* swap over the whole document, so a list paged
        // to 500 rows would re-format 500 instants per keypress. The server owns
        // `datetime`, so a timestamp that really changed still re-formats.
        //
        // Deliberately *not* `data-localized`: that name belongs to
        // rdrs-flash.js as a valueless "already rewritten" marker.
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

// Single source of truth for the in-app shortcut help: pages register nothing
// extra, so every shortcut the keyboard handler recognizes is listed here.
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
// A first `g` arms the namespace, the second key picks the target; entry-relative
// jumps (g f / g c) need a selected row. The pending state times out so a stray
// `g` doesn't swallow the next keystroke forever, and the listener captures and
// stops propagation so `g s` can never double as the single-key Save.
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
        if (link) swapListPane(link.getAttribute('href'), { categoryId: row?.dataset.categoryId });
        return;
    }
    // key === 'c' — the selected entry's own category, else the page-parent
    // category on /feeds/{id}/entries.
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
            // Consumed unconditionally: a mistyped sequence must not fire that
            // key's unrelated single-key binding.
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
    // By id, not by node: a multi-target swap can replace the row element, and
    // `indexOf` on the orphan returns -1, sending `j`/`k` back to the top.
    let activeId = null;
    // Cached node behind `activeId`. Rows are usually morphed rather than
    // replaced, so re-resolving by attribute selector several times per keypress
    // against a list hundreds of rows long is repeat work. Validated on every
    // read, so a row that *was* replaced falls back to the query.
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
    // A server-rendered replacement row cannot carry the client-side
    // `.selected`, so it is re-applied after every swap.
    //
    // The open pane is the authority on *which* entry that is: a neighbor past
    // the loaded page swaps only `#reading-pane`, so no row is clicked and
    // `activeId` would otherwise stay on the last loaded row — keeping
    // `.selected` there and sending `j`/`k` back to it when the pane closes. A
    // missing row is fine: Load More appends it and re-runs this handler.
    document.addEventListener('rdrs:swap-complete', () => {
        const paneId = currentPaneEntryId();
        if (paneId != null && paneId !== activeId) {
            // Before reassigning, or the orphaned `.selected` shows two
            // highlights.
            activeRow()?.classList.remove('selected');
            activeId = paneId;
        }
        const row = activeRow();
        if (row) row.classList.add('selected');
    });
    // Sync `activeId` on a title click so `j`/`k` continue from the clicked row
    // rather than the last keyboard selection.
    document.addEventListener('click', (e) => {
        const link = e.target.closest('[data-entry-row] a[data-swap="#reading-pane"]');
        if (!link) return;
        const row = link.closest('[data-entry-row]');
        if (row) focusRow(row);
    });
    // Null when no entry is loaded, or when the form's submit button is
    // disabled (Summarize while a request is in flight).
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
                // With the pane open, j/k navigate across the whole filter
                // rather than only the loaded rows; the selection follows.
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
                // The row has no read form since the Wire Room redesign, so
                // drive the swap machinery directly rather than appending a
                // throwaway one. The in-flight guard keeps a rapid double-press
                // from reading stale state and double-POSTing.
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
                // The `[data-status-filter] <select>` options, in order:
                // All / Unread / Read / Starred.
                const options = document.querySelectorAll('[data-status-filter] option');
                if (options.length === 0) return;
                const idx = parseInt(e.key, 10) - 1;
                if (idx < 0 || idx >= options.length) return;
                e.preventDefault();
                window.location.href = options[idx].value;
                break;
            }
            case 'A': {
                // The button is hidden while a scoped search is active, since
                // "Mark N matching as Read" owns that case visually — but the
                // shortcut still works there, hence the search-box check.
                const btn = document.getElementById('mark-above-read');
                const searching = !!document.querySelector('[data-entries-search] input[name="q"]')?.value;
                if (!btn && !searching) return;
                e.preventDefault();
                markLoadedEntriesAsRead(btn);
                break;
            }
            case 'v': {
                // The URL rides on `data-entry-link`, absent when the entry has
                // no link.
                const current = activeRow();
                const url = current?.getAttribute('data-entry-link');
                if (!url) return;
                e.preventDefault();
                // `window.open(url, '_blank', <features>)` drops noreferrer, and
                // a non-empty features string forces a popup instead of a tab.
                const a = document.createElement('a');
                a.href = url;
                a.target = '_blank';
                a.rel = 'noopener noreferrer';
                a.click();
                break;
            }
            case 'd': {
                // Once the pane shows full content the Fetch button is replaced
                // by a "Show Original" link, so fall through to that.
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
                // Swallowed while in flight, so requestSubmit() can't bypass the
                // disabled button; Cancel lives in the summary box.
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
                // Starts from the current feed or category; every other list
                // page enters at the first row going forward, the last going
                // back. Wrapping stays inside this list, never out to Unread/All.
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
                // Virtual start index when nothing is current, so the first
                // probe lands on targets[0] / the last one.
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
                // One key, one meaning: no fallback action when the pane is
                // empty.
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

// Shared by the action-bar Summarize button and the 'a' shortcut. Clicking the
// mounted Dismiss control reuses the DELETE + clear flow in
// installSummaryActions. False when no summary is showing, so callers fall
// through to kicking one off.
function dismissVisibleSummary() {
    const dismiss = document.querySelector('#reading-pane [data-summary-dismiss]');
    if (!dismiss) return false;
    dismiss.click();
    return true;
}

// True while the summary is mid-generation, when Cancel is the only intended
// action and the Summarize/Dismiss toggle must stay inert.
function summaryInFlight() {
    return !!document.querySelector('#reading-pane [data-summary-pending]');
}

// The button flips behaviour in the submit handler, so its label / icon /
// aria-label must follow. Skipped mid-request so it doesn't clobber the
// transient "Summarizing…" label — the SSE completion swap syncs again once
// busy clears.
function syncSummarizeToggleLabel() {
    const form = document.querySelector('#reading-pane [data-summary-toggle]');
    if (!form || form.getAttribute('aria-busy') === 'true') return;
    const btn = form.querySelector('button');
    if (!btn) return;
    const showing = !!document.querySelector('#reading-pane [data-summary-dismiss]');
    // Matches the server render; the handler gates cover the window before this
    // sync catches up.
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

// Copy writes to the clipboard; Dismiss DELETEs the cached summary and strips
// the summary block and the entry row's badge.
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
                // Writing the button's textContent would clobber the icon span,
                // dropping the glyph until the container re-renders.
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
            // The reader can switch entries while the DELETE is in flight, and
            // clearing then would blank the *new* entry's summary. Same
            // staleness rule performSwap() applies to the same target.
            if (String(currentPaneEntryId()) === String(entryId)) {
                // The wrapper stays: it is the swap target for a later
                // summarize click.
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

// "Mark as Read..." posts to the GReader bulk-mark endpoint with an optional
// `ts=` cutoff, then swaps the refreshed list in place: a `location.reload()`
// would throw away the open entry, the sidebar's loaded feed lists and both
// scroll positions to redraw a list that only lost some rows. A native form-POST
// would navigate the reader to a JSON response, so JS glue is the long-term home
// (the GReader API itself is permanent per the SSR-first spec).
const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};
const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';

// Bulk writes keep the GReader-standard `OK` body and carry the row count in
// `X-RDRS-Affected`. Null when it is missing or unparseable, so callers fall
// back to their own estimate rather than reporting "Marked null entries".
function affectedCount(resp) {
    const raw = resp.headers.get('X-RDRS-Affected');
    if (raw === null) return null;
    const n = Number.parseInt(raw, 10);
    return Number.isNaN(n) ? null : n;
}

// Bound per element and re-run after every swap: a category swap replaces the
// whole list-pane header, discarding the listener-bearing <select>. The guard
// stops swaps that leave the header in place from stacking a second listener.
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
        // The GReader stream ID for the current page (`feed/<url>`,
        // `user/-/label/<cat>`), so one dropdown scopes to whatever is on screen.
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
                // No list pane, or the swap bailed: hand the message to the
                // next document via the cookie.
                window.flash?.set('success', message);
                window.location.reload();
                return;
            }
            // Shown rather than `set()`: the page it belongs to is still up.
            window.flash?.success(message);
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

// Each option's value is the URL to navigate to, and the 1-4 keys hit the same
// options by position. Re-bound after swaps like the dropdown above.
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
// The server renders the drawer open when the request carried `?q=`, so deep
// links and list swaps arrive in the right state; only the interactive
// transitions are handled here.
//
// Closing clears the search: a collapsed box that is still filtering turns a
// short list into a mystery.
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
    // Only when there is something to clear — a needless swap would drop the
    // reader's scroll position.
    if (input && input.value !== '') {
        input.value = '';
        form?.requestSubmit();
    }
    // Focus would otherwise stay on a control inside a collapsed, zero-height
    // container, which strands the keyboard user.
    toggle?.focus();
}

// Delegated, and therefore installed exactly once: a list-pane swap replaces
// both the toggle and the close button, so per-element binding would have to
// re-run every swap and would stack duplicate listeners when it did.
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

// The form lives outside the swapped `[data-entries-list]`, so it survives every
// swap and keeps focus and caret while typing; only the binding is re-applied,
// for swaps that re-render the whole layout. `installSwap()`'s submit handler
// does the actual GET → query-string → swap.
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

/// Re-render the current list in place from `?fragment=1` — page 1 of whatever
/// the URL already asks for, status tab and scoped search included.
///
/// Resolves `false` when there is no list to swap or the swap bailed, so callers
/// can fall back to a reload rather than leave stale rows on screen.
async function refreshEntriesList() {
    if (!document.querySelector('[data-entries-list]')) return false;
    const url = new URL(window.location.href);
    url.searchParams.set('fragment', '1');
    // `after` would answer with the Load-More *append* fragment; `entry` feeds
    // the SSR reading pane, which this response leaves alone.
    url.searchParams.delete('after');
    url.searchParams.delete('entry');
    const applied = await performSwap(url.toString(), { method: 'GET' }, '[data-entries-list]',
        { fallbackUrl: window.location.href }); // `?fragment=1` is not a page.
    if (applied) scrollEntriesListToTop();
    return applied;
}

/// Send the list scroller back to the first row: a bulk mark-as-read answers
/// with page 1 again, so the kept offset points at unrelated rows — or, after
/// "Mark Above as Read", past the end of the list.
///
/// Both scrollers are reset, as in `swapListPane()`: desktop scrolls
/// `[data-entries-list]` internally, mobile scrolls the document.
function scrollEntriesListToTop() {
    document.querySelector('[data-entries-list]')?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
}

// Marks every entry currently rendered — loaded rows plus anything Load More
// appended — via one `i=<id>` per row to the GReader edit-tag endpoint. Entries
// not yet loaded stay untouched.
//
// Split from the button so the `A` shortcut still reaches it while a scoped
// search hides the button. `btn` is optional and only carries the busy state.
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
        // usually smaller than the number posted.
        const n = affectedCount(resp) ?? ids.length;
        const message = `Marked ${n} ${n === 1 ? 'entry' : 'entries'} as read.`;
        const refreshed = await refreshEntriesList();
        if (!refreshed) {
            // No list pane, or the swap bailed: hand the message to the next
            // document via the cookie.
            window.flash?.set('success', message);
            window.location.reload();
            return;
        }
        // Shown rather than `set()`: the page it belongs to is still up.
        window.flash?.success(message);
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
    // The button lives *inside* the swapped container, so a scoped-search swap
    // drops in a fresh one; the guard keeps swaps that leave it in place from
    // stacking a second listener and double-POSTing.
    btn.dataset.markAboveBound = '1';
    btn.addEventListener('click', () => markLoadedEntriesAsRead(btn));
}
installMarkAboveButton();
document.addEventListener('rdrs:swap-complete', installMarkAboveButton);

// Clicking anywhere on a row opens the entry, by delegating to the title's
// `<a data-swap="#reading-pane">` so `installSwap()` handles the multi-target
// response (auto-mark-as-read, sidebar update).
function installRowClickToOpen() {
    document.addEventListener('click', (event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey ||
            event.shiftKey || event.altKey) return;
        const row = event.target.closest('[data-entry-row]');
        if (!row) return;
        // Already handled: the row action forms and the title link itself.
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
