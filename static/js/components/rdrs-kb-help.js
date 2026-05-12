// <rdrs-kb-help> — Keyboard shortcut help overlay (Shadow DOM)

class RdrsKbHelp extends HTMLElement {
    constructor() {
        super();
        const shadow = this.attachShadow({ mode: 'open' });
        shadow.innerHTML = `
            <style>
                :host {
                    display: none;
                    position: fixed;
                    top: 0;
                    left: 0;
                    right: 0;
                    bottom: 0;
                    background: var(--color-overlay, rgba(0, 0, 0, 0.5));
                    z-index: 1000;
                    justify-content: center;
                    align-items: center;
                    padding: var(--space-4, 1rem);
                }
                :host(.visible) {
                    display: flex;
                }
                .modal {
                    background: var(--color-bg, #FAF8F5);
                    border: 1px solid var(--color-border, #D4CFC8);
                    border-radius: var(--radius-lg, 10px);
                    padding: var(--space-6, 1.5rem) var(--space-8, 2rem);
                    width: 100%;
                    max-width: 640px;
                    max-height: 80vh;
                    overflow-y: auto;
                    font-size: 0.9375rem;
                    color: var(--color-text, #1A1715);
                    font-family: var(--font-ui, 'DM Sans', sans-serif);
                    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.12);
                }
                .header {
                    display: flex;
                    align-items: center;
                    justify-content: space-between;
                    margin-bottom: var(--space-4, 1rem);
                    padding-bottom: var(--space-3, 0.75rem);
                    border-bottom: 1px solid var(--color-border-light, #E5E1DB);
                }
                h2 {
                    font-family: var(--font-display, 'Playfair Display', serif);
                    font-size: 1.25rem;
                    font-weight: 600;
                    margin: 0;
                }
                .close-btn {
                    appearance: none;
                    background: none;
                    border: 1px solid var(--color-border, #D4CFC8);
                    border-radius: var(--radius-sm, 3px);
                    color: var(--color-text-muted, #8A847D);
                    font-family: var(--font-mono, 'JetBrains Mono', monospace);
                    font-size: 0.75rem;
                    padding: 0.2rem 0.5rem;
                    cursor: pointer;
                    line-height: 1;
                }
                .close-btn:hover {
                    background: var(--color-bg-tertiary, #EBE7E2);
                    color: var(--color-text, #1A1715);
                }
                #content {
                    columns: 2;
                    column-gap: var(--space-8, 2rem);
                }
                .shortcut-group {
                    break-inside: avoid;
                    margin-bottom: var(--space-4, 1rem);
                }
                .shortcut-group h3 {
                    font-family: var(--font-ui, 'DM Sans', sans-serif);
                    font-size: 0.6875rem;
                    font-weight: 600;
                    text-transform: uppercase;
                    letter-spacing: 0.08em;
                    color: var(--color-text-muted, #8A847D);
                    margin: 0 0 var(--space-2, 0.5rem);
                }
                .shortcut-row {
                    display: flex;
                    align-items: baseline;
                    gap: var(--space-3, 0.75rem);
                    padding: 0.2rem 0;
                }
                .shortcut-key {
                    flex-shrink: 0;
                    min-width: 5.5rem;
                    text-align: right;
                }
                .shortcut-key kbd {
                    display: inline-block;
                    font-family: var(--font-mono, 'JetBrains Mono', monospace);
                    font-size: 0.75rem;
                    line-height: 1;
                    padding: 0.2rem 0.4rem;
                    background: var(--color-bg-secondary, #F3F0EC);
                    border: 1px solid var(--color-border, #D4CFC8);
                    border-bottom-width: 2px;
                    border-radius: var(--radius-sm, 3px);
                    color: var(--color-text-secondary, #4A4540);
                    white-space: nowrap;
                }
                .shortcut-desc {
                    color: var(--color-text-secondary, #4A4540);
                    font-size: 0.875rem;
                    line-height: 1.4;
                }

                /* Single-column on narrow screens */
                @media (max-width: 520px) {
                    .modal {
                        padding: var(--space-4, 1rem) var(--space-5, 1.25rem);
                        max-height: 85vh;
                    }
                    #content {
                        columns: 1;
                    }
                    .shortcut-key {
                        min-width: 4.5rem;
                    }
                }
            </style>
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
            if (e.key === 'Escape') this.hide();
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
