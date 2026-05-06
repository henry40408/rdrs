// <rdrs-sidebar active="statistics"> — CSR sidebar with category unread counts.
// Mirrors the SSR `macros.html::sidebar` macro DOM structure so existing CSS
// (sidebar-*, nav-* selectors) keeps working unchanged.

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text == null ? '' : String(text);
    return div.innerHTML;
}

class RdrsSidebar extends HTMLElement {
    static get observedAttributes() { return ['active', 'active-category-id']; }

    connectedCallback() {
        this.render();
        this.fetchData();
    }

    attributeChangedCallback() {
        if (this._data) this.render(this._data);
    }

    async fetchData() {
        try {
            const resp = await fetch('/api/sidebar', { credentials: 'same-origin' });
            if (!resp.ok) return;
            this._data = await resp.json();
            this.render(this._data);
        } catch (e) { /* silent */ }
    }

    render(data) {
        const active = this.getAttribute('active') || '';
        const activeCatId = parseInt(this.getAttribute('active-category-id') || '0', 10);
        const username = data ? data.username : '';
        const isAdmin = data ? !!data.is_admin : false;
        const isMasq = data ? !!data.is_masquerading : false;
        const cats = data ? data.categories : [];
        const totalUnread = data ? data.total_unread : 0;

        const isActive = (name) => active === name ? ' active' : '';

        const categoriesHtml = cats && cats.length > 0 ? `
        <div class="sidebar-section">
            <div class="sidebar-section-title">Categories</div>
            <div id="sidebar-categories">
                ${cats.map(cat => `
                <a href="/categories/${cat.id}/entries" class="sidebar-item${cat.id === activeCatId ? ' active' : ''}">
                    <span>${escapeHtml(cat.name)}</span>
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
            <a href="/entries" class="sidebar-item${isActive('entries')}" data-testid="nav-entries">
                <span class="sidebar-item-icon">&#x1F4F0;&#xFE0F;</span>
                <span>All Entries</span>
            </a>
        </div>
        ${categoriesHtml}
        <div class="sidebar-divider"></div>
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
        <div class="sidebar-divider"></div>
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
