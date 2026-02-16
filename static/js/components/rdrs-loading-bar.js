// <rdrs-loading-bar> — Global loading indicator (Shadow DOM)

class RdrsLoadingBar extends HTMLElement {
    constructor() {
        super();
        this._count = 0;
        const shadow = this.attachShadow({ mode: 'open' });
        shadow.innerHTML = `
            <style>
                :host {
                    position: fixed;
                    top: 0;
                    left: 0;
                    right: 0;
                    height: 3px;
                    background: transparent;
                    z-index: 1001;
                    overflow: hidden;
                    opacity: 0;
                    transition: opacity 0.2s ease;
                }
                :host(.active) { opacity: 1; }
                .bar {
                    position: absolute;
                    top: 0;
                    left: -30%;
                    width: 30%;
                    height: 100%;
                    background: var(--color-accent, #0066cc);
                    animation: loading-slide 1.5s ease-in-out infinite;
                }
                @keyframes loading-slide {
                    0% { left: -30%; }
                    50% { left: 100%; }
                    100% { left: -30%; }
                }
            </style>
            <div class="bar"></div>
        `;
    }

    start() {
        this._count++;
        this.classList.add('active');
    }

    stop() {
        this._count = Math.max(0, this._count - 1);
        if (this._count === 0) {
            this.classList.remove('active');
        }
    }
}

customElements.define('rdrs-loading-bar', RdrsLoadingBar);

// Global proxy for backwards compatibility
window.loading = {
    get _el() {
        return document.querySelector('rdrs-loading-bar');
    },
    init() {
        // No-op: element is already in the DOM via template
    },
    start() {
        const el = this._el;
        if (el) el.start();
    },
    stop() {
        const el = this._el;
        if (el) el.stop();
    }
};
