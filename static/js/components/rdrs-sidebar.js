// <rdrs-sidebar active="statistics"> — CSR sidebar with category unread counts.
//
// `render()` below is the *only* definition of the sidebar's markup; there is
// no Askama counterpart. (An earlier `macros.html::sidebar` macro is gone — the
// comment claiming this file mirrors it outlived the macro by some margin.) The
// class names it emits are what `static/css/app.css` styles, so the two move
// together: a rename here without one there silently unstyles the sidebar.
//
// Server-rendering it instead was measured and rejected — the bootstrap JSON is
// 180 B brotli against 653 B for equivalent markup, and logged-in responses are
// `no-store`, so that difference is paid on every page load rather than once.
// Badge-only updates would also regress from patching textContent to refetching
// a fragment. See the sidebar-SSR notes if this comes up again.
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
// Action paths that mutate unread/category state announce it with
// `document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'))` — the element
// subscribes while connected and refetches, so the bootstrap, the
// sessionStorage mirror, and the visible badges all advance together. Callers
// don't need to know whether a sidebar is mounted.

// `?v=` is substituted at serve time (see handlers/static_assets.rs) so this
// nested import is cache-busted like the top-level <script> tags.
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
};

const SIDEBAR_CACHE_KEY = 'rdrs.sidebar.v1';

/// Per-category feed lists, mirrored so revisiting a category paints its feeds
/// from the last known state instead of an empty gap while the fetch runs.
/// Same trade as the main payload's mirror: possibly one interaction stale,
/// corrected by the revalidation that follows immediately.
const FEEDS_CACHE_KEY = 'rdrs.sidebar.feeds.v1';

/// Window over which repeated `rdrs:sidebar-stale` signals collapse into a
/// single fetch. Long enough to absorb the two that one entry-open produces
/// (measured ~4 ms apart), short enough that the badges still settle within a
/// frame or two of the click.
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
    // Keep the embedded bootstrap <script> aligned with the latest payload
    // so subsequent mounts read the freshest state from a single source.
    const node = document.getElementById('rdrs-sidebar-bootstrap');
    if (node) node.textContent = json;
}

/// True when the difference between two sidebar payloads can't be expressed
/// by surgical badge updates alone — identity changed, masquerade/admin role
/// changed, or the category set was added/removed/renamed.
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

/// Favicon for a sidebar feed row: the real icon when the server says there is
/// one, otherwise the initial-letter chip. Mirrors `_entry_row.html` —
/// including `feed_initial()` (first character, uppercased) and
/// `feed_color_index()` (feed id modulo the six-colour palette) from
/// handlers/pages — so the same feed wears the same mark in both places.
function feedFavicon(feed) {
    if (feed.has_icon) {
        return `<img class="entry-favicon" src="/api/feeds/${feed.id}/icon" alt="" loading="lazy" width="15" height="15">`;
    }
    const initial = (Array.from(feed.title || '')[0] || '?').toUpperCase();
    const color = ((feed.id % 6) + 6) % 6;
    return `<span class="entry-favicon entry-favicon-chip fav-c${color}" aria-hidden="true">${escapeHtml(initial)}</span>`;
}

/// The reader's sidebar display preferences, read out of an `/api/sidebar`
/// payload. Defaults match the server's, so a payload from before these
/// settings existed (a stale sessionStorage mirror, say) behaves as it did.
function sidebarPrefs(data) {
    return {
        sort: data?.sidebar_sort === 'unread' ? 'unread' : 'name',
        hideRead: !!data?.sidebar_hide_read,
    };
}

/// Apply those preferences to a list of category or feed rows.
///
/// `keepId` is the row the reader currently has open. It stays listed even at
/// zero unread: hiding it would make the category or feed vanish from under
/// the cursor the moment its last entry is marked read — while the reader is
/// still looking at it.
///
/// The server always sends these lists complete and in name order, so 'name'
/// is a no-op here and 'unread' re-sorts a copy. `Array.prototype.sort` is
/// stable, so equal counts keep that A-Z order rather than shuffling.
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

/// Whether the nav item named `nav` is the active one for the page-level
/// `active` attribute. Shared by `render()` and `_applyActive()` so the class
/// a fresh render paints and the one an attribute change patches can't diverge.
/// "All Entries" is the odd one out: it stays lit across the /entries family.
function navIsActive(nav, active) {
    if (nav === 'entries') return ['all', 'read', 'entries'].includes(active);
    return nav === active;
}

function isStructuralChange(prev, next) {
    if (prev.username !== next.username) return true;
    if (!!prev.is_admin !== !!next.is_admin) return true;
    if (!!prev.is_masquerading !== !!next.is_masquerading) return true;
    if (!!prev.via_forward_auth !== !!next.via_forward_auth) return true;
    if (prev.sidebar_sort !== next.sidebar_sort) return true;
    if (!!prev.sidebar_hide_read !== !!next.sidebar_hide_read) return true;
    const key = (cats) => (cats || []).map((c) => `${c.id}:${c.name}`).join('|');
    if (key(prev.categories) !== key(next.categories)) return true;
    // With fully-read groups hidden, a badge reaching or leaving zero adds or
    // removes a row — something `_updateBadges` has no way to express.
    if (next.sidebar_hide_read) {
        const shown = (cats) => (cats || []).filter((c) => c.unread_count > 0)
            .map((c) => c.id).join('|');
        if (shown(prev.categories) !== shown(next.categories)) return true;
    }
    // Under the unread ordering, changed counts also change the *order*, which
    // this deliberately does not treat as structural: re-sorting the list on
    // every mark-as-read would move rows out from under the pointer mid-click.
    // The order settles on the next full render (a navigation, or any of the
    // changes above).
    return false;
}

class RdrsSidebar extends HTMLElement {
    static get observedAttributes() { return ['active', 'active-category-id', 'active-feed-id']; }

    constructor() {
        super();
        // Bound once so connect/disconnect add and remove the *same* reference.
        this._onDocumentClick = this._onDocumentClick.bind(this);
        this._onStale = this._onStale.bind(this);
        // category id -> feed list, for the categories opened this session.
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

        // Tap-outside-to-close for the mobile drawer. The scrim is a CSS
        // pseudo-element (no clickable element of its own), so the listener has
        // to live on the document rather than on a real overlay node.
        document.addEventListener('click', this._onDocumentClick);
        // Action paths that mutate state the sidebar reflects announce it with
        // `rdrs:sidebar-stale` instead of reaching in for `.refresh()`.
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
        // The observed attributes only decide which item carries `.active`, so
        // patch those classes instead of re-rendering. That matters now that
        // category switching swaps the list pane in place: `render()` rebuilds
        // `innerHTML`, and a rebuilt `.sidebar-nav` loses its scroll position —
        // the exact jump the in-place swap exists to avoid.
        this._applyActive();
        // A new active category needs its feed list, which is loaded on demand.
        if (name === 'active-category-id' && oldValue !== newValue) {
            this._renderFeeds();
            this.fetchFeeds();
        }
    }

    /// Repaint `.active` from the current `active` / `active-category-id` /
    /// `active-feed-id` attributes. No-op before the first render (nothing to
    /// patch yet); `render()` reads the same attributes itself.
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

    /// Latest category list from /api/sidebar — in the order it is rendered,
    /// and without the rows the reader's preferences hide — or [] before the
    /// first payload lands. Read by app.js's `[` / `]` category navigation,
    /// which must not reach into the private `_data` field, and must step
    /// through exactly what is on screen.
    get categories() {
        return arrangeSidebarRows(this._data?.categories, sidebarPrefs(this._data),
            this.activeCategoryId);
    }

    /// Feeds of the currently active category, in the order they are rendered,
    /// or [] when no category is active or the list hasn't arrived yet. Read by
    /// app.js's `[` / `]` navigation, which walks categories and the open
    /// category's feeds as one flat list — the order on screen.
    get activeFeeds() {
        const catId = this.activeCategoryId;
        if (!catId) return [];
        return arrangeSidebarRows(this._feeds[catId], sidebarPrefs(this._data),
            this.activeFeedId);
    }

    /// Which category a feed belongs to, if any list loaded this session names
    /// it. Used by app.js to keep the right category expanded when navigation
    /// lands on a feed; `null` means "unknown", not "no category".
    categoryIdOfFeed(feedId) {
        const wanted = parseInt(feedId, 10);
        for (const [catId, feeds] of Object.entries(this._feeds)) {
            if (feeds.some((f) => f.id === wanted)) return parseInt(catId, 10);
        }
        return null;
    }

    /// Load the active category's feeds. Only the open category is ever shown,
    /// so only it is fetched: a several-hundred-feed account would otherwise
    /// pay for its whole subscription list on every page load to render one
    /// category's worth (see `get_sidebar_category_feeds`).
    async fetchFeeds(options = {}) {
        const catId = this.activeCategoryId;
        if (!catId) return;
        // First mount asks twice — once from the upgrade-time
        // attributeChangedCallback, once from connectedCallback — and the
        // second would abort the first for the same answer. A revalidation
        // (`force`) still supersedes whatever is in flight.
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
            // The reader may have moved to another category while this was in
            // flight; the response describes the category it was asked about.
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

    /// Imperative escape hatch for a caller that already holds the element and
    /// wants to await the refetch. The `rdrs:sidebar-stale` event is the normal
    /// path — prefer it, since it doesn't require finding the element first.
    refresh() { return this.fetchData(); }

    /// Coalesced: one interaction routinely raises `rdrs:sidebar-stale` more
    /// than once. Opening an entry fires it twice a few ms apart — once from
    /// app.js's `rdrs:swap-complete` hook, once from the server's SSE `sidebar`
    /// event after the auto-mark-as-read — and both mean the same thing.
    ///
    /// Trailing edge, so the fetch runs after the last signal in a burst and
    /// therefore reads state with every write in that burst applied. The delay
    /// is invisible in practice: the row and pane have already been swapped by
    /// this point, and this only revalidates the counts beside them.
    _onStale() {
        clearTimeout(this._staleTimer);
        this._staleTimer = setTimeout(() => {
            this._staleTimer = null;
            this.fetchData();
            // Feed badges move for the same reasons category badges do — an
            // entry opened, a bulk mark-as-read — and they come from a
            // different endpoint, so they need their own refetch.
            this.fetchFeeds({ force: true });
        }, STALE_COALESCE_MS);
    }

    async fetchData() {
        // A newer request supersedes whatever is still in flight: without this,
        // two overlapping /api/sidebar responses can land out of order and the
        // staler payload wins.
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

    /// Mobile drawer open/close. The hamburger is hidden while the drawer is
    /// open so it doesn't sit on top of the panel.
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
        // `closest()` rather than `sidebar.contains()`: render() rebuilds the
        // whole subtree, so a click can land on a node that has already been
        // detached. closest() still walks that node's own ancestor chain.
        if (e.target.closest('#sidebar') || e.target.closest('.sidebar-toggle')) return;
        this.closeDrawer();
    }

    /// Mount (or refresh) the feed list under the open category, and drop any
    /// list left behind by the category before it. Written into the existing
    /// DOM rather than folded into `render()` for the usual reason: a full
    /// rebuild resets `.sidebar-nav`'s scroll offset, and this runs on every
    /// category switch.
    _renderFeeds() {
        const container = this.querySelector('#sidebar-categories');
        if (!container) return;
        const catId = this.activeCategoryId;
        for (const list of container.querySelectorAll('.sidebar-feeds')) {
            if (parseInt(list.dataset.categoryId || '0', 10) !== catId) list.remove();
        }
        if (!catId) return;
        const link = container.querySelector(`a[data-category-id="${catId}"]`);
        const feeds = this._feeds[catId];
        // No link yet (categories still loading) or no feed list yet: leave the
        // gap rather than flash an empty group — fetchFeeds() calls back here.
        if (!link || !feeds) return;
        let list = container.querySelector(`.sidebar-feeds[data-category-id="${catId}"]`);
        if (!list) {
            list = document.createElement('div');
            list.className = 'sidebar-feeds';
            list.dataset.categoryId = String(catId);
            link.insertAdjacentElement('afterend', list);
        }
        const activeFeedId = this.activeFeedId;
        list.innerHTML = arrangeSidebarRows(feeds, sidebarPrefs(this._data), activeFeedId)
            .map((feed) => `
            <a href="/feeds/${feed.id}/entries" class="sidebar-feed${feed.id === activeFeedId ? ' active' : ''}" data-feed-id="${feed.id}" title="${escapeHtml(feed.title)}">
                ${feedFavicon(feed)}
                <span class="sidebar-item-label">${escapeHtml(feed.title)}</span>
                ${feed.unread_count > 0 ? `<span class="sidebar-badge">${feed.unread_count}</span>` : ''}
            </a>`).join('');
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

        // /settings and /admin are both admin-only server-side; hide the links
        // for regular accounts so the nav matches what they can actually open.
        const appSettingsLink = isAdmin ? `
            <a href="/settings" class="sidebar-item${isActive('settings')}" data-nav="settings" data-testid="nav-app-settings">
                <span class="sidebar-item-icon">${ICON.cog}</span>
                <span>App</span>
            </a>` : '';

        const adminLink = isAdmin ? `
            <a href="/admin" class="sidebar-item${isActive('admin')}" data-nav="admin" data-testid="nav-admin">
                <span class="sidebar-item-icon">${ICON.shield}</span>
                <span>Admin</span>
            </a>` : '';

        this.innerHTML = `
<button class="sidebar-toggle" type="button" aria-label="Open menu">${ICON.menu}</button>

<aside class="sidebar" id="sidebar" data-testid="main-nav">
    ${masqBanner}
    <div class="sidebar-header">
        <a href="/" class="sidebar-logo">rdrs</a>
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
            </a>
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

        // The rebuild above also discards the open category's feed list, which
        // lives inside #sidebar-categories and is not part of this template.
        this._renderFeeds();

        // innerHTML above discards the previous subtree along with its
        // listeners, so every render re-binds from scratch.
        this.querySelector('.sidebar-toggle')?.addEventListener('click', () => this.toggleDrawer());
        this.querySelector('.sidebar-close')?.addEventListener('click', () => this.closeDrawer());

        this.querySelector('[data-rdrs-logout]')?.addEventListener('click', async (e) => {
            e.preventDefault();
            // Once we've told a forward-auth user to log out at their proxy, the
            // local session is already gone; a second click would just 401 and
            // flash a misleading "Logout failed". Make further clicks a no-op.
            if (this._proxyLogoutNotified) return;
            try {
                const r = await fetch('/api/session', { method: 'DELETE' });
                if (r.ok) {
                    const d = await r.json();
                    if (d.logout_url_configured) {
                        // A proxy/SSO logout URL is configured (absolute or a same-host
                        // path): hand off so the upstream session actually ends.
                        window.location.href = d.redirect_to;
                    } else if (d.via_forward_auth) {
                        // Forward-auth with no logout URL configured: a local logout is a no-op
                        // because the proxy re-injects the identity header on the next request.
                        // Be honest instead of bouncing to /login and silently re-authenticating.
                        this._proxyLogoutNotified = true;
                        window.flash.warning('You are signed in via your reverse proxy. To end your session, log out at your proxy or SSO provider, then reload this page to keep using the app.');
                    } else {
                        // Normal cookie/password session: fully logged out server-side.
                        window.flash.redirect(d.redirect_to, 'info', 'You have been logged out.');
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
