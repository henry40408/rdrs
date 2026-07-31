// static/js/behaviors.js — the declarative markup behaviours that used to be
// inline `on*` attributes.
//
// A strict `script-src 'self'` (no `'unsafe-inline'`, see
// middleware::security_headers) blocks every inline event-handler attribute, so
// `onsubmit="return confirm(…)"` and friends stop firing — silently, and in the
// *unsafe* direction: a destructive form would submit with no confirmation at
// all. Each one is re-expressed here as a `data-` attribute plus one delegated
// listener, which also means a swapped-in fragment (see `installSwap` in
// app.js) picks the behaviour up for free instead of shipping a fresh handler.
//
// Loaded from app_layout.html, so it covers the whole logged-in surface. The
// /login and /register pages extend base.html directly and carry none of these
// attributes; the one behaviour they do share — dismissing a server-rendered
// flash banner — lives in components/rdrs-flash.js, which base.html loads.

/**
 * `data-confirm="<message>"` on a <form>: ask before submitting, and cancel the
 * submit when the user declines.
 *
 * Capture phase on purpose. The bubble-phase form-swap handler in app.js would
 * otherwise have already fired for a `data-swap` form by the time a
 * bubble-phase listener here could cancel it, so a declined confirm would still
 * issue the request. Capturing lets `stopPropagation()` cut the event off
 * before any other listener sees it.
 */
function installConfirm() {
    document.addEventListener(
        'submit',
        (event) => {
            const form = event.target;
            if (!(form instanceof HTMLFormElement)) return;
            const message = form.getAttribute('data-confirm');
            if (!message) return;
            if (window.confirm(message)) return;
            event.preventDefault();
            event.stopPropagation();
        },
        true
    );
}

/**
 * `data-submit-on-change` on a <select>: submit the owning form as soon as the
 * selection changes, for filter bars that have no Apply button.
 *
 * `requestSubmit()` rather than `submit()` — the latter skips the `submit`
 * event entirely, which would bypass csrf.js's `_csrf` injection if one of
 * these forms ever became a POST.
 */
function installSubmitOnChange() {
    document.addEventListener('change', (event) => {
        const control = event.target;
        if (!(control instanceof HTMLElement)) return;
        if (!control.hasAttribute('data-submit-on-change')) return;
        const form = control.form;
        if (form) form.requestSubmit();
    });
}

/**
 * `data-hide-on-error` on an <img>: hide the element when the image fails to
 * load, so a feed whose cached icon 404s doesn't render a broken-image glyph.
 *
 * Two mechanisms, because this module is a deferred ES module and so runs
 * *after* the document has parsed: the listener catches images still in flight,
 * and the sweep catches any that already failed. `error` does not bubble, hence
 * the capture-phase registration.
 */
function installHideOnError() {
    document.addEventListener(
        'error',
        (event) => {
            const el = event.target;
            if (el instanceof HTMLImageElement && el.hasAttribute('data-hide-on-error')) {
                el.hidden = true;
            }
        },
        true
    );

    for (const img of document.querySelectorAll('img[data-hide-on-error]')) {
        // `complete` with a zero intrinsic width is the standard "finished, and
        // failed" signal — a successfully decoded image always reports a
        // non-zero naturalWidth.
        if (img.complete && img.naturalWidth === 0) img.hidden = true;
    }
}

installConfirm();
installSubmitOnChange();
installHideOnError();
