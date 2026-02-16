// <rdrs-modal> — Reusable modal component (Shadow DOM with slots)

class RdrsModal extends HTMLElement {
    constructor() {
        super();
        const shadow = this.attachShadow({ mode: 'open' });
        shadow.innerHTML = `
            <style>
                :host {
                    display: none;
                    position: fixed;
                    inset: 0;
                    background: var(--color-overlay, rgba(0, 0, 0, 0.5));
                    z-index: 1000;
                }
                :host([open]) {
                    display: block;
                }
                .content {
                    background: var(--color-bg, #ffffff);
                    max-width: 600px;
                    margin: var(--space-8, 2rem) auto;
                    padding: var(--space-8, 2rem);
                    border: 1px solid var(--color-border, #111111);
                    color: var(--color-text, #111111);
                }
            </style>
            <div class="content">
                <slot></slot>
            </div>
        `;

        // Close when clicking the overlay background
        shadow.addEventListener('click', (e) => {
            if (e.target === shadow.querySelector(':host') || e.composedPath()[0] === this.shadowRoot.host) {
                // Only close if clicking directly on host
            }
        });
        this.addEventListener('click', (e) => {
            if (e.target === this) this.close();
        });
    }

    open() {
        this.setAttribute('open', '');
    }

    close() {
        this.removeAttribute('open');
    }

    get isOpen() {
        return this.hasAttribute('open');
    }
}

customElements.define('rdrs-modal', RdrsModal);
