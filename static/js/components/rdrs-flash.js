// <rdrs-flash> — Client-side flash/toast message system (Light DOM)

class RdrsFlash extends HTMLElement {
    constructor() {
        super();
        this._maxMessages = 3;
    }

    connectedCallback() {
        if (!this.classList.contains('flash-container')) {
            this.classList.add('flash-container');
        }
    }

    _formatTime(date) {
        return date.toLocaleTimeString('en-US', {
            hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit'
        });
    }

    _escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    /** Set a flash message cookie for next page load. */
    set(level, message) {
        const messages = [{ level, message }];
        document.cookie = 'flash=' + encodeURIComponent(JSON.stringify(messages)) + '; path=/; SameSite=Lax';
    }

    /** Show a flash message immediately on the page. */
    show(level, message) {
        // Ensure we're in the DOM
        if (!this.parentNode) {
            document.body.insertBefore(this, document.body.firstChild);
        }

        // Remove oldest messages if we have too many
        const existing = this.querySelectorAll('.flash');
        if (existing.length >= this._maxMessages) {
            for (let i = 0; i <= existing.length - this._maxMessages; i++) {
                existing[i].remove();
            }
        }

        const div = document.createElement('div');
        div.className = `flash flash-${level}`;
        div.setAttribute('data-testid', 'flash-message');
        const timestamp = this._formatTime(new Date());
        div.innerHTML = `<span>${this._escapeHtml(message)}</span><span class="flash-right"><span class="flash-time">${timestamp}</span> <a href="#" class="flash-close" data-testid="flash-close" onclick="this.parentElement.parentElement.remove(); return false;">\u00d7</a></span>`;
        this.appendChild(div);
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

// Global proxy for backwards compatibility
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
    escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }
};
