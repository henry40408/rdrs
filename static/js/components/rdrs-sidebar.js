// <rdrs-sidebar active="statistics"> — CSR sidebar with category unread counts.
//
// `render()` is the only definition of this markup (no Askama twin); its class
// names are what app.css styles. SSR was measured and rejected: markup is ~3.5x
// the bootstrap JSON, paid on every `no-store` load.
//
// Anti-flicker: paints synchronously from `<script id="rdrs-sidebar-bootstrap">`
// (and a sessionStorage mirror), then revalidates and patches badges in place.
// Feed rows are reconciled by id because WebKit paints a fresh `<img>` a frame
// late even from cache. Callers signal `rdrs:sidebar-stale` rather than calling
// `.refresh()`.

// `?v=` is substituted at serve time so this nested import is cache-busted.
import { escapeHtml } from '/static/js/utils.js?v=__RDRS_ASSET_VERSION__';

const ICON = {
  inbox: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.4 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.4-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.8 1.1z"/></svg>',
  star: '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3.5l2.7 5.5 6 .9-4.3 4.2 1 6L12 17.3 6.6 20l1-6L3.3 9.9l6-.9z"/></svg>',
  sparkle: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3l1.7 4.6L18 9.3l-4.3 1.7L12 16l-1.7-4.9L6 9.3l4.3-1.7z"/><path d="M18 15l.7 1.8L20.5 17.5l-1.8.7L18 20l-.7-1.8L15.5 17.5l1.8-.7z"/></svg>',
  wand: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M15 4V2M15 10V8M9 6H7M17 6h-2"/><path d="m3 21 12-12 3 3L6 24z" transform="translate(-3 -3)"/><path d="M12.5 6.5 17.5 11.5"/></svg>',
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
  download: '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v11"/><path d="m8 10.5 4 4 4-4"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/></svg>',
};

/// Connection lamp. Pure CSS reaction to `<html data-offline>` (owned by
/// offline.js), so re-renders can never leave a stale light. Colour is never the
/// only channel: offline shows the word; online keeps it for screen readers.
const CONNECTION_LAMP = `
        <span class="conn-status" role="status" data-testid="connection-status">
            <span class="conn-dot" aria-hidden="true"></span>
            <span class="conn-state conn-state--online sr-only">Online</span>
            <span class="conn-state conn-state--offline" data-testid="connection-offline">Offline</span>
        </span>`;

const SIDEBAR_CACHE_KEY = 'rdrs.sidebar.v1';

/// Per-category feed lists, mirrored so revisits paint instantly; revalidated right after.
const FEEDS_CACHE_KEY = 'rdrs.sidebar.feeds.v1';

/// Coalescing window for `rdrs:sidebar-stale`; absorbs the two an entry-open fires (~4 ms apart).
const STALE_COALESCE_MS = 50;

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
    // Keeps subsequent mounts reading the freshest state from one source.
    const node = document.getElementById('rdrs-sidebar-bootstrap');
    if (node) node.textContent = json;
}

function readCachedFeeds() {
    try {
        const raw = sessionStorage.getItem(FEEDS_CACHE_KEY);
        const parsed = raw ? JSON.parse(raw) : null;
        return parsed && typeof parsed === 'object' ? parsed : {};
    } catch { return {}; }
}

function writeCachedFeeds(byCategory) {
    try { sessionStorage.setItem(FEEDS_CACHE_KEY, JSON.stringify(byCategory)); }
    catch { /* quota / disabled storage — fine */ }
}

/// Mirrors `feed_initial()` / `feed_color_index()` in handlers/pages.
function feedInitial(feed) { return (Array.from(feed.title || '')[0] || '?').toUpperCase(); }

function feedColorIndex(feed) { return ((feed.id % 6) + 6) % 6; }

/// Stamped as `data-favicon-key` so a refresh leaves an unchanged `<img>` alone
/// (recreating it blinks in WebKit).
function feedFaviconKey(feed) {
    return feed.has_icon ? `img:${feed.id}` : `chip:${feedColorIndex(feed)}:${feedInitial(feed)}`;
}

/// Real icon if any, else the initial-letter chip; built as a node for in-place reconciling.
function buildFeedFavicon(feed) {
    let node;
    if (feed.has_icon) {
        node = document.createElement('img');
        node.className = 'entry-favicon';
        node.src = `/api/feeds/${feed.id}/icon`;
        node.alt = '';
        // No lazy loading and sync decode, matching `_entry_row.html`: otherwise icons blink on iOS.
        node.decoding = 'sync';
        node.width = 15;
        node.height = 15;
    } else {
        node = document.createElement('span');
        node.className = `entry-favicon entry-favicon-chip fav-c${feedColorIndex(feed)}`;
        node.setAttribute('aria-hidden', 'true');
        node.textContent = feedInitial(feed);
    }
    node.dataset.faviconKey = feedFaviconKey(feed);
    return node;
}

/// The reader's sidebar display preferences; defaults match the server's.
function sidebarPrefs(data) {
    return {
        sort: data?.sidebar_sort === 'unread' ? 'unread' : 'name',
        hideRead: !!data?.sidebar_hide_read,
    };
}

/// Apply those preferences to category or feed rows. `keepId` (the open row)
/// stays listed at zero unread so it doesn't vanish under the cursor. Input is
/// name-ordered; 'unread' does a stable re-sort of a copy.
function arrangeSidebarRows(rows, prefs, keepId) {
    let out = rows || [];
    if (prefs.hideRead) {
        out = out.filter((r) => r.unread_count > 0 || r.id === keepId);
    }
    if (prefs.sort === 'unread') {
        out = out.slice().sort((a, b) => b.unread_count - a.unread_count);
    }
    return out;
}

/// Whether this reader keeps an offline library, from `<html data-offline-keep>`;
/// it only changes across full page loads, unlike the revalidated payload.
function offlineLibraryKept() {
    const keep = Number.parseInt(document.documentElement.dataset.offlineKeep || '0', 10);
    return Number.isFinite(keep) && keep > 0;
}

/// Shared by `render()` and `_applyActive()` so they can't diverge. "All Entries"
/// stays lit across the /entries family.
function navIsActive(nav, active) {
    if (nav === 'entries') return ['all', 'read', 'entries'].includes(active);
    return nav === active;
}

/// True when a payload change needs a full render, not just badge updates.
function isStructuralChange(prev, next) {
    if (prev.username !== next.username) return true;
    if (!!prev.is_admin !== !!next.is_admin) return true;
    if (!!prev.is_masquerading !== !!next.is_masquerading) return true;
    if (!!prev.via_forward_auth !== !!next.via_forward_auth) return true;
    if (prev.sidebar_sort !== next.sidebar_sort) return true;
    if (!!prev.sidebar_hide_read !== !!next.sidebar_hide_read) return true;
    const key = (cats) => (cats || []).map((c) => `${c.id}:${c.name}`).join('|');
    if (key(prev.categories) !== key(next.categories)) return true;
    // With fully-read groups hidden, reaching/leaving zero adds or removes a row.
    if (next.sidebar_hide_read) {
        const shown = (cats) => (cats || []).filter((c) => c.unread_count > 0)
            .map((c) => c.id).join('|');
        if (shown(prev.categories) !== shown(next.categories)) return true;
    }
    // Reordering by unread is deliberately not structural: rows must not move
    // mid-click. It settles on the next full render.
    return false;
}

class RdrsSidebar extends HTMLElement {
    static get observedAttributes() { return ['active', 'active-category-id', 'active-feed-id']; }

    constructor() {
        super();
        // Bound once so connect/disconnect add and remove the *same* reference.
        this._onDocumentClick = this._onDocumentClick.bind(this);
        this._onStale = this._onStale.bind(this);
        // category id -> feed list, for categories opened this session.
        this._feeds = readCachedFeeds();
    }

    connectedCallback() {
        const initial = readBootstrap() || readCachedSidebar();
        if (initial) {
            this._data = initial;
            writeCachedSidebar(initial);
            this.render(initial);
        }
        // No initial render on cold start — first paint waits for fetch.
        this.fetchData();
        this.fetchFeeds();

        // The scrim is a pseudo-element, so tap-outside listens on the document.
        document.addEventListener('click', this._onDocumentClick);
        document.addEventListener('rdrs:sidebar-stale', this._onStale);
    }

    disconnectedCallback() {
        document.removeEventListener('click', this._onDocumentClick);
        document.removeEventListener('rdrs:sidebar-stale', this._onStale);
        clearTimeout(this._staleTimer);
        this._staleTimer = null;
        this._abort?.abort();
        this._abort = null;
    }

    attributeChangedCallback(name, oldValue, newValue) {
        // Patch `.active` instead of re-rendering, which would reset `.sidebar-nav`'s scroll.
        this._applyActive();
        // A new active category needs its feed list (loaded on demand).
        if (name === 'active-category-id' && oldValue !== newValue) {
            this._renderFeeds();
            this.fetchFeeds();
        }
    }

    /// Repaint `.active` from the current attributes; no-op before first render.
    _applyActive() {
        const active = this.getAttribute('active') || '';
        const activeCatId = this.activeCategoryId;
        const activeFeedId = this.activeFeedId;
        for (const item of this.querySelectorAll('.sidebar-item[data-nav]')) {
            item.classList.toggle('active', navIsActive(item.dataset.nav, active));
        }
        for (const item of this.querySelectorAll('#sidebar-categories .sidebar-item')) {
            const id = parseInt(item.dataset.categoryId || '0', 10);
            item.classList.toggle('active', id !== 0 && id === activeCatId);
        }
        for (const item of this.querySelectorAll('.sidebar-feed[data-feed-id]')) {
            const id = parseInt(item.dataset.feedId || '0', 10);
            item.classList.toggle('active', id !== 0 && id === activeFeedId);
        }
    }

    get activeCategoryId() { return parseInt(this.getAttribute('active-category-id') || '0', 10); }

    get activeFeedId() { return parseInt(this.getAttribute('active-feed-id') || '0', 10); }

    /// Categories in render order (minus hidden rows), or []. Used by app.js's
    /// `[` / `]` navigation to step through exactly what is on screen.
    get categories() {
        return arrangeSidebarRows(this._data?.categories, sidebarPrefs(this._data),
            this.activeCategoryId);
    }

    /// Feeds of the active category in render order, or []. Also for `[` / `]`.
    get activeFeeds() {
        const catId = this.activeCategoryId;
        if (!catId) return [];
        return arrangeSidebarRows(this._feeds[catId], sidebarPrefs(this._data),
            this.activeFeedId);
    }

    /// A feed's category if known this session; `null` means unknown.
    categoryIdOfFeed(feedId) {
        const wanted = parseInt(feedId, 10);
        for (const [catId, feeds] of Object.entries(this._feeds)) {
            if (feeds.some((f) => f.id === wanted)) return parseInt(catId, 10);
        }
        return null;
    }

    /// Only the open category's feeds are fetched, so large accounts don't pay
    /// for every subscription on each load.
    async fetchFeeds(options = {}) {
        const catId = this.activeCategoryId;
        if (!catId) return;
        // First mount asks twice (attributeChanged + connected); don't abort for
        // the same answer. A `force` revalidation still supersedes.
        if (this._feedsInFlightFor === catId && !options.force) return;
        this._feedsInFlightFor = catId;
        this._feedsAbort?.abort();
        const controller = new AbortController();
        this._feedsAbort = controller;
        try {
            const resp = await fetch(`/api/sidebar/categories/${catId}/feeds`, {
                credentials: 'same-origin',
                signal: controller.signal,
            });
            if (!resp.ok) return;
            const data = await resp.json();
            // The reader may have moved on; this response is for `categoryId`.
            if (data.category_id !== this.activeCategoryId) return;
            this._feeds[data.category_id] = data.feeds || [];
            writeCachedFeeds(this._feeds);
            this._renderFeeds();
        } catch { /* silent — includes the AbortError from being superseded */ }
        finally {
            if (this._feedsAbort === controller) {
                this._feedsAbort = null;
                this._feedsInFlightFor = null;
            }
        }
    }

    /// Imperative escape hatch to await a refetch; `rdrs:sidebar-stale` is the normal path.
    refresh() { return this.fetchData(); }

    /// Coalesced trailing-edge refresh: one interaction often raises
    /// `rdrs:sidebar-stale` twice (swap-complete, then SSE).
    _onStale() {
        clearTimeout(this._staleTimer);
        this._staleTimer = setTimeout(() => {
            this._staleTimer = null;
            this.fetchData();
            // Feed badges come from a different endpoint.
            this.fetchFeeds({ force: true });
        }, STALE_COALESCE_MS);
    }

    async fetchData() {
        // Otherwise overlapping responses can land out of order.
        this._abort?.abort();
        const controller = new AbortController();
        this._abort = controller;
        try {
            const resp = await fetch('/api/sidebar', {
                credentials: 'same-origin',
                signal: controller.signal,
            });
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
        } catch (e) { /* silent — includes the AbortError from being superseded */ }
        finally {
            if (this._abort === controller) this._abort = null;
        }
    }

    /// Mobile drawer open/close; the hamburger hides while open.
    toggleDrawer() {
        const sidebar = this.querySelector('#sidebar');
        if (!sidebar) return;
        const toggle = this.querySelector('.sidebar-toggle');
        sidebar.classList.toggle('open');
        if (toggle) toggle.style.display = sidebar.classList.contains('open') ? 'none' : '';
    }

    closeDrawer() {
        const sidebar = this.querySelector('#sidebar');
        const toggle = this.querySelector('.sidebar-toggle');
        if (sidebar) sidebar.classList.remove('open');
        if (toggle) toggle.style.display = '';
    }

    _onDocumentClick(e) {
        const sidebar = this.querySelector('#sidebar');
        if (!sidebar || !sidebar.classList.contains('open')) return;
        if (!(e.target instanceof Element)) return;
        // `closest()`: render() may have detached the clicked node.
        if (e.target.closest('#sidebar') || e.target.closest('.sidebar-toggle')) return;
        this.closeDrawer();
    }

    /// Uses `textContent`, so nothing to escape.
    _buildFeedRow(feed) {
        const row = document.createElement('a');
        row.className = 'sidebar-feed';
        row.href = `/feeds/${feed.id}/entries`;
        row.dataset.feedId = String(feed.id);
        row.appendChild(buildFeedFavicon(feed));
        const label = document.createElement('span');
        label.className = 'sidebar-item-label';
        row.appendChild(label);
        this._patchFeedRow(row, feed);
        return row;
    }

    /// Touch only what differs; favicons compared by `data-favicon-key` to avoid blinking.
    _patchFeedRow(row, feed) {
        const title = feed.title || '';
        if (row.title !== title) row.title = title;
        const favicon = row.firstElementChild;
        if (!favicon || favicon.dataset.faviconKey !== feedFaviconKey(feed)) {
            const next = buildFeedFavicon(feed);
            if (favicon) row.replaceChild(next, favicon);
            else row.prepend(next);
        }
        const label = row.querySelector('.sidebar-item-label');
        if (label && label.textContent !== title) label.textContent = title;
        const badge = row.querySelector('.sidebar-badge');
        if (feed.unread_count > 0) {
            const text = String(feed.unread_count);
            if (badge) {
                if (badge.textContent !== text) badge.textContent = text;
            } else {
                const span = document.createElement('span');
                span.className = 'sidebar-badge';
                span.textContent = text;
                row.appendChild(span);
            }
        } else if (badge) {
            badge.remove();
        }
        row.classList.toggle('active', feed.id === this.activeFeedId);
    }

    /// Mount or refresh the open category's feed list in the existing DOM (a full
    /// render resets `.sidebar-nav` scroll). Rows reconciled by feed id so WebKit
    /// doesn't blink their `<img>` on every `rdrs:sidebar-stale`.
    _renderFeeds() {
        const container = this.querySelector('#sidebar-categories');
        if (!container) return;
        const catId = this.activeCategoryId;
        for (const list of container.querySelectorAll('.sidebar-feeds')) {
            if (parseInt(list.dataset.categoryId || '0', 10) !== catId) list.remove();
        }
        const detached = this._detachedFeeds;
        this._detachedFeeds = null;
        if (!catId) return;
        const link = container.querySelector(`a[data-category-id="${catId}"]`);
        const feeds = this._feeds[catId];
        // Leave the gap rather than flash an empty group; fetchFeeds() calls back.
        if (!link || !feeds) return;
        let list = container.querySelector(`.sidebar-feeds[data-category-id="${catId}"]`);
        // Re-adopt the list render() set aside, keeping loaded icons.
        if (!list && detached
            && parseInt(detached.dataset.categoryId || '0', 10) === catId) {
            list = detached;
        }
        if (!list) {
            list = document.createElement('div');
            list.className = 'sidebar-feeds';
            list.dataset.categoryId = String(catId);
        }
        if (list.parentNode !== link.parentNode || list.previousElementSibling !== link) {
            link.insertAdjacentElement('afterend', list);
        }

        const rows = new Map();
        for (const row of list.children) {
            if (row.dataset.feedId) rows.set(row.dataset.feedId, row);
        }
        let cursor = null;
        const wanted = arrangeSidebarRows(feeds, sidebarPrefs(this._data), this.activeFeedId);
        for (const feed of wanted) {
            const key = String(feed.id);
            let row = rows.get(key);
            if (row) {
                rows.delete(key);
                this._patchFeedRow(row, feed);
            } else {
                row = this._buildFeedRow(feed);
            }
            // Only move misplaced rows, so reordering doesn't reattach everything after.
            const at = cursor ? cursor.nextSibling : list.firstChild;
            if (at !== row) list.insertBefore(row, at);
            cursor = row;
        }
        for (const row of rows.values()) row.remove();
    }

    /// Surgical badge update, so mark-as-read doesn't flash the whole sidebar.
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
        const viaForwardAuth = data ? !!data.via_forward_auth : false;
        const cats = arrangeSidebarRows(data?.categories, sidebarPrefs(data), activeCatId);
        const totalUnread = data ? data.total_unread : 0;
        const totalSummarized = data ? data.total_summarized : 0;

        const isActive = (name) => navIsActive(name, active) ? ' active' : '';

        const categoriesHtml = cats && cats.length > 0 ? `
        <div class="sidebar-section">
            <div class="sidebar-section-title">Categories</div>
            <div id="sidebar-categories">
                ${cats.map(cat => `
                <a href="/categories/${cat.id}/entries" class="sidebar-item${cat.id === activeCatId ? ' active' : ''}" data-category-id="${cat.id}" title="${escapeHtml(cat.name)}">
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

        // Admin-only server-side; hide for regular accounts.
        const appSettingsLink = isAdmin ? `
            <a href="/settings" class="sidebar-item${isActive('settings')}" data-nav="settings" data-testid="nav-app-settings">
                <span class="sidebar-item-icon">${ICON.cog}</span>
                <span>App</span>
            </a>` : '';

        // The only route to the offline library (everything else needs the server).
        // Hidden at `offline_keep = 0`, where it would always be empty.
        const offlineLink = offlineLibraryKept() ? `
            <a href="/entries/offline" class="sidebar-item${isActive('offline')}" data-nav="offline" data-testid="nav-offline">
                <span class="sidebar-item-icon">${ICON.download}</span>
                <span>Offline</span>
            </a>` : '';

        const adminLink = isAdmin ? `
            <a href="/admin" class="sidebar-item${isActive('admin')}" data-nav="admin" data-testid="nav-admin">
                <span class="sidebar-item-icon">${ICON.shield}</span>
                <span>Admin</span>
            </a>` : '';

        // Detach the open category's feed list so `_renderFeeds()` can re-adopt
        // it; otherwise icons blink when hide-read makes mark-as-read structural.
        const openFeeds = activeCatId
            ? this.querySelector(`.sidebar-feeds[data-category-id="${activeCatId}"]`)
            : null;
        openFeeds?.remove();
        this._detachedFeeds = openFeeds;

        // Carry `.sidebar-nav` scroll across the rebuild (e.g. "Mark Above as Read"
        // with hide-read on used to jump to the top).
        const navOffset = this.querySelector('.sidebar-nav')?.scrollTop ?? 0;

        this.innerHTML = `
<button class="sidebar-toggle" type="button" aria-label="Open menu">${ICON.menu}</button>

<aside class="sidebar" id="sidebar" data-testid="main-nav">
    ${masqBanner}
    <div class="sidebar-header">
        <a href="/" class="sidebar-logo">rdrs</a>
        ${CONNECTION_LAMP}
        <button class="sidebar-close" type="button" aria-label="Close menu">${ICON.close}</button>
    </div>
    <nav class="sidebar-nav">
        <div class="sidebar-section">
            <a href="/" class="sidebar-item${isActive('unread')}" data-nav="unread" data-testid="nav-unread">
                <span class="sidebar-item-icon">${ICON.inbox}</span>
                <span>Unread</span>
                <span class="sidebar-badge" id="unread-count">${totalUnread > 0 ? totalUnread : ''}</span>
            </a>
            <a href="/entries/starred" class="sidebar-item${isActive('starred')}" data-nav="starred">
                <span class="sidebar-item-icon">${ICON.star}</span>
                <span>Starred</span>
            </a>
            <a href="/entries/summarized" class="sidebar-item${isActive('summarized')}" data-nav="summarized" data-testid="nav-summarized">
                <span class="sidebar-item-icon">${ICON.sparkle}</span>
                <span>Summarized</span>
                <span class="sidebar-badge" id="summarized-count">${totalSummarized > 0 ? totalSummarized : ''}</span>
            </a>
            <a href="/entries" class="sidebar-item${isActive('entries')}" data-nav="entries" data-testid="nav-entries">
                <span class="sidebar-item-icon">${ICON.list}</span>
                <span>All Entries</span>
            </a>${offlineLink}
        </div>
        ${categoriesHtml}
        <div class="sidebar-section">
            <a href="/feeds" class="sidebar-item${isActive('feeds')}" data-nav="feeds" data-testid="nav-feeds">
                <span class="sidebar-item-icon">${ICON.rss}</span>
                <span>Feeds</span>
            </a>
            <a href="/categories" class="sidebar-item${isActive('categories')}" data-nav="categories" data-testid="nav-categories">
                <span class="sidebar-item-icon">${ICON.folder}</span>
                <span>Categories</span>
            </a>
        </div>
        <div class="sidebar-section">
            <a href="/summarizer" class="sidebar-item${isActive('summarizer')}" data-nav="summarizer" data-testid="nav-summarizer">
                <span class="sidebar-item-icon">${ICON.wand}</span>
                <span>Summarizer</span>
            </a>
            <a href="/search" class="sidebar-item${isActive('search')}" data-nav="search" data-testid="nav-search">
                <span class="sidebar-item-icon">${ICON.search}</span>
                <span>Search</span>
            </a>
            <a href="/statistics" class="sidebar-item${isActive('statistics')}" data-nav="statistics" data-testid="nav-statistics">
                <span class="sidebar-item-icon">${ICON.barchart}</span>
                <span>Statistics</span>
            </a>
            <a href="/user-settings" class="sidebar-item${isActive('user-settings')}" data-nav="user-settings" data-testid="nav-settings">
                <span class="sidebar-item-icon">${ICON.user}</span>
                <span>Settings</span>
            </a>
            ${appSettingsLink}
            ${adminLink}
        </div>
    </nav>
    <div class="sidebar-footer">
        <span class="sidebar-id">
            <span class="sidebar-user">${escapeHtml(username)}</span>
            ${viaForwardAuth ? '<span class="sidebar-auth-pill" data-testid="auth-pill">SSO</span>' : ''}
        </span>
        <a href="#" data-testid="logout-btn" data-rdrs-logout>Sign Out</a>
    </div>
</aside>`;

        // The rebuild also discarded the open category's feed list.
        this._renderFeeds();

        // After `_renderFeeds()`, or the offset is clamped to a shorter list.
        if (navOffset > 0) {
            const nav = this.querySelector('.sidebar-nav');
            if (nav) nav.scrollTop = navOffset;
        }

        // innerHTML above discards the previous subtree along with its listeners.
        this.querySelector('.sidebar-toggle')?.addEventListener('click', () => this.toggleDrawer());
        this.querySelector('.sidebar-close')?.addEventListener('click', () => this.closeDrawer());

        this.querySelector('[data-rdrs-logout]')?.addEventListener('click', async (e) => {
            e.preventDefault();
            // Forward-auth session already ended locally; a second click would 401.
            if (this._proxyLogoutNotified) return;
            try {
                const r = await fetch('/api/session', { method: 'DELETE' });
                if (r.ok) {
                    const d = await r.json();
                    if (d.logout_url_configured) {
                        // Hand off to the proxy/SSO so the upstream session ends.
                        window.location.href = d.redirect_to;
                    } else if (d.via_forward_auth) {
                        // No logout URL: the proxy would re-inject identity, so say
                        // so rather than silently re-authenticate via /login.
                        this._proxyLogoutNotified = true;
                        window.flash.warning('You are signed in via your reverse proxy. To end your session, log out at your proxy or SSO provider, then reload this page to keep using the app.');
                    } else {
                        // Normal cookie/password session: fully logged out server-side.
                        window.flash.redirect(d.redirect_to);
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
