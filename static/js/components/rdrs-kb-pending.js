// <rdrs-kb-pending> — Keyboard shortcut pending indicator (Shadow DOM)

class RdrsKbPending extends HTMLElement {
    constructor() {
        super();
        const shadow = this.attachShadow({ mode: 'open' });
        shadow.innerHTML = `
            <style>
                :host {
                    display: none;
                    position: fixed;
                    bottom: var(--space-4, 1rem);
                    right: var(--space-4, 1rem);
                    background: var(--color-button-bg, #111111);
                    color: var(--color-button-text, #ffffff);
                    padding: var(--space-2, 0.5rem) var(--space-4, 1rem);
                    font-family: monospace;
                    font-size: var(--font-base, 1rem);
                    z-index: 999;
                }
                :host(.visible) {
                    display: block;
                }
            </style>
            <slot></slot>
        `;
    }

    show(text) {
        this.textContent = text;
        this.classList.add('visible');
    }

    hide() {
        this.classList.remove('visible');
    }
}

customElements.define('rdrs-kb-pending', RdrsKbPending);
