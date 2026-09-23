// Declarative `data-` behaviours replacing inline `on*` attributes, which a
// strict `script-src 'self'` blocks silently (a destructive form would submit
// unconfirmed). Delegated, so swapped-in fragments work for free.

/**
 * `data-confirm="<message>"` on a <form>: cancel the submit if declined.
 * Capture phase, or app.js's form-swap handler would already have sent it.
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
 * `data-submit-on-change` on a <select>: submit the form on change.
 * `requestSubmit()` so the `submit` event (and csrf.js) still runs.
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
 * `data-hide-on-error` on an <img>: hide it when it fails to load.
 * Listener for in-flight images plus a sweep for ones that already failed;
 * `error` doesn't bubble, hence capture.
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
        // Complete with zero width means it failed.
        if (img.complete && img.naturalWidth === 0) img.hidden = true;
    }
}

installConfirm();
installSubmitOnChange();
installHideOnError();
