// <rdrs-sidebar active="statistics"> — CSR sidebar with category unread counts.
// Mirrors the SSR `macros.html::sidebar` macro DOM structure so existing CSS
// (sidebar-*, nav-* selectors) keeps working unchanged.
//
// Anti-flicker strategy:
//   1. The shell embeds the initial /api/sidebar payload as a JSON
//      `<script id="rdrs-sidebar-bootstrap">`. On every mount we read it
//      synchronously and paint — zero round trips, zero flash.
//   2. After every successful /api/sidebar fetch we rewrite that <script>'s
//      textContent and the sessionStorage mirror with the new payload, so
//      the next mount reads fresh data.
//   3. Background-revalidate via /api/sidebar after every mount, and surgically
//      patch the unread badges (full-rerender only if identity / category set
//      changed).
//
// Action paths that mutate unread/category state should call
// `document.querySelector('rdrs-sidebar')?.refresh()` so the bootstrap, the
// sessionStorage mirror, and the visible badges all advance together.

import { escapeHtml } from '/static/js/utils.js';

const SIDEBAR_CACHE_KEY = 'rdrs.sidebar.v1';

function readBootstrap() {
    const node = document.getElementById('rdrs-sidebar-bootstrap');
    if (!node || !node.textContent) return null;
    try {
        const parsed = JSON.parse(node.textContent);
        return parsed && typeof parsed === 'object' ? parsed : null;
    } catch { return null; }
}

function readCachedSidebar() {
    try {
        const raw = sessionStorage.getItem(SIDEBAR_CACHE_KEY);
        return raw ? JSON.parse(raw) : null;
    } catch { return null; }
}

function writeCachedSidebar(data) {
    const json = JSON.stringify(data);
    try { sessionStorage.setItem(SIDEBAR_CACHE_KEY, json); }
    catch { /* quota / disabled storage — fine */ }
    // Keep the embedded bootstrap <script> aligned with the latest payload
    // so subsequent mounts read the freshest state from a single source.
    const node = document.getElementById('rdrs-sidebar-bootstrap');
    if (node) node.textContent = json;
}

/// True when the difference between two sidebar payloads can't be expressed
/// by surgical badge updates alone — identity changed, masquerade/admin role
/// changed, or the category set was added/removed/renamed.
function isStructuralChange(prev, next) {
    if (prev.username !== next.username) return true;
    if (!!prev.is_admin !== !!next.is_admin) return true;
    if (!!prev.is_masquerading !== !!next.is_masquerading) return true;
    const key = (cats) => (cats || []).map((c) => `${c.id}:${c.name}`).join('|');
    return key(prev.categories) !== key(next.categories);
}

class RdrsSidebar extends HTMLElement {
    static get observedAttributes() { return ['active', 'active-category-id']; }

    connectedCallback() {
        const initial = readBootstrap() || readCachedSidebar();
        if (initial) {
            this._data = initial;
            writeCachedSidebar(initial);
            this.render(initial);
        }
        // No initial render on cold start — first paint waits for fetch.
        this.fetchData();
    }

    attributeChangedCallback() {
        if (this._data) this.render(this._data);
    }

    /// Public refresh hook for action paths that mutate state the sidebar
    /// reflects (mark-as-read, mark-unread, mark-all-as-read, etc). Re-fetches
    /// /api/sidebar so sessionStorage and the live badges both update.
    refresh() { return this.fetchData(); }

    async fetchData() {
        try {
            const resp = await fetch('/api/sidebar', { credentials: 'same-origin' });
            if (!resp.ok) return;
            const data = await resp.json();
            const prev = this._data;
            this._data = data;
            writeCachedSidebar(data);
            if (!prev || isStructuralChange(prev, data)) {
                this.render(data);
            } else {
                this._updateBadges(data);
            }
        } catch (e) { /* silent */ }
    }

    /// Surgical badge update — used when only unread counts changed. Avoids a
    /// full innerHTML rebuild so frequent mark-as-read clicks don't flash the
    /// whole sidebar.
    _updateBadges(data) {
        const totalEl = this.querySelector('#unread-count');
        if (totalEl) {
            const total = data.total_unread || 0;
            totalEl.textContent = total > 0 ? String(total) : '';
        }
        const sumEl = this.querySelector('#summarized-count');
        if (sumEl) {
            const sum = data.total_summarized || 0;
            sumEl.textContent = sum > 0 ? String(sum) : '';
        }
        const catContainer = this.querySelector('#sidebar-categories');
        if (!catContainer) return;
        for (const cat of data.categories || []) {
            const link = catContainer.querySelector(`a[href="/categories/${cat.id}/entries"]`);
            if (!link) continue;
            const existing = link.querySelector('.sidebar-badge');
            if (cat.unread_count > 0) {
                if (existing) {
                    existing.textContent = String(cat.unread_count);
                } else {
                    const span = document.createElement('span');
                    span.className = 'sidebar-badge';
                    span.textContent = String(cat.unread_count);
                    link.appendChild(span);
                }
            } else if (existing) {
                existing.remove();
            }
        }
    }

    render(data) {
        const active = this.getAttribute('active') || '';
        const activeCatId = parseInt(this.getAttribute('active-category-id') || '0', 10);
        const username = data ? data.username : '';
        const isAdmin = data ? !!data.is_admin : false;
        const isMasq = data ? !!data.is_masquerading : false;
        const cats = data ? data.categories : [];
        const totalUnread = data ? data.total_unread : 0;
        const totalSummarized = data ? data.total_summarized : 0;

        const isActive = (name) => active === name ? ' active' : '';

        const categoriesHtml = cats && cats.length > 0 ? `
        <div class="sidebar-section">
            <div class="sidebar-section-title">Categories</div>
            <div id="sidebar-categories">
                ${cats.map(cat => `
                <a href="/categories/${cat.id}/entries" class="sidebar-item${cat.id === activeCatId ? ' active' : ''}" title="${escapeHtml(cat.name)}">
                    <span class="sidebar-item-label">${escapeHtml(cat.name)}</span>
                    ${cat.unread_count > 0 ? `<span class="sidebar-badge">${cat.unread_count}</span>` : ''}
                </a>
                `).join('')}
            </div>
        </div>` : '';

        const masqBanner = isMasq ? `
            <div class="masquerade-banner">
                Viewing as another user &middot; <a href="#" data-rdrs-stop-masq>Stop</a>
            </div>` : '';

        const adminLink = isAdmin ? `
            <a href="/admin" class="sidebar-item${isActive('admin')}" data-testid="nav-admin">
                <span class="sidebar-item-icon">&#x1F6E1;&#xFE0F;</span>
                <span>Admin</span>
            </a>` : '';

        this.innerHTML = `
<button class="sidebar-toggle" onclick="toggleSidebar()" aria-label="Open menu">&#9776;</button>

<aside class="sidebar" id="sidebar" data-testid="main-nav">
    ${masqBanner}
    <div class="sidebar-header">
        <a href="/" class="sidebar-logo">rdrs</a>
        <button class="sidebar-close" onclick="closeSidebar()" aria-label="Close menu">&times;</button>
    </div>
    <nav class="sidebar-nav">
        <div class="sidebar-section">
            <a href="/" class="sidebar-item${isActive('unread')}" data-testid="nav-unread">
                <span class="sidebar-item-icon">&#x1F4EC;&#xFE0F;</span>
                <span>Unread</span>
                <span class="sidebar-badge" id="unread-count">${totalUnread > 0 ? totalUnread : ''}</span>
            </a>
            <a href="/entries/starred" class="sidebar-item${isActive('starred')}">
                <span class="sidebar-item-icon">&#x2B50;&#xFE0F;</span>
                <span>Starred</span>
            </a>
            <a href="/entries/summarized" class="sidebar-item${isActive('summarized')}" data-testid="nav-summarized">
                <span class="sidebar-item-icon">&#x2728;</span>
                <span>Summarized</span>
                <span class="sidebar-badge" id="summarized-count">${totalSummarized > 0 ? totalSummarized : ''}</span>
            </a>
            <a href="/entries" class="sidebar-item${['all', 'read', 'entries'].includes(active) ? ' active' : ''}" data-testid="nav-entries">
                <span class="sidebar-item-icon">&#x1F4F0;&#xFE0F;</span>
                <span>All Entries</span>
            </a>
        </div>
        ${categoriesHtml}
        <div class="sidebar-section">
            <a href="/feeds" class="sidebar-item${isActive('feeds')}" data-testid="nav-feeds">
                <span class="sidebar-item-icon">&#x1F4E1;&#xFE0F;</span>
                <span>Feeds</span>
            </a>
            <a href="/categories" class="sidebar-item${isActive('categories')}" data-testid="nav-categories">
                <span class="sidebar-item-icon">&#x1F5C2;&#xFE0F;</span>
                <span>Categories</span>
            </a>
        </div>
        <div class="sidebar-section">
            <a href="/search" class="sidebar-item${isActive('search')}" data-testid="nav-search">
                <span class="sidebar-item-icon">&#x1F50D;&#xFE0F;</span>
                <span>Search</span>
            </a>
            <a href="/statistics" class="sidebar-item${isActive('statistics')}" data-testid="nav-statistics">
                <span class="sidebar-item-icon">&#x1F4CA;&#xFE0F;</span>
                <span>Statistics</span>
            </a>
            <a href="/user-settings" class="sidebar-item${isActive('user-settings')}" data-testid="nav-settings">
                <span class="sidebar-item-icon">&#x1F464;&#xFE0F;</span>
                <span>Settings</span>
            </a>
            <a href="/settings" class="sidebar-item${isActive('settings')}">
                <span class="sidebar-item-icon">&#x2699;&#xFE0F;</span>
                <span>App</span>
            </a>
            ${adminLink}
        </div>
    </nav>
    <div class="sidebar-footer">
        <span class="sidebar-user">${escapeHtml(username)}</span>
        <a href="#" data-testid="logout-btn" data-rdrs-logout>Sign Out</a>
    </div>
</aside>`;

        this.querySelector('[data-rdrs-logout]')?.addEventListener('click', async (e) => {
            e.preventDefault();
            try {
                const r = await fetch('/api/session', { method: 'DELETE' });
                if (r.ok) {
                    window.flash.redirect('/login', 'info', 'You have been logged out.');
                } else {
                    window.flash.error('Logout failed');
                }
            } catch {
                window.flash.error('An error occurred during logout');
            }
        });

        this.querySelector('[data-rdrs-stop-masq]')?.addEventListener('click', async (e) => {
            e.preventDefault();
            try {
                const r = await fetch('/api/admin/unmasquerade', { method: 'POST' });
                if (r.ok) {
                    window.flash.success('Stopped masquerading.');
                    window.location.reload();
                } else {
                    const err = await r.json().catch(() => ({}));
                    window.flash.error(err.error || 'Failed to stop masquerade');
                }
            } catch {
                window.flash.error('An error occurred');
            }
        });
    }
}

customElements.define('rdrs-sidebar', RdrsSidebar);

document.addEventListener('click', (e) => {
    const sidebar = document.getElementById('sidebar');
    const toggle = document.querySelector('.sidebar-toggle');
    if (sidebar && sidebar.classList.contains('open') &&
        !sidebar.contains(e.target) && (!toggle || !toggle.contains(e.target))) {
        if (typeof closeSidebar === 'function') closeSidebar();
    }
});
