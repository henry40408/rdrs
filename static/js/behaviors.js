// static/js/behaviors.js — the declarative markup behaviours that used to be
// inline `on*` attributes.
//
// A strict `script-src 'self'` (middleware::security_headers) blocks inline
// event-handler attributes silently and in the *unsafe* direction: a
// destructive form would submit with no confirmation at all. Each one is
// re-expressed as a `data-` attribute plus one delegated listener, which also
// means a swapped-in fragment picks the behaviour up for free.
//
// Loaded from app_layout.html. /login and /register extend base.html directly
// and carry none of these attributes; the flash-dismiss behaviour they do share
// lives in components/rdrs-flash.js.

/**
 * `data-confirm="<message>"` on a <form>: ask before submitting, and cancel the
 * submit when the user declines.
 *
 * Capture phase on purpose: the bubble-phase form-swap handler in app.js would
 * already have issued the request by the time a bubble-phase listener here
 * could cancel it.
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
 * event, bypassing csrf.js's `_csrf` injection if one of these ever became a
 * POST.
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
 * Two mechanisms, because a deferred ES module runs after the document has
 * parsed: the listener catches images still in flight, the sweep catches any
 * that already failed. `error` does not bubble, hence capture phase.
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
        // failed" signal.
        if (img.complete && img.naturalWidth === 0) img.hidden = true;
    }
}

installConfirm();
installSubmitOnChange();
installHideOnError();
