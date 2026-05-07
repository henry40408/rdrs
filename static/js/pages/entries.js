// <rdrs-entries-page> — CSR shell for the entries-family list pages.
//
// One element handles every list-mode route. data-mode is derived from
// location.pathname on connect:
//
//   /                       -> unread
//   /entries                -> all
//   /entries/read           -> read
//   /entries/starred        -> starred
//   /entries/summarized     -> summarized
//   (B2 will add feed + category, B3 will add search.)
//
// The element renders the shell once: sidebar + flash + main + split-view +
// list-pane header + list-pane body wrapping <rdrs-entry-list> + reading-pane
// skeleton. Once rendered, only <rdrs-entry-list> updates its own subtree on
// data reloads.
//
// Deep links (?entry=N) work via the existing fallback inside
// <rdrs-entry-list>._checkEntryParam → _loadEntryByIdInPane (which calls
// /reader/api/0/stream/items/contents). No deep-link logic at this layer.
//
// <rdrs-entry-list>, <rdrs-sidebar>, <rdrs-flash> are loaded by
// app_shell.html before this module — module ordering by document order
// guarantees they're registered before connectedCallback runs. We do NOT
// re-import them here: the shell uses versioned URLs (?v=GIT_VERSION) and
// ES-module dedup is URL-exact, so a bare import would re-execute the
// module and trigger a double customElements.define.

const MARK_AS_READ_DROPDOWN = `
    <div class="form-group form-group-inline">
        <select id="mark-read-age" data-testid="mark-read-select" class="select-auto">
            <option value="">Mark as Read...</option>
            <option value="1">Older than 1 day</option>
            <option value="7">Older than 1 week</option>
            <option value="30">Older than 1 month</option>
            <option value="365">Older than 1 year</option>
            <option value="all">All entries</option>
        </select>
    </div>
`;

const TAB_BAR = `
    <div class="tab-bar">
        <a href="/entries" data-testid="tab-all" data-tab="all">All</a>
        <a href="/entries/read" data-testid="tab-read" data-tab="read">Read</a>
        <a href="/entries/starred" data-testid="tab-starred" data-tab="starred">Starred</a>
        <a href="/entries/summarized" data-testid="tab-summarized" data-tab="summarized">Summarized</a>
    </div>
`;

const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};

const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';
const READ_STATE = 'user/-/state/com.google/read';
const STARRED_STATE = 'user/-/state/com.google/starred';

const FILTER_STATUS_DROPDOWN = `
    <div class="form-group form-group-inline">
        <select id="filter-status" data-testid="filter-status" class="select-auto">
            <option value="">All</option>
            <option value="unread">Unread</option>
            <option value="read">Read</option>
            <option value="starred">Starred</option>
        </select>
    </div>
`;

function statusToApiParams(status) {
    if (status === 'unread') return { xt: READ_STATE };
    if (status === 'read') return { it: READ_STATE };
    if (status === 'starred') return { it: STARRED_STATE };
    return {};
}

function readSidebarBootstrap() {
    const el = document.getElementById('rdrs-sidebar-bootstrap');
    if (!el) return null;
    try { return JSON.parse(el.textContent); } catch { return null; }
}

async function fetchFeedMeta(feedId) {
    const res = await fetch('/api/feeds');
    if (!res.ok) return null;
    const data = await res.json();
    return data.feeds.find(f => f.id === feedId) || null;
}

function escapeHtmlInline(s) {
    return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

const TAB_KB = [
    { key: '1', desc: 'Go to All entries', handle: () => { location.href = '/entries'; return true; } },
    { key: '2', desc: 'Go to Read entries', handle: () => { location.href = '/entries/read'; return true; } },
    { key: '3', desc: 'Go to Starred entries', handle: () => { location.href = '/entries/starred'; return true; } },
    { key: '4', desc: 'Go to Summarized entries', handle: () => { location.href = '/entries/summarized'; return true; } },
];

const MODES = {
    unread: {
        title: 'Unread',
        navKey: 'unread',
        renderHeader: () => `<h1>Unread</h1><div class="filter-bar">${MARK_AS_READ_DROPDOWN}</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ xt: READ_STATE }),
            origin: 'unread',
            'show-feed': '',
            'show-category': '',
            'show-mark-above': '',
            'empty-message': 'No unread entries.',
        },
        kb: [
            { key: 'A', desc: 'Mark above as read', handle: (list) => { list.markAboveAsRead(); return true; } },
        ],
    },
    all: {
        title: 'Entries',
        navKey: 'entries',
        renderHeader: () => `<h1>Entries</h1>${TAB_BAR}<div class="filter-bar">${MARK_AS_READ_DROPDOWN}</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            origin: 'entries',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No entries found.',
        },
        kb: TAB_KB,
    },
    read: {
        title: 'Read',
        navKey: 'entries',
        renderHeader: () => `<h1>Read</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ it: READ_STATE }),
            origin: 'read',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No read entries.',
        },
        kb: TAB_KB,
    },
    starred: {
        title: 'Starred',
        navKey: 'starred',
        renderHeader: () => `<h1>Starred</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ it: STARRED_STATE }),
            origin: 'starred',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No starred entries.',
        },
        kb: TAB_KB,
    },
    summarized: {
        title: 'Summarized',
        navKey: 'entries',
        renderHeader: () => `<h1>Summarized</h1>${TAB_BAR}`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            'api-params': JSON.stringify({ has_summary: 'true' }),
            origin: 'summarized',
            'show-feed': '',
            'show-category': '',
            'empty-message': 'No summarized entries.',
        },
        kb: TAB_KB,
    },
    // Feed and category modes resolve their stream-id and breadcrumb
    // asynchronously in _connectAsync. listAttrs/renderHeader here are
    // placeholders — the real ones are constructed at runtime after meta
    // arrives (feed: GET /api/feeds; category: sidebar bootstrap blob).
    feed: {
        title: 'Feed',
        navKey: 'feeds',
        renderHeader: () => `<h1>Loading…</h1>`,
        listAttrs: null,
        kb: [
            { key: '1', desc: 'Show all entries', handle: (list, page) => { page._setStatus(''); return true; } },
            { key: '2', desc: 'Show unread only', handle: (list, page) => { page._setStatus('unread'); return true; } },
            { key: '3', desc: 'Show read only', handle: (list, page) => { page._setStatus('read'); return true; } },
            { key: '4', desc: 'Show starred only', handle: (list, page) => { page._setStatus('starred'); return true; } },
            { key: 'A', desc: 'Mark above as read', handle: (list) => { list.markAboveAsRead(); return true; } },
            { key: 'c', desc: 'Go to category page', handle: (list, page) => { if (page._categoryId) location.href = `/categories/${page._categoryId}/entries`; return true; } },
            { key: 'x', desc: 'Go to category page', handle: (list, page) => { if (page._categoryId) location.href = `/categories/${page._categoryId}/entries`; return true; } },
        ],
    },
    category: {
        title: 'Category',
        navKey: 'category',
        renderHeader: () => `<h1>Loading…</h1>`,
        listAttrs: null,
        kb: [
            { key: '1', desc: 'Show all entries', handle: (list, page) => { page._setStatus(''); return true; } },
            { key: '2', desc: 'Show unread only', handle: (list, page) => { page._setStatus('unread'); return true; } },
            { key: '3', desc: 'Show read only', handle: (list, page) => { page._setStatus('read'); return true; } },
            { key: '4', desc: 'Show starred only', handle: (list, page) => { page._setStatus('starred'); return true; } },
            { key: 'A', desc: 'Mark above as read', handle: (list) => { list.markAboveAsRead(); return true; } },
            { key: 'x', desc: 'Go to unread page', handle: () => { location.href = '/'; return true; } },
        ],
    },
    search: {
        title: 'Search',
        navKey: 'search',
        renderHeader: () => `
<h1>Search</h1>
<div class="filter-bar">
    <div class="form-group form-group-inline flex-1">
        <input type="text" id="filter-search" placeholder="Search entries..." autofocus data-testid="search-input">
    </div>
    <div>
        <button type="button" id="search-btn" data-testid="search-btn">Search</button>
    </div>
</div>`,
        listAttrs: {
            'stream-id': READING_LIST_STREAM,
            origin: 'search',
            'show-feed': '',
            'show-category': '',
            'no-auto-load': '',
            'empty-message': 'Enter a search term and press Enter to search.',
        },
        kb: [
            { key: '/', desc: 'Focus search box', handle: (list, page) => { const input = page.querySelector('#filter-search'); if (input) input.focus(); return true; } },
        ],
    },
};

function inferMode() {
    const path = location.pathname;
    if (path === '/' || path === '') return 'unread';
    if (path === '/entries') return 'all';
    if (path === '/entries/read') return 'read';
    if (path === '/entries/starred') return 'starred';
    if (path === '/entries/summarized') return 'summarized';
    if (path === '/search') return 'search';
    if (/^\/feeds\/\d+\/entries$/.test(path)) return 'feed';
    if (/^\/categories\/\d+\/entries$/.test(path)) return 'category';
    return 'unread';
}

function pathId() {
    const m = location.pathname.match(/^\/(?:feeds|categories)\/(\d+)\/entries$/);
    return m ? parseInt(m[1], 10) : null;
}

function attrString(attrs) {
    return Object.entries(attrs)
        .map(([k, v]) => v === '' ? k : `${k}="${String(v).replace(/"/g, '&quot;')}"`)
        .join(' ');
}

class RdrsEntriesPage extends HTMLElement {
    connectedCallback() {
        const mode = inferMode();
        this.dataset.mode = mode;
        const cfg = MODES[mode];

        if (mode === 'feed' || mode === 'category') {
            this._connectAsync(mode, cfg);
            return;
        }

        // Static-mode flow (unread / all / read / starred / summarized).
        // Mount the entry-list with `no-auto-load` so we can configure its
        // attributes from /api/user-settings before it fires its first
        // stream/contents fetch.
        const attrs = { ...cfg.listAttrs, 'no-auto-load': '' };
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="${cfg.navKey}"></rdrs-sidebar>
<main class="main-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <div class="split-view">
        <div class="list-pane">
            <div class="list-pane-header">${cfg.renderHeader()}</div>
            <div class="list-pane-body">
                <rdrs-entry-list ${attrString(attrs)} reading-pane="#reading-pane"></rdrs-entry-list>
            </div>
        </div>
        <div class="reading-pane" id="reading-pane">
            <div class="reading-pane-empty">Select an entry to read</div>
        </div>
    </div>
</main>
</div>`;

        this._wireMarkAsRead();
        this._wireTabActive(mode);
        this._wireKeyboardHandlers(mode);
        this._loadAndStart();
    }

    /// feed/category modes resolve their stream-id and breadcrumb data
    /// asynchronously. Render a placeholder shell first so sidebar +
    /// flash paint immediately, then await meta and mount the entry-list.
    async _connectAsync(mode, cfg) {
        const id = pathId();
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="${cfg.navKey}"></rdrs-sidebar>
<main class="main-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <div class="split-view">
        <div class="list-pane">
            <div class="list-pane-header" id="list-pane-header"><h1>Loading…</h1></div>
            <div class="list-pane-body" id="list-pane-body"></div>
        </div>
        <div class="reading-pane" id="reading-pane">
            <div class="reading-pane-empty">Select an entry to read</div>
        </div>
    </div>
</main>
</div>`;

        const initialStatus = new URLSearchParams(location.search).get('status') || 'unread';

        let streamId, headerHtml;
        if (mode === 'feed') {
            const meta = await fetchFeedMeta(id);
            if (!meta) {
                this.querySelector('#list-pane-header').innerHTML = `<h1>Feed not found</h1>`;
                return;
            }
            this._feedId = id;
            this._categoryId = meta.category_id;
            this._feedUrl = meta.url;
            this._feedTitle = meta.title;
            streamId = `feed/${meta.url}`;
            const iconImg = meta.has_icon
                ? `<img src="/api/feeds/${id}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">`
                : '';
            headerHtml = `
<div class="breadcrumb">
    <a href="/feeds">Feeds</a> / <a href="/categories/${meta.category_id}/entries">${escapeHtmlInline(meta.category_name)}</a> / ${escapeHtmlInline(meta.title)}
</div>
<h1>${iconImg}${escapeHtmlInline(meta.title)}</h1>
<div class="filter-bar">
    ${FILTER_STATUS_DROPDOWN}
    ${MARK_AS_READ_DROPDOWN}
</div>`;
        } else {
            const sidebar = readSidebarBootstrap();
            const cat = sidebar?.categories?.find(c => c.id === id);
            if (!cat) {
                this.querySelector('#list-pane-header').innerHTML = `<h1>Category not found</h1>`;
                return;
            }
            this._categoryId = id;
            this._categoryName = cat.name;
            streamId = `user/-/label/${cat.name}`;
            headerHtml = `
<div class="breadcrumb">
    <a href="/categories">Categories</a> / ${escapeHtmlInline(cat.name)}
</div>
<h1>${escapeHtmlInline(cat.name)}</h1>
<div class="filter-bar">
    ${FILTER_STATUS_DROPDOWN}
    ${MARK_AS_READ_DROPDOWN}
</div>`;
        }

        this._streamId = streamId;
        this.querySelector('#list-pane-header').innerHTML = headerHtml;

        const attrs = {
            'stream-id': streamId,
            origin: mode,
            'show-feed': '',
            ...(mode === 'category' ? { 'show-category': '' } : {}),
            'show-mark-above': '',
            'no-auto-load': '',
            'empty-message': 'No entries found.',
        };
        const body = this.querySelector('#list-pane-body');
        body.innerHTML = `<rdrs-entry-list ${attrString(attrs)} reading-pane="#reading-pane"></rdrs-entry-list>`;

        this._currentStatus = initialStatus;
        this.querySelector('#filter-status').value = initialStatus;
        this._wireFilterStatus();
        this._wireMarkAsReadStream(streamId, mode);
        this._wireKeyboardHandlers(mode);

        const list = this.querySelector('rdrs-entry-list');
        try {
            const res = await fetch('/api/user-settings');
            if (res.ok) {
                const settings = await res.json();
                if (settings.entries_per_page) list.setAttribute('entries-per-page', String(settings.entries_per_page));
                if (settings.linkding_configured) list.setAttribute('has-save-services', '');
                if (settings.kagi_configured) list.setAttribute('has-kagi-configured', '');
            }
        } catch { /* defaults */ }
        list.setApiParams(statusToApiParams(initialStatus));
        list.loadEntries();
    }

    _setStatus(status) {
        this._currentStatus = status;
        const select = this.querySelector('#filter-status');
        if (select) select.value = status;
        const params = new URLSearchParams();
        if (status) params.set('status', status);
        const url = location.pathname + (params.toString() ? '?' + params.toString() : '');
        history.replaceState(null, '', url);
        const list = this.querySelector('rdrs-entry-list');
        if (list) {
            list.setApiParams(statusToApiParams(status));
            list.loadEntries();
        }
    }

    _wireFilterStatus() {
        const select = this.querySelector('#filter-status');
        if (!select) return;
        select.addEventListener('change', () => this._setStatus(select.value));
    }

    /// Stream-scoped mark-as-read for feed / category modes. Posts to
    /// /reader/api/0/mark-all-as-read with `s=<streamId>` and an optional
    /// `ts=` cutoff in microseconds.
    _wireMarkAsReadStream(streamId, mode) {
        const select = this.querySelector('#mark-read-age');
        if (!select) return;
        const scopeLabel = mode === 'feed' ? 'this feed' : 'this category';
        select.addEventListener('change', async () => {
            const age = select.value;
            select.selectedIndex = 0;
            if (!age) return;
            const ageLabel = AGE_LABELS[age] || age;
            if (!confirm(`Mark ${ageLabel} entries in ${scopeLabel} as read?`)) return;
            try {
                const body = new URLSearchParams();
                body.set('s', streamId);
                if (age !== 'all') {
                    const days = parseInt(age, 10);
                    const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
                    body.set('ts', tsUsec.toString());
                }
                const response = await fetch('/reader/api/0/mark-all-as-read', { method: 'POST', body });
                if (!response.ok) throw new Error('Failed to mark as read');
                window.flash && window.flash.success('Marked entries as read.');
                this.querySelector('rdrs-entry-list').loadEntries();
            } catch (err) {
                window.flash && window.flash.error(err.message);
            }
        });
    }

    async _loadAndStart() {
        const list = this.querySelector('rdrs-entry-list');
        if (!list) return;
        try {
            const res = await fetch('/api/user-settings');
            if (res.ok) {
                const settings = await res.json();
                if (settings.entries_per_page) {
                    list.setAttribute('entries-per-page', String(settings.entries_per_page));
                }
                if (settings.linkding_configured) list.setAttribute('has-save-services', '');
                if (settings.kagi_configured) list.setAttribute('has-kagi-configured', '');
            }
        } catch {
            // Network error — proceed with defaults; reading-pane action
            // bar just won't show Save/Summarize buttons.
        }
        // Search mode is no-auto-load: wire the input + button, and only
        // fetch when ?q= is present in the URL (or the user submits).
        if (this.dataset.mode === 'search') {
            this._wireSearch(list);
            return;
        }
        list.loadEntries();
    }

    _wireSearch(list) {
        const input = this.querySelector('#filter-search');
        const btn = this.querySelector('#search-btn');
        if (!input || !btn) return;

        const doSearch = () => {
            const q = input.value.trim();
            if (!q) {
                list.search = '';
                list.showEmpty('Enter a search term and press Enter to search.');
                history.replaceState(null, '', '/search');
                return;
            }
            list.search = q;
            list.loadEntries();
            const params = new URLSearchParams();
            params.set('q', q);
            history.replaceState(null, '', '/search?' + params.toString());
        };

        btn.addEventListener('click', doSearch);
        input.addEventListener('keydown', (event) => {
            if (event.key === 'Enter') {
                event.preventDefault();
                doSearch();
            } else if (event.key === 'Escape') {
                event.preventDefault();
                input.blur();
            }
        });

        const initialQ = new URLSearchParams(location.search).get('q');
        if (initialQ) {
            input.value = initialQ;
            list.search = initialQ;
            list.loadEntries();
        } else {
            list.showEmpty('Enter a search term and press Enter to search.');
        }
    }

    _wireMarkAsRead() {
        const select = this.querySelector('#mark-read-age');
        if (!select) return;
        select.addEventListener('change', async () => {
            const age = select.value;
            select.selectedIndex = 0;
            if (!age) return;
            const ageLabel = AGE_LABELS[age] || age;
            if (!confirm(`Mark ${ageLabel} entries as read?`)) return;
            try {
                const body = new URLSearchParams();
                body.set('s', READING_LIST_STREAM);
                if (age !== 'all') {
                    const days = parseInt(age, 10);
                    const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
                    body.set('ts', tsUsec.toString());
                }
                const response = await fetch('/reader/api/0/mark-all-as-read', { method: 'POST', body });
                if (!response.ok) throw new Error('Failed to mark as read');
                window.flash && window.flash.success('Marked entries as read.');
                this.querySelector('rdrs-entry-list').loadEntries();
            } catch (err) {
                window.flash && window.flash.error(err.message);
            }
        });
    }

    _wireTabActive(mode) {
        const tabs = this.querySelectorAll('.tab-bar a[data-tab]');
        const activeTab = mode === 'all' ? 'all' : mode;
        tabs.forEach(a => {
            if (a.dataset.tab === activeTab) a.classList.add('active');
        });
    }

    _wireKeyboardHandlers(mode) {
        const cfg = MODES[mode];
        if (!cfg.kb || cfg.kb.length === 0) return;
        const page = this;
        customElements.whenDefined('rdrs-entry-list').then(() => {
            const list = page.querySelector('rdrs-entry-list');
            if (!list) return;
            list.registerKeyboardHandlers({
                helpItems: cfg.kb.map(k => ({ key: k.key, desc: k.desc })),
                handleKey(key) {
                    const entry = cfg.kb.find(k => k.key === key);
                    if (!entry) return false;
                    return entry.handle(list, page);
                },
            });
        });
    }
}

customElements.define('rdrs-entries-page', RdrsEntriesPage);
