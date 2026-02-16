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
                }
                :host(.visible) {
                    display: flex;
                }
                .modal {
                    background: var(--color-bg, #ffffff);
                    border: 2px solid var(--color-border, #111111);
                    padding: var(--space-6, 1.5rem);
                    max-width: 600px;
                    max-height: 80vh;
                    overflow-y: auto;
                    font-size: 0.9rem;
                    color: var(--color-text, #111111);
                }
                h2 {
                    font-size: 1rem;
                    margin-bottom: var(--space-4, 1rem);
                    border-bottom: 1px solid var(--color-border, #111111);
                    padding-bottom: var(--space-2, 0.5rem);
                }
                button {
                    margin-bottom: var(--space-4, 1rem);
                    padding: var(--space-2, 0.5rem) var(--space-4, 1rem);
                    border-radius: var(--radius-md, 4px);
                    background: var(--color-button-bg, #111111);
                    color: var(--color-button-text, #ffffff);
                    border: 1px solid var(--color-border, #111111);
                    font-family: inherit;
                    font-size: inherit;
                    cursor: pointer;
                }
                button:hover {
                    background: var(--color-bg, #ffffff);
                    color: var(--color-text, #111111);
                }
                #content h3 {
                    font-size: 0.9rem;
                    margin: var(--space-4, 1rem) 0 var(--space-2, 0.5rem);
                    color: var(--color-text-muted, #666666);
                }
                table {
                    width: 100%;
                    margin-bottom: var(--space-4, 1rem);
                    border-collapse: collapse;
                }
                td {
                    padding: var(--space-1, 0.25rem) 0;
                    border: none;
                }
                td:first-child {
                    width: 80px;
                    font-family: monospace;
                    font-weight: bold;
                }
            </style>
            <div class="modal">
                <h2>Keyboard Shortcuts</h2>
                <button id="close-btn">Close</button>
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

    show(helpItems) {
        const content = this.shadowRoot.getElementById('content');
        let html = '';

        // Global shortcuts
        html += '<h3>Global</h3><table>';
        html += '<tr><td>g h</td><td>Go to Unread</td></tr>';
        html += '<tr><td>g e</td><td>Go to Entries</td></tr>';
        html += '<tr><td>g s</td><td>Go to Search</td></tr>';
        html += '<tr><td>?</td><td>Toggle this help</td></tr>';
        html += '<tr><td>Esc</td><td>Close help / Blur input</td></tr>';
        html += '</table>';

        // Page-specific shortcuts
        if (helpItems && helpItems.length > 0) {
            html += '<h3>Page Shortcuts</h3><table>';
            helpItems.forEach(item => {
                html += `<tr><td>${item.key}</td><td>${item.desc}</td></tr>`;
            });
            html += '</table>';
        }

        content.innerHTML = html;
        this.classList.add('visible');

        // Focus close button
        this.shadowRoot.getElementById('close-btn').focus();
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
