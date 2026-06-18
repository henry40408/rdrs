// <rdrs-reading-chart> — tap/click a bar to highlight it and show an info card.
// Light DOM: enhances the server-rendered bars on /statistics. Pointer/click
// events cover mouse, touch, and pen with one code path; bars are focusable so
// Enter/Space work too. With JS disabled the static chart + native title remain.

class RdrsReadingChart extends HTMLElement {
    connectedCallback() {
        this.card = this.querySelector('[data-chart-card]');
        if (!this.card) return;
        this.cols = Array.from(this.querySelectorAll('.stats-bar-col'));
        this._active = null;

        this.cols.forEach((col) => {
            col.addEventListener('click', () => this._toggle(col));
            col.addEventListener('keydown', (e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    this._toggle(col);
                }
            });
        });

        // Tap the chart's empty area to dismiss.
        this.addEventListener('click', (e) => {
            if (e.target === this) this._clear();
        });
        this.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') this._clear();
        });
    }

    _toggle(col) {
        if (this._active === col) {
            this._clear();
        } else {
            this._show(col);
        }
    }

    _show(col) {
        if (this._active) this._active.classList.remove('is-active');
        this._active = col;
        col.classList.add('is-active');

        this.card.textContent = `${col.dataset.date} · ${col.dataset.count}`;
        // Anchor the card to the visible bar fill, not the full-height column —
        // otherwise it floats at the chart's top edge for every short bar.
        const wrapRect = this.getBoundingClientRect();
        const bar = col.querySelector('.stats-bar') || col;
        const barRect = bar.getBoundingClientRect();
        const left = barRect.left - wrapRect.left + barRect.width / 2;
        const top = barRect.top - wrapRect.top - 6;
        this.card.style.left = `${left}px`;
        this.card.style.top = `${top}px`;
        this.card.hidden = false;
    }

    _clear() {
        if (this._active) this._active.classList.remove('is-active');
        this._active = null;
        this.card.hidden = true;
    }
}

customElements.define('rdrs-reading-chart', RdrsReadingChart);
