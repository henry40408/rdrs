// Keyboard shortcut handler (ES module, not a Web Component)

const keyboard = {
    pendingKey: null,
    pendingTimeout: null,
    handlers: {},
    pageType: null,
    helpItems: [],

    init(pageType) {
        this.pageType = pageType;
        if (!this._bound) {
            this._bound = this.handleKeyDown.bind(this);
            document.addEventListener('keydown', this._bound);
        }
    },

    setHelpItems(items) {
        this.helpItems = items;
    },

    handleKeyDown(e) {
        // Ignore when typing in inputs
        if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') {
            return;
        }

        // Ignore if modifier keys are pressed (except shift)
        if (e.ctrlKey || e.altKey || e.metaKey) {
            return;
        }

        const key = e.key;

        // Handle pending key combinations (like 'g h')
        if (this.pendingKey) {
            const combo = this.pendingKey + ' ' + key;
            this.clearPending();
            if (this.handleCombo(combo)) {
                e.preventDefault();
                return;
            }
        }

        // Check for keys that start combinations
        if (key === 'g') {
            this.setPending('g');
            e.preventDefault();
            return;
        }

        // Handle single key shortcuts
        if (this.handleSingleKey(key, e.shiftKey)) {
            e.preventDefault();
        }
    },

    setPending(key) {
        this.pendingKey = key;
        const indicator = document.querySelector('rdrs-kb-pending');
        if (indicator) {
            indicator.show(key + '-');
        }
        // Clear pending after timeout
        this.pendingTimeout = setTimeout(() => {
            this.clearPending();
        }, 1500);
    },

    clearPending() {
        this.pendingKey = null;
        if (this.pendingTimeout) {
            clearTimeout(this.pendingTimeout);
            this.pendingTimeout = null;
        }
        const indicator = document.querySelector('rdrs-kb-pending');
        if (indicator) {
            indicator.hide();
        }
    },

    handleCombo(combo) {
        // Global navigation shortcuts
        switch (combo) {
            case 'g h': window.rdrsNavigate('/'); return true;
            case 'g e': window.rdrsNavigate('/entries'); return true;
            case 'g s': window.rdrsNavigate('/search'); return true;
        }
        // Page-specific combo handlers
        if (this.handlers.handleCombo) {
            return this.handlers.handleCombo(combo);
        }
        return false;
    },

    handleSingleKey(key, shiftKey) {
        // Show help
        if (key === '?') {
            this.toggleHelp();
            return true;
        }

        // Page-specific handlers
        if (this.handlers.handleKey) {
            return this.handlers.handleKey(key, shiftKey);
        }

        return false;
    },

    registerHandlers(handlers) {
        this.handlers = handlers;
    },

    toggleHelp() {
        const overlay = document.querySelector('rdrs-kb-help');
        if (!overlay) return;

        if (overlay.isVisible) {
            overlay.hide();
        } else {
            overlay.show(this.helpItems);
        }
    },

    hideHelp() {
        const overlay = document.querySelector('rdrs-kb-help');
        if (overlay) overlay.hide();
    }
};

// Expose globally
window.keyboard = keyboard;
