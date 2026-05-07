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
};

function inferMode() {
    const path = location.pathname;
    if (path === '/' || path === '') return 'unread';
    if (path === '/entries') return 'all';
    if (path === '/entries/read') return 'read';
    if (path === '/entries/starred') return 'starred';
    if (path === '/entries/summarized') return 'summarized';
    return 'unread';
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

        // Mount the entry-list with `no-auto-load` so we can configure its
        // attributes from /api/user-settings before it fires its first
        // stream/contents fetch. Without this, the element would fetch
        // immediately with the wrong entries-per-page (and reading-pane
        // action bar would flash empty until save/kagi flags arrived).
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
        list.loadEntries();
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
        customElements.whenDefined('rdrs-entry-list').then(() => {
            const list = this.querySelector('rdrs-entry-list');
            if (!list) return;
            list.registerKeyboardHandlers({
                helpItems: cfg.kb.map(k => ({ key: k.key, desc: k.desc })),
                handleKey(key) {
                    const entry = cfg.kb.find(k => k.key === key);
                    if (!entry) return false;
                    return entry.handle(list);
                },
            });
        });
    }
}

customElements.define('rdrs-entries-page', RdrsEntriesPage);
