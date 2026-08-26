// <rdrs-flash> — accessible banner-stack for flash messages (Light DOM).
//
// role="status" (polite) for success/info, role="alert" (assertive) for
// warning/error. Dismiss is a real <button> that returns focus to whatever
// triggered the message, or to the stack region as a fallback.

const MAX_MESSAGES = 3;

// Shared with `localizeTime()`, so both emit paths render the clock identically.
const TIME_FORMAT = { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false };

const LEVEL_META = {
    success: { role: 'status', label: 'Success' },
    info:    { role: 'status', label: 'Info' },
    warning: { role: 'alert',  label: 'Warning' },
    error:   { role: 'alert',  label: 'Error' },
};

class RdrsFlash extends HTMLElement {
    connectedCallback() {
        this.classList.add('banner-stack');
        if (!this.hasAttribute('role')) this.setAttribute('role', 'region');
        if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', 'Notifications');
        // Focusable as a fallback target when the trigger element is gone.
        if (!this.hasAttribute('tabindex')) this.tabIndex = -1;

        // Nothing to bootstrap: the server renders the page's own messages into
        // this element (`macros::flash_mount`) — the only way a reader without
        // JavaScript sees them — already dismissible via the delegated handler
        // below. This element's job from here is `show()`.
    }

    /** Set a flash message cookie for next page load. */
    set(level, message) {
        const messages = [{ level, message }];
        document.cookie = 'flash=' + encodeURIComponent(JSON.stringify(messages)) + '; path=/; SameSite=Lax';
    }

    /** Show a flash message immediately on the page. */
    show(level, message) {
        if (!this.parentNode) {
            document.body.insertBefore(this, document.body.firstChild);
        }

        const existing = this.querySelectorAll('.banner');
        if (existing.length >= MAX_MESSAGES) {
            for (let i = 0; i <= existing.length - MAX_MESSAGES; i++) {
                existing[i].remove();
            }
        }

        const meta = LEVEL_META[level] || LEVEL_META.info;
        const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;

        const banner = document.createElement('div');
        banner.className = `banner banner--${level}`;
        banner.setAttribute('role', meta.role);
        banner.setAttribute('data-testid', 'flash-message');

        const icon = document.createElement('span');
        icon.className = 'banner-icon';
        icon.setAttribute('aria-hidden', 'true');

        const body = document.createElement('div');
        body.className = 'banner-body';
        const srLevel = document.createElement('span');
        srLevel.className = 'sr-only';
        srLevel.textContent = `${meta.label}: `;
        const msg = document.createElement('span');
        msg.className = 'banner-message';
        msg.textContent = message;
        body.append(srLevel, msg);

        // Cookie and inline-template flashes carry no timestamp into the JS
        // layer, so client-time is the one signal every emit path shares.
        const now = new Date();
        const time = document.createElement('time');
        time.className = 'banner-time';
        time.dateTime = now.toISOString();
        time.textContent = now.toLocaleTimeString(undefined, TIME_FORMAT);
        time.dataset.localized = '';
        time.setAttribute('data-testid', 'flash-time');

        const dismiss = document.createElement('button');
        dismiss.type = 'button';
        dismiss.className = 'banner-dismiss';
        dismiss.setAttribute('aria-label', 'Dismiss notification');
        dismiss.setAttribute('data-testid', 'flash-close');
        dismiss.innerHTML = '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>';
        dismiss.addEventListener('click', () => {
            banner.remove();
            if (trigger && document.contains(trigger) && typeof trigger.focus === 'function') {
                trigger.focus();
            } else {
                this.focus();
            }
        });

        banner.append(icon, body, time, dismiss);
        this.appendChild(banner);
    }

    success(message) { this.show('success', message); }
    error(message) { this.show('error', message); }
    info(message) { this.show('info', message); }
    warning(message) { this.show('warning', message); }

    /** Used by navigation-like partial swaps (opening a different entry,
     *  back/forward) so stale toasts don't follow the reader across views. */
    clear() {
        for (const banner of Array.from(this.querySelectorAll('.banner'))) {
            banner.remove();
        }
    }

    redirect(url, level, message) {
        this.set(level, message);
        window.location.href = url;
    }
}

customElements.define('rdrs-flash', RdrsFlash);

// Server-rendered banners never pass through `show()`, so their dismiss button
// gets no listener above — and the inline `onclick` it used to carry is blocked
// by a strict `script-src 'self'`, leaving an inert close button.
//
// Scoped to `[data-flash-dismiss]`, which only the macro emits: the buttons
// `show()` builds carry their own listener, and matching on `.banner-dismiss`
// would double-handle them.
document.addEventListener('click', (event) => {
    const button = event.target.closest('[data-flash-dismiss]');
    if (!button) return;
    button.closest('.banner')?.remove();
});

// Those same banners print UTC, while the ones `show()` builds print local time
// — one UI element reading two clocks, eight hours apart for a UTC+8 viewer.
// Rewritten from the unambiguous RFC 3339 `datetime` attribute, which still
// describes the UTC text a viewer without JS keeps.
const TIME_SELECTOR = 'time.banner-time[datetime]:not([data-localized])';

function localizeTime(node) {
    const parsed = new Date(node.getAttribute('datetime'));
    if (Number.isNaN(parsed.getTime())) return;
    node.dataset.localized = '';
    node.textContent = parsed.toLocaleTimeString(undefined, TIME_FORMAT);
}

function localizeTimesIn(root) {
    if (root.nodeType !== Node.ELEMENT_NODE) return;
    if (root.matches(TIME_SELECTOR)) localizeTime(root);
    for (const node of root.querySelectorAll(TIME_SELECTOR)) localizeTime(node);
}

localizeTimesIn(document.documentElement);

// Banners also arrive mid-session through a fragment swap, which no load-time
// pass can catch. Observing additions covers every such path without app.js
// having to know this module exists.
new MutationObserver((records) => {
    for (const record of records) {
        for (const node of record.addedNodes) localizeTimesIn(node);
    }
}).observe(document.documentElement, { childList: true, subtree: true });

window.flash = {
    get _el() {
        let el = document.querySelector('rdrs-flash');
        if (!el) {
            el = document.createElement('rdrs-flash');
            document.body.insertBefore(el, document.body.firstChild);
        }
        return el;
    },
    set(level, message) { this._el.set(level, message); },
    show(level, message) { this._el.show(level, message); },
    success(message) { this._el.success(message); },
    error(message) { this._el.error(message); },
    info(message) { this._el.info(message); },
    warning(message) { this._el.warning(message); },
    clear() { this._el.clear(); },
    redirect(url, level, message) { this._el.redirect(url, level, message); },
};
