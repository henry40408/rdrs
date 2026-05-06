// <rdrs-statistics-page> — CSR replacement for the SSR statistics page.
// Renders the same DOM structure (and therefore same CSS hooks) the previous
// `templates/statistics.html` produced. Data comes from `GET /api/statistics`.

import { escapeHtml } from '/static/js/utils.js';

class RdrsStatisticsPage extends HTMLElement {
    connectedCallback() {
        const params = new URLSearchParams(window.location.search);
        this._period = params.get('period') || '7d';
        this._customFrom = params.get('from') || '';
        this._customTo = params.get('to') || '';
        this.render();
        this.fetchData();
    }

    render(data, errorMessage) {
        const active = data ? data.active_period : this._period;
        const customFrom = data ? data.custom_from : this._customFrom;
        const customTo = data ? data.custom_to : this._customTo;
        const isActive = (p) => p === active ? ' active' : '';

        const headerHtml = `
        <div class="stats-header">
            <h1>Statistics</h1>
            <form class="stats-period" method="get" action="/statistics">
                <a href="/statistics?period=7d" class="stats-period-btn${isActive('7d')}">7d</a>
                <a href="/statistics?period=30d" class="stats-period-btn${isActive('30d')}">30d</a>
                <a href="/statistics?period=90d" class="stats-period-btn${isActive('90d')}">90d</a>
                <a href="/statistics?period=all" class="stats-period-btn${isActive('all')}">All</a>
                <span class="stats-period-divider">|</span>
                <input type="hidden" name="period" value="custom">
                <input type="date" name="from" value="${escapeHtml(customFrom)}" class="stats-date-input">
                <span class="stats-period-dash">&mdash;</span>
                <input type="date" name="to" value="${escapeHtml(customTo)}" class="stats-date-input">
                <button type="submit" class="stats-period-btn">Apply</button>
            </form>
        </div>`;

        const sidebarHtml = `<rdrs-sidebar active="statistics"></rdrs-sidebar>`;

        let bodyHtml;
        if (errorMessage) {
            bodyHtml = `<p class="muted" data-testid="stats-error">${escapeHtml(errorMessage)}</p>`;
        } else if (!data) {
            bodyHtml = `<p class="muted" data-testid="stats-loading">Loading statistics&hellip;</p>`;
        } else {
            bodyHtml = this._renderContent(data);
        }

        this.innerHTML = `
<div class="app-layout">
${sidebarHtml}
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    ${headerHtml}
    ${bodyHtml}
    </div>
</main>
</div>`;
    }

    _renderContent(data) {
        const o = data.overview;
        const dailyMax = data.daily_read_counts.reduce((m, d) => Math.max(m, d.count), 0);
        const catMax = data.categories.reduce((m, c) => Math.max(m, c.count), 0);
        const feedMax = data.top_feeds.reduce((m, f) => Math.max(m, f.count), 0);

        const cards = `
        <div class="stats-cards">
            <div class="stats-card">
                <div class="stats-card-value">${o.total_entries}</div>
                <div class="stats-card-label">Total Entries</div>
            </div>
            <div class="stats-card">
                <div class="stats-card-value stats-card-success">${o.read_entries}</div>
                <div class="stats-card-label">Read</div>
            </div>
            <div class="stats-card">
                <div class="stats-card-value stats-card-warning">${o.unread_entries}</div>
                <div class="stats-card-label">Unread</div>
            </div>
            <div class="stats-card">
                <div class="stats-card-value">${o.read_rate.toFixed(1)}%</div>
                <div class="stats-card-label">Read Rate</div>
            </div>
            <div class="stats-card">
                <div class="stats-card-value">${o.starred_entries}</div>
                <div class="stats-card-label">Starred</div>
            </div>
            <div class="stats-card">
                <div class="stats-card-value">${o.summaries}</div>
                <div class="stats-card-label">Summaries</div>
            </div>
        </div>`;

        const dailyChart = `
        <div class="stats-section">
            <h2>Daily Read Articles</h2>
            ${dailyMax === 0 ? '<p class="muted">No read activity in this period</p>' : `
            <div class="stats-chart">
                ${data.daily_read_counts.map(d => {
                    const h = dailyMax > 0 ? (d.count * 100) / dailyMax : 0;
                    const md = d.date.length >= 10 ? `${d.date.slice(5, 7)}/${d.date.slice(8, 10)}` : d.date;
                    return `
                    <div class="stats-bar-col" title="${escapeHtml(d.date)}: ${d.count}">
                        <div class="stats-bar" style="height: ${h}%"></div>
                        <div class="stats-bar-label">${md}</div>
                    </div>`;
                }).join('')}
            </div>`}
        </div>`;

        const renderRows = (items, max) =>
            items.map(it => {
                const w = max > 0 ? (it.count * 100) / max : 0;
                const label = it.name != null ? it.name : it.title;
                return `
                <div class="stats-bar-row">
                    <div class="stats-bar-row-header">
                        <span>${escapeHtml(label)}</span>
                        <span class="muted">${it.count}</span>
                    </div>
                    <div class="stats-progress">
                        <div class="stats-progress-fill" style="width: ${w}%"></div>
                    </div>
                </div>`;
            }).join('');

        const columns = `
        <div class="stats-columns">
            <div class="stats-section">
                <h2>Entries by Category</h2>
                ${data.categories.length === 0
                    ? '<p class="muted">No entries in this period</p>'
                    : renderRows(data.categories, catMax)}
            </div>
            <div class="stats-section">
                <h2>Top Feeds</h2>
                ${data.top_feeds.length === 0
                    ? '<p class="muted">No entries in this period</p>'
                    : renderRows(data.top_feeds, feedMax)}
            </div>
        </div>`;

        const adminSection = data.admin ? `
        <div class="stats-admin-section">
            <h2>Site-wide Statistics</h2>
            <div class="stats-cards">
                <div class="stats-card stats-card-admin">
                    <div class="stats-card-value">${data.admin.total_users}</div>
                    <div class="stats-card-label">Total Users</div>
                </div>
                <div class="stats-card stats-card-admin">
                    <div class="stats-card-value">${data.admin.total_entries}</div>
                    <div class="stats-card-label">Site Entries</div>
                </div>
                <div class="stats-card stats-card-admin">
                    <div class="stats-card-value">${data.admin.total_feeds}</div>
                    <div class="stats-card-label">Total Feeds</div>
                </div>
                <div class="stats-card stats-card-admin">
                    <div class="stats-card-value">${data.admin.read_rate.toFixed(1)}%</div>
                    <div class="stats-card-label">Site Read Rate</div>
                </div>
            </div>
        </div>` : '';

        return cards + dailyChart + columns + adminSection;
    }

    async fetchData() {
        const qs = window.location.search || '';
        try {
            const resp = await fetch('/api/statistics' + qs, { credentials: 'same-origin' });
            if (!resp.ok) {
                this.render(undefined, `Failed to load statistics (${resp.status}).`);
                return;
            }
            const data = await resp.json();
            this.render(data);
        } catch (e) {
            this.render(undefined, 'Network error while loading statistics.');
        }
    }
}

customElements.define('rdrs-statistics-page', RdrsStatisticsPage);
