// <rdrs-flash> — accessible banner-stack for flash messages (Light DOM).
//
// Per-message ARIA role: role="status" (polite) for success/info,
// role="alert" (assertive) for warning/error. Dismiss is a real <button>;
// closing it returns focus to the element that triggered the message,
// or to the stack region as a fallback.

const MAX_MESSAGES = 3;

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

        const node = document.getElementById('rdrs-flash-bootstrap');
        if (node && node.textContent && !this._bootstrapApplied) {
            this._bootstrapApplied = true;
            try {
                const messages = JSON.parse(node.textContent);
                if (Array.isArray(messages)) {
                    for (const m of messages) {
                        if (m && m.level && m.message) this.show(m.level, m.message);
                    }
                }
            } catch { /* malformed — ignore */ }
        }
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

        const dismiss = document.createElement('button');
        dismiss.type = 'button';
        dismiss.className = 'banner-dismiss';
        dismiss.setAttribute('aria-label', 'Dismiss notification');
        dismiss.setAttribute('data-testid', 'flash-close');
        dismiss.textContent = '×';
        dismiss.addEventListener('click', () => {
            banner.remove();
            // Return focus to the original trigger if still in the DOM and visible,
            // otherwise to the stack region (which is tabindex=-1).
            if (trigger && document.contains(trigger) && typeof trigger.focus === 'function') {
                trigger.focus();
            } else {
                this.focus();
            }
        });

        banner.append(icon, body, dismiss);
        this.appendChild(banner);
    }

    success(message) { this.show('success', message); }
    error(message) { this.show('error', message); }
    info(message) { this.show('info', message); }
    warning(message) { this.show('warning', message); }

    redirect(url, level, message) {
        this.set(level, message);
        window.location.href = url;
    }
}

customElements.define('rdrs-flash', RdrsFlash);

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
    redirect(url, level, message) { this._el.redirect(url, level, message); },
};
