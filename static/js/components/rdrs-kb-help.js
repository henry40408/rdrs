// <rdrs-kb-help> — Keyboard shortcut help overlay (Shadow DOM)
//
// The shadow stylesheet references design tokens with no `var(--x, fallback)`
// defaults, on purpose. Custom properties inherit across the shadow boundary,
// and app.css is a render-blocking <link> in <head> that defines every token
// used here — so a fallback can only ever fire in a scenario where the whole
// app is already unstyled. Meanwhile each fallback is a second copy of a token
// that nothing keeps in sync, and it silently freezes the *light* half of a
// `light-dark()` pair. Four of them had already drifted from app.css before
// they were removed (--color-overlay, --font-ui, --font-display, --font-mono).
// Don't reintroduce them: add the token to app.css instead.

// The shadow styles are adopted as a constructable stylesheet rather than
// injected as an inline style element. Markup parsed into a shadow root is
// policed by `style-src` exactly like markup in the document, and the app's
// Content-Security-Policy is `style-src 'self'` — an inline style element
// here would simply not apply. The CSSOM route is not markup, so it is
// unaffected; see src/middleware/security_headers.rs.
const HELP_STYLES = new CSSStyleSheet();
HELP_STYLES.replaceSync(`
:host {
    display: none;
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: var(--color-overlay);
    z-index: 1000;
    justify-content: center;
    align-items: center;
    padding: var(--space-4);
}
:host(.visible) {
    display: flex;
}
.modal {
    background: var(--color-panel);
    border: 1px solid var(--color-border-light);
    border-radius: 12px;
    padding: 28px 32px 32px;
    width: 100%;
    max-width: 720px;
    max-height: 85vh;
    overflow-y: auto;
    font-size: 0.9375rem;
    color: var(--color-text);
    font-family: var(--font-ui);
    box-shadow: var(--shadow-lg);
}
.header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 22px;
}
h2 {
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 600;
    margin: 0;
}
.close-btn {
    appearance: none;
    background: var(--color-kbd-bg);
    border: 1px solid var(--color-border);
    border-bottom-width: 2px;
    border-radius: 4px;
    color: var(--color-text-secondary);
    font-family: var(--font-mono);
    font-size: 0.75rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
    line-height: 1;
}
.close-btn:hover {
    color: var(--color-text);
}
#content {
    columns: 2;
    column-gap: var(--space-8);
}
.shortcut-group {
    break-inside: avoid;
    margin-bottom: var(--space-4);
}
.shortcut-group h3 {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.16em;
    color: var(--color-accent-text);
    border-bottom: 1px solid var(--color-border-light);
    padding-bottom: 6px;
    margin: 0 0 8px;
}
.shortcut-row {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    padding: 3.5px 0;
}
.shortcut-key {
    flex-shrink: 0;
    width: 7rem;
    text-align: right;
}
.shortcut-key kbd {
    display: inline-block;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    line-height: 1;
    padding: 0.2rem 0.4rem;
    background: var(--color-kbd-bg);
    border: 1px solid var(--color-border);
    border-bottom-width: 2px;
    border-radius: 4px;
    color: var(--color-text-secondary);
    white-space: nowrap;
}
.shortcut-desc {
    color: var(--color-text-secondary);
    font-size: 0.875rem;
    line-height: 1.4;
}

/* Single-column on narrow screens */
@media (max-width: 520px) {
    .modal {
        padding: var(--space-4) var(--space-5);
        max-height: 85vh;
    }
    #content {
        columns: 1;
    }
    .shortcut-key {
        width: 7rem;
    }
}
`);

class RdrsKbHelp extends HTMLElement {
    constructor() {
        super();
        const shadow = this.attachShadow({ mode: 'open' });
        shadow.adoptedStyleSheets = [HELP_STYLES];
        shadow.innerHTML = `
            <div class="modal">
                <div class="header">
                    <h2>Keyboard Shortcuts</h2>
                    <button class="close-btn" id="close-btn">Esc</button>
                </div>
                <div id="content"></div>
            </div>
        `;

        this.shadowRoot.getElementById('close-btn').addEventListener('click', () => this.hide());
        this.addEventListener('click', (e) => {
            if (e.target === this) this.hide();
        });
        this.addEventListener('keydown', (e) => {
            if (e.key !== 'Escape') return;
            // Stop the event here: it would otherwise bubble on to the
            // document-level entries handler, whose `help.isVisible` guard
            // re-checks AFTER hide() has flipped it — and then closes the
            // reading pane too.
            e.preventDefault();
            e.stopPropagation();
            this.hide();
        });
    }

    _kbd(keyStr) {
        // Render key string as <kbd> elements, splitting on ' / ', ' + ', and spaces for combos
        return keyStr.split(' / ').map(part => {
            if (part.includes('+')) {
                // e.g. "Shift+Space" → <kbd>Shift</kbd>+<kbd>Space</kbd>
                return part.split('+').map(k => `<kbd>${k}</kbd>`).join('+');
            }
            // e.g. "g g" → <kbd>g</kbd> <kbd>g</kbd>
            return part.split(' ').map(k => `<kbd>${k}</kbd>`).join(' ');
        }).join(' / ');
    }

    show(helpItems) {
        const content = this.shadowRoot.getElementById('content');
        let html = '';

        if (helpItems && helpItems.length > 0) {
            const groups = new Map();
            for (const item of helpItems) {
                const groupName = item.group || 'Page';
                if (!groups.has(groupName)) groups.set(groupName, []);
                groups.get(groupName).push(item);
            }
            for (const [groupName, items] of groups) {
                html += this._renderGroup(groupName, items);
            }
        }

        content.innerHTML = html;
        this.classList.add('visible');

        this.shadowRoot.getElementById('close-btn').focus();
    }

    _renderGroup(title, items) {
        let html = `<div class="shortcut-group"><h3>${title}</h3>`;
        items.forEach(item => {
            html += `<div class="shortcut-row">
                <span class="shortcut-key">${this._kbd(item.key)}</span>
                <span class="shortcut-desc">${item.desc}</span>
            </div>`;
        });
        html += '</div>';
        return html;
    }

    hide() {
        this.classList.remove('visible');
        if (document.activeElement) {
            document.activeElement.blur();
        }
    }

    get isVisible() {
        return this.classList.contains('visible');
    }
}

customElements.define('rdrs-kb-help', RdrsKbHelp);
