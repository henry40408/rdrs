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

const ICON = {
  inbox: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.4 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.4-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.8 1.1z"/></svg>',
  star: '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l2.7 5.5 6 .9-4.3 4.2 1 6L12 17.3 6.6 20l1-6L3.3 9.9l6-.9z"/></svg>',
  sparkle: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l1.7 4.6L18 9.3l-4.3 1.7L12 16l-1.7-4.9L6 9.3l4.3-1.7z"/><path d="M18 15l.7 1.8L20.5 17.5l-1.8.7L18 20l-.7-1.8L15.5 17.5l1.8-.7z"/></svg>',
  list: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 6h13M8 12h13M8 18h13"/><path d="M3.5 6h.01M3.5 12h.01M3.5 18h.01"/></svg>',
  rss: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 11a9 9 0 0 1 9 9"/><path d="M4 4a16 16 0 0 1 16 16"/><circle cx="5" cy="19" r="1.6" fill="currentColor" stroke="none"/></svg>',
  folder: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  search: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>',
  barchart: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 20v-6M12 20V4M18 20v-9"/><path d="M4 20h16"/></svg>',
  user: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8" r="4"/><path d="M4.5 21a7.5 7.5 0 0 1 15 0"/></svg>',
  cog: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
  shield: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l8 3v5c0 5-3.5 8-8 9.5C7.5 19 4 16 4 11V6z"/></svg>',
  menu: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 6h18M3 12h18M3 18h18"/></svg>',
  close: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>',
};

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
                <span class="sidebar-item-icon">${ICON.shield}</span>
                <span>Admin</span>
            </a>` : '';

        this.innerHTML = `
<button class="sidebar-toggle" onclick="toggleSidebar()" aria-label="Open menu">${ICON.menu}</button>

<aside class="sidebar" id="sidebar" data-testid="main-nav">
    ${masqBanner}
    <div class="sidebar-header">
        <a href="/" class="sidebar-logo">rdrs</a>
        <button class="sidebar-close" onclick="closeSidebar()" aria-label="Close menu">${ICON.close}</button>
    </div>
    <nav class="sidebar-nav">
        <div class="sidebar-section">
            <a href="/" class="sidebar-item${isActive('unread')}" data-testid="nav-unread">
                <span class="sidebar-item-icon">${ICON.inbox}</span>
                <span>Unread</span>
                <span class="sidebar-badge" id="unread-count">${totalUnread > 0 ? totalUnread : ''}</span>
            </a>
            <a href="/entries/starred" class="sidebar-item${isActive('starred')}">
                <span class="sidebar-item-icon">${ICON.star}</span>
                <span>Starred</span>
            </a>
            <a href="/entries/summarized" class="sidebar-item${isActive('summarized')}" data-testid="nav-summarized">
                <span class="sidebar-item-icon">${ICON.sparkle}</span>
                <span>Summarized</span>
                <span class="sidebar-badge" id="summarized-count">${totalSummarized > 0 ? totalSummarized : ''}</span>
            </a>
            <a href="/entries" class="sidebar-item${['all', 'read', 'entries'].includes(active) ? ' active' : ''}" data-testid="nav-entries">
                <span class="sidebar-item-icon">${ICON.list}</span>
                <span>All Entries</span>
            </a>
        </div>
        ${categoriesHtml}
        <div class="sidebar-section">
            <a href="/feeds" class="sidebar-item${isActive('feeds')}" data-testid="nav-feeds">
                <span class="sidebar-item-icon">${ICON.rss}</span>
                <span>Feeds</span>
            </a>
            <a href="/categories" class="sidebar-item${isActive('categories')}" data-testid="nav-categories">
                <span class="sidebar-item-icon">${ICON.folder}</span>
                <span>Categories</span>
            </a>
        </div>
        <div class="sidebar-section">
            <a href="/search" class="sidebar-item${isActive('search')}" data-testid="nav-search">
                <span class="sidebar-item-icon">${ICON.search}</span>
                <span>Search</span>
            </a>
            <a href="/statistics" class="sidebar-item${isActive('statistics')}" data-testid="nav-statistics">
                <span class="sidebar-item-icon">${ICON.barchart}</span>
                <span>Statistics</span>
            </a>
            <a href="/user-settings" class="sidebar-item${isActive('user-settings')}" data-testid="nav-settings">
                <span class="sidebar-item-icon">${ICON.user}</span>
                <span>Settings</span>
            </a>
            <a href="/settings" class="sidebar-item${isActive('settings')}">
                <span class="sidebar-item-icon">${ICON.cog}</span>
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
                    const d = await r.json();
                    if (d.redirect_to.startsWith('/')) {
                        window.flash.redirect(d.redirect_to, 'info', 'You have been logged out.');
                    } else {
                        window.location.href = d.redirect_to;
                    }
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
