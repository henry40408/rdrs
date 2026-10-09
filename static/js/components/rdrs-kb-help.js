// <rdrs-kb-help> — help overlay (Shadow DOM): keyboard shortcuts, plus an optional
// search-syntax tab when `show()` is given one.
// Tokens deliberately have no `var(--x, fallback)`: they inherit from app.css,
// and fallbacks just drift. Add missing tokens to app.css.

// Constructable stylesheet: `style-src 'self'` blocks an inline style element in a shadow root.
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
.tabs {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    border-bottom: 1px solid var(--color-border-light);
    margin: -8px 0 18px;
}
.tabs[hidden],
.pane[hidden] {
    display: none;
}
.tab {
    appearance: none;
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    padding: 6px 12px;
    color: var(--color-text-secondary);
    font: inherit;
    cursor: pointer;
}
.tab[aria-selected="true"] {
    color: var(--color-accent-text);
    border-bottom-color: var(--color-accent);
}
.tab-hint {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: 0.75rem;
    white-space: nowrap;
}
.tab-hint kbd {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    padding: 0.1rem 0.3rem;
    background: var(--color-kbd-bg);
    border: 1px solid var(--color-border);
    border-bottom-width: 2px;
    border-radius: 4px;
    color: var(--color-text-secondary);
}
.pane {
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
.shortcut-key kbd,
.shortcut-key code {
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
    .pane {
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
                    <h2>Help</h2>
                    <button class="close-btn" id="close-btn">Esc</button>
                </div>
                <div class="tabs" role="tablist" id="tabs" hidden>
                    <button class="tab" role="tab" id="tab-shortcuts" aria-controls="content" aria-selected="true">Keyboard shortcuts</button>
                    <button class="tab" role="tab" id="tab-syntax" aria-controls="syntax" aria-selected="false" tabindex="-1">Search syntax</button>
                    <span class="tab-hint"><kbd>←</kbd> <kbd>→</kbd> switch</span>
                </div>
                <div class="pane" id="content" role="tabpanel" aria-labelledby="tab-shortcuts"></div>
                <div class="pane" id="syntax" role="tabpanel" aria-labelledby="tab-syntax" hidden></div>
            </div>
        `;

        this.shadowRoot.getElementById('close-btn').addEventListener('click', () => this.hide());
        const tabs = this.shadowRoot.getElementById('tabs');
        tabs.addEventListener('click', (e) => {
            const tab = e.target.closest('.tab');
            if (tab) this._selectTab(tab.id === 'tab-syntax' ? 'syntax' : 'shortcuts', false);
        });
        tabs.addEventListener('keydown', (e) => {
            if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
            e.preventDefault();
            this._selectTab(e.key === 'ArrowRight' ? 'syntax' : 'shortcuts', true);
        });
        this.addEventListener('click', (e) => {
            if (e.target === this) this.hide();
        });
        this.addEventListener('keydown', (e) => {
            if (e.key !== 'Escape') return;
            // Otherwise the document-level handler also closes the reading pane.
            e.preventDefault();
            e.stopPropagation();
            this.hide();
        });
    }

    _kbd(keyStr) {
        // Split on ' / ', ' + ' and spaces so combos render as separate <kbd>s.
        return keyStr.split(' / ').map(part => {
            if (part.includes('+')) {
                return part.split('+').map(k => `<kbd>${k}</kbd>`).join('+');
            }
            return part.split(' ').map(k => `<kbd>${k}</kbd>`).join(' ');
        }).join(' / ');
    }

    // `syntax` (same item shape) adds the Search syntax tab; `tab` picks the one
    // shown first ('shortcuts' | 'syntax').
    show(helpItems, { syntax, tab } = {}) {
        this._returnFocus = document.activeElement;
        this.shadowRoot.getElementById('content').innerHTML = this._renderGroups(helpItems, (k) => this._kbd(k));
        this.shadowRoot.getElementById('syntax').innerHTML = this._renderGroups(syntax, (k) => `<code>${this._esc(k)}</code>`);
        this.shadowRoot.getElementById('tabs').hidden = !(syntax && syntax.length > 0);
        this._selectTab(syntax && syntax.length > 0 && tab === 'syntax' ? 'syntax' : 'shortcuts', false);
        this.classList.add('visible');

        // The selected tab when there are tabs, so the arrows work at once.
        const tabs = this.shadowRoot.getElementById('tabs');
        const target = tabs.hidden
            ? this.shadowRoot.getElementById('close-btn')
            : tabs.querySelector('.tab[aria-selected="true"]');
        target.focus();
    }

    _selectTab(name, focus) {
        const root = this.shadowRoot;
        for (const id of ['shortcuts', 'syntax']) {
            const tab = root.getElementById(`tab-${id}`);
            const selected = id === name;
            tab.setAttribute('aria-selected', String(selected));
            tab.tabIndex = selected ? 0 : -1;
            root.getElementById(id === 'shortcuts' ? 'content' : 'syntax').hidden = !selected;
            if (selected && focus) tab.focus();
        }
    }

    _esc(text) {
        return String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    _renderGroups(items, renderKey) {
        if (!items || items.length === 0) return '';
        const groups = new Map();
        for (const item of items) {
            const groupName = item.group || 'Page';
            if (!groups.has(groupName)) groups.set(groupName, []);
            groups.get(groupName).push(item);
        }
        let html = '';
        for (const [groupName, group] of groups) {
            html += this._renderGroup(groupName, group, renderKey);
        }
        return html;
    }

    _renderGroup(title, items, renderKey) {
        let html = `<div class="shortcut-group"><h3>${title}</h3>`;
        items.forEach(item => {
            html += `<div class="shortcut-row">
                <span class="shortcut-key">${renderKey(item.key)}</span>
                <span class="shortcut-desc">${this._esc(item.desc)}</span>
            </div>`;
        });
        html += '</div>';
        return html;
    }

    hide() {
        this.classList.remove('visible');
        // Back to the control that opened it (the inline search's `?` button);
        // otherwise just drop focus.
        const back = this._returnFocus;
        this._returnFocus = null;
        if (back && back.isConnected && back !== document.body) {
            back.focus();
        } else if (document.activeElement) {
            document.activeElement.blur();
        }
    }

    get isVisible() {
        return this.classList.contains('visible');
    }
}

customElements.define('rdrs-kb-help', RdrsKbHelp);
