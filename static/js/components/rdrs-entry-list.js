// <rdrs-entry-list> — Paginated entry list with keyboard navigation (Light DOM)
// Uses Google Reader API for data fetching and entry actions.
import { escapeHtml, decodeHtml, formatDate, formatDateTime, highlightText, getContentSnippet } from '/static/js/utils.js';

class RdrsEntryList extends HTMLElement {
    constructor() {
        super();
        this.entries = [];
        this.continuation = null;
        this.total = -1; // unknown until first count
        this.selectedIndex = -1;
        this._search = '';
        this._readingPaneEntry = null; // currently displayed entry in reading pane
        this._readingPaneData = null; // full data for the reading pane entry
        this._summaryPollInterval = null;
        this._currentSummary = null;
        this._showingFullContent = false;
        this._originalContent = null;
        this._fullContent = null;
        this._extraHandlers = {};
        this._popstateHandler = null;
    }

    connectedCallback() {
        this._render();
        this._setupDelegation();
        this._setupPersistedRestore();
        this._setupPopstate();

        // Pages that need to set API params before loading should add no-auto-load.
        if (!this.hasAttribute('no-auto-load')) {
            this.loadEntries();
        }
    }

    disconnectedCallback() {
        this._stopSummaryPolling();
        if (this._popstateHandler) {
            window.removeEventListener('popstate', this._popstateHandler);
        }
    }

    // --- Attributes ---
    /** Google Reader stream ID, e.g. "user/-/state/com.google/reading-list" */
    get streamId() { return this.getAttribute('stream-id') || 'user/-/state/com.google/reading-list'; }
    get apiParams() {
        try { return JSON.parse(this.getAttribute('api-params') || '{}'); }
        catch { return {}; }
    }
    get entriesPerPage() { return parseInt(this.getAttribute('entries-per-page') || '20', 10); }
    get origin() { return this.getAttribute('origin') || 'unread'; }
    get showFeed() { return this.hasAttribute('show-feed'); }
    get showCategory() { return this.hasAttribute('show-category'); }
    get showMarkAbove() { return this.hasAttribute('show-mark-above'); }
    get isUnreadMode() { return this.origin === 'unread'; }
    get search() { return this._search; }
    set search(v) { this._search = v; }
    get emptyMessage() { return this.getAttribute('empty-message') || 'No entries found.'; }
    get readingPaneSelector() { return this.getAttribute('reading-pane') || null; }
    get hasSaveServices() { return this.hasAttribute('has-save-services'); }
    get hasKagiConfigured() { return this.hasAttribute('has-kagi-configured'); }

    /** Get the reading pane element, if configured. */
    _getReadingPane() {
        const sel = this.readingPaneSelector;
        if (!sel) return null;
        return document.querySelector(sel);
    }

    /** Check if we're in mobile layout (reading pane is not visible by default). */
    _isMobileLayout() {
        return window.matchMedia('(max-width: 1024px)').matches;
    }

    // --- Initial render (skeleton) ---
    _render() {
        const initialMessage = this.hasAttribute('no-auto-load') ? this.emptyMessage : 'Loading...';
        this.innerHTML = `
<div id="entries-list" data-testid="entries-list">
    <p class="muted entries-status-msg">${initialMessage}</p>
</div>
<div id="load-more" class="hidden-mt4">
    <button type="button" data-testid="load-more-btn">Load More</button>
</div>
${this.showMarkAbove ? `<div id="mark-above-read" class="hidden-mt4">
    <button type="button" class="btn-secondary" data-testid="mark-above-btn">Mark Above as Read</button>
</div>` : ''}
<p id="entries-count" class="muted" data-testid="entries-count"></p>
        `;

        // Load more button
        this.querySelector('#load-more button').addEventListener('click', () => this.loadMore());

        // Mark above button
        const markAboveBtn = this.querySelector('#mark-above-read button');
        if (markAboveBtn) {
            markAboveBtn.addEventListener('click', () => this.markAboveAsRead());
        }
    }

    // --- Event delegation for entry actions ---
    _setupDelegation() {
        this.querySelector('#entries-list').addEventListener('click', (e) => {
            const target = e.target;

            // Feed and category links in the meta row are now real anchors
            // (`<a href="/feeds/{id}/entries">`); the document-level SPA
            // router intercepts them, so this handler doesn't need to do
            // anything for them.

            // Actions
            const action = target.dataset.action;
            const entryId = target.dataset.entryId;
            if (!action || !entryId) return;

            const id = parseInt(entryId, 10);

            switch (action) {
                case 'mark-read':
                    e.preventDefault();
                    this.markRead(id);
                    break;
                case 'mark-unread':
                    e.preventDefault();
                    this.markUnread(id);
                    break;
                case 'toggle-star':
                    e.preventDefault();
                    this.toggleStar(id);
                    break;
                case 'open-original':
                    // Don't prevent default — let the link open in new tab
                    this.markRead(id);
                    break;
            }
        });

        // Click on entry item to select + show in reading pane
        this.querySelector('#entries-list').addEventListener('click', (e) => {
            const entryItem = e.target.closest('.entry-item');
            if (!entryItem) return;

            // Don't handle if clicking on action links
            if (e.target.closest('.entry-item-actions') || e.target.closest('.entry-item-meta a')) return;

            const index = parseInt(entryItem.dataset.index, 10);
            if (isNaN(index)) return;

            this.selectEntry(index);

            // If reading pane is visible, load entry there; otherwise navigate to entry page
            const pane = this._getReadingPane();
            if (pane) {
                e.preventDefault();
                this._loadInReadingPane(index);
            } else {
                this.openSelectedEntry();
            }
        });
    }

    // --- popstate for browser back/forward ---
    _setupPopstate() {
        this._popstateHandler = (e) => {
            if (e.state && e.state.entryId !== undefined) {
                const idx = this.entries.findIndex(en => en.id === e.state.entryId);
                if (idx >= 0) {
                    this.selectEntry(idx);
                    this._loadInReadingPane(idx, true);
                } else {
                    this._loadEntryByIdInPane(e.state.entryId);
                }
            } else {
                this._closeReadingPaneDetail();
            }
        };
        window.addEventListener('popstate', this._popstateHandler);
    }

    // --- Check URL for ?entry= param on load ---
    _checkEntryParam() {
        const urlEntry = new URLSearchParams(window.location.search).get('entry');
        if (!urlEntry) return;

        const entryId = parseInt(urlEntry, 10);
        if (isNaN(entryId)) return;

        const idx = this.entries.findIndex(e => e.id === entryId);
        if (idx >= 0) {
            this.selectEntry(idx);
            this._loadInReadingPane(idx);
        } else {
            // Entry not in current list, load directly via API.
            this._loadEntryByIdInPane(entryId);
        }
    }

    // --- Load an entry by ID directly into the reading pane ---
    async _loadEntryByIdInPane(entryId) {
        const pane = this._getReadingPane();
        if (!pane) return;

        pane.classList.add('reading-pane-active');
        pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Loading...</p></div>`;

        try {
            const response = await fetch(`/api/entries/${entryId}`);
            if (response.status === 404) {
                pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Entry not found.</p></div>`;
                return;
            }
            if (!response.ok) throw new Error('Failed to load entry');
            const data = await response.json();
            this._readingPaneEntry = { id: entryId, ...data };
            this._readingPaneData = data;
            this._renderReadingPaneDetail(pane, data, entryId);
            pane.scrollTop = 0;

            if (data.read_at === null) {
                this.markRead(entryId, true);
            }

            if (data.summary_status) {
                this._handleSummaryStatus(data.summary_status, entryId);
            }
        } catch (err) {
            pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Failed to load entry.</p></div>`;
        }
    }

    // --- bfcache restore ---
    _setupPersistedRestore() {
        window.setupPersistedRestore(
            () => this.loadEntries(),
            () => this.entries,
            (idx) => this.selectEntry(idx)
        );
    }

    // --- Data loading (Google Reader stream/contents) ---
    async loadEntries(reset = true) {
        const container = this.querySelector('#entries-list');
        container.classList.add('entries-list-refreshing');

        if (reset) {
            this.continuation = null;
            this.entries = [];
        }

        const streamId = this.streamId;
        const url = `/reader/api/0/stream/contents/${encodeURIComponent(streamId)}`;
        const params = new URLSearchParams();
        params.set('n', this.entriesPerPage);

        if (this.continuation) {
            params.set('c', this.continuation);
        }

        // Merge in configured API params (xt, it, ot, nt, r)
        const extra = this.apiParams;
        for (const [k, v] of Object.entries(extra)) {
            params.set(k, v);
        }

        // Search query
        if (this._search) {
            params.set('q', this._search);
        }

        // Skip content sanitization when not searching (list view doesn't need it)
        if (!this._search) {
            params.set('no_content', 'true');
        }

        const fullUrl = `${url}?${params.toString()}`;

        try {
            const response = await fetch(fullUrl, { cache: 'no-store' });
            if (!response.ok) throw new Error('Failed to load entries');
            const data = await response.json();

            // Transform GReader items to internal entry format
            const newEntries = (data.items || []).map(item => this._transformItem(item));

            if (reset) {
                this.entries = newEntries;
            } else {
                this.entries = this.entries.concat(newEntries);
            }

            this.continuation = data.continuation || null;

            this.renderEntries();
            this._updateLoadMore();
            this._updateEntriesCount();
            this._updateMarkAbove();

            if (this.isUnreadMode) {
                this._updateUnreadCount();
            }

            // After first load, check for ?entry= param
            if (reset) {
                this._checkEntryParam();
            }
        } catch (err) {
            container.innerHTML = '<p class="muted entries-status-msg">Failed to load entries</p>';
        } finally {
            container.classList.remove('entries-list-refreshing');
        }
    }

    /** Transform a GReader item to the internal entry format used by rendering. */
    _transformItem(item) {
        return {
            id: item._entryId,
            feed_id: item._feedId,
            category_id: item._categoryId,
            category_name: item._categoryName,
            feed_title: item.origin?.title || '',
            feed_url: item.origin?.htmlUrl || '',
            feed_has_icon: item._feedHasIcon || false,
            title: item.title || '',
            link: item.canonical?.[0]?.href || item.alternate?.[0]?.href || '',
            content: item._content || item.summary?.content || '',
            author: item.author || '',
            published_at: item._publishedAt || null,
            read_at: item._readAt || null,
            starred_at: item._starredAt || null,
            summary_status: item._summaryStatus || null,
        };
    }

    /** Extract full entry data from a GReader item for reading pane detail. */
    _extractEntryData(item) {
        return {
            title: item.title || '',
            link: item.canonical?.[0]?.href || item.alternate?.[0]?.href || '',
            content: item._content || item.summary?.content || '',
            author: item.author || '',
            feed_title: item.origin?.title || '',
            feed_has_icon: item._feedHasIcon || false,
            feed_id: item._feedId,
            category_id: item._categoryId,
            category_name: item._categoryName,
            published_at: item._publishedAt || null,
            read_at: item._readAt || null,
            starred_at: item._starredAt || null,
            summary_status: item._summaryStatus || null,
        };
    }

    async loadMore() {
        await this.loadEntries(false);
    }

    // --- Rendering ---
    renderEntries() {
        const container = this.querySelector('#entries-list');

        if (this.entries.length === 0) {
            container.innerHTML = `<p class="muted entries-status-msg">${escapeHtml(this.emptyMessage)}</p>`;
            return;
        }

        container.innerHTML = this.entries.map((entry, index) => {
            const title = decodeHtml(entry.title) || 'Untitled';
            const feedTitle = decodeHtml(entry.feed_title) || entry.feed_url;
            const date = entry.published_at ? formatDate(entry.published_at) : '';
            const dateTitle = entry.published_at ? formatDateTime(entry.published_at) : '';
            const isRead = entry.read_at !== null;
            const isStarred = entry.starred_at !== null;
            const summaryStatus = entry.summary_status;
            const isSelected = index === this.selectedIndex;
            const origin = this.origin;
            const search = this._search;

            // Feed icon
            const feedIconHtml = entry.feed_has_icon
                ? `<img src="/api/feeds/${entry.feed_id}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">`
                : '';

            // Summary badge
            let summaryBadgeHtml = '';
            if (summaryStatus === 'completed') {
                summaryBadgeHtml = '<span title="Has Summary" class="summary-badge">&#10003;</span>';
            } else if (summaryStatus === 'pending') {
                summaryBadgeHtml = '<span title="Pending" class="summary-badge-pending">&#9675;</span>';
            } else if (summaryStatus === 'processing') {
                summaryBadgeHtml = '<span title="Processing" class="summary-badge-processing">&#8635;</span>';
            } else if (summaryStatus === 'failed') {
                summaryBadgeHtml = '<span title="Failed" class="summary-badge-failed">&#10007;</span>';
            }

            // Title with optional search highlight
            const titleHtml = search ? highlightText(title, search) : escapeHtml(title);

            // Origin query for entry link
            let originQuery = `?origin=${encodeURIComponent(origin)}`;
            if (origin === 'feed' && entry.feed_id) originQuery += `&feed=${entry.feed_id}`;
            if (origin === 'category' && entry.category_id) originQuery += `&category=${entry.category_id}`;
            if (origin === 'read') originQuery += '&read_only=true';
            if (origin === 'starred') originQuery += '&starred_only=true';
            if (origin === 'summarized') originQuery += '&has_summary=true';

            // Content snippet for search
            let contentSnippetHtml = '';
            if (search && entry.content) {
                const snippet = getContentSnippet(entry.content, search);
                if (snippet) {
                    contentSnippetHtml = `<div class="muted content-snippet">${highlightText(snippet, search)}</div>`;
                }
            }

            // Meta — feed/category links use real href so the SPA router
            // intercepts them at document level. The entry-item-meta a
            // selector in the row-click handler also keeps these clicks
            // from triggering reading-pane selection.
            let metaParts = [];
            if (this.showFeed) {
                metaParts.push(feedIconHtml + `<a href="/feeds/${entry.feed_id}/entries">${escapeHtml(feedTitle)}</a>`);
            }
            if (this.showCategory) {
                metaParts.push(`<a href="/categories/${entry.category_id}/entries">${escapeHtml(entry.category_name)}</a>`);
            }
            if (date) {
                metaParts.push(`<span title="${dateTitle}">${date}</span>`);
            }

            // Actions
            let readAction;
            if (this.isUnreadMode) {
                readAction = `<a href="#" data-action="mark-read" data-entry-id="${entry.id}" data-testid="entry-read-action">read</a>`;
            } else {
                readAction = isRead
                    ? `<a href="#" data-action="mark-unread" data-entry-id="${entry.id}" data-testid="entry-read-action">unread</a>`
                    : `<a href="#" data-action="mark-read" data-entry-id="${entry.id}" data-testid="entry-read-action">read</a>`;
            }
            const starAction = `<a href="#" data-action="toggle-star" data-entry-id="${entry.id}" data-testid="entry-star-action">${isStarred ? 'unstar' : 'star'}</a>`;
            const originalLink = entry.link
                ? `<a href="${escapeHtml(entry.link)}" target="_blank" rel="noopener noreferrer" data-action="open-original" data-entry-id="${entry.id}" data-testid="entry-original-link">original</a>`
                : '';

            return `
            <div class="entry-item${isSelected ? ' selected' : ''}${isRead ? ' entry-read' : ''}" id="entry-${entry.id}" data-index="${index}" data-testid="entry-item">
                <div>
                    <span class="entry-item-title ${isRead ? 'entry-title-normal' : 'entry-title-bold'}" data-testid="entry-title-link">${titleHtml}</span>
                    ${isStarred ? '<span title="Starred" class="star-icon">&#9733;</span>' : ''}
                    ${summaryBadgeHtml}
                </div>${contentSnippetHtml}
                <div class="muted entry-item-meta">
                    ${metaParts.join(' &middot; ')}
                </div>
                <div class="entry-item-actions">
                    ${readAction}
                    ${starAction}
                    ${originalLink}
                </div>
            </div>`;
        }).join('');
    }

    // --- Reading Pane: Full Entry Detail ---
    async _loadInReadingPane(index, skipPushState = false, replaceHistory = false) {
        const entry = this.entries[index];
        if (!entry) return;

        const pane = this._getReadingPane();
        if (!pane) return;

        this._readingPaneEntry = entry;
        this._resetReadingPaneState();

        // Activate reading pane (for mobile full-screen)
        pane.classList.add('reading-pane-active');

        // Show loading state
        pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Loading...</p></div>`;

        try {
            // Fetch full content
            const body = new URLSearchParams();
            body.set('i', entry.id.toString());

            const response = await fetch('/reader/api/0/stream/items/contents', {
                method: 'POST',
                body: body
            });

            if (!response.ok) throw new Error('Failed to load entry');
            const result = await response.json();
            if (!result.items || result.items.length === 0) {
                pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Entry not found.</p></div>`;
                return;
            }

            const item = result.items[0];
            const data = this._extractEntryData(item);
            this._readingPaneData = data;

            this._renderReadingPaneDetail(pane, data, entry.id);
            this._updateReadingPaneNav();

            // Scroll reading pane to top
            pane.scrollTop = 0;

            // pushState to update URL
            if (!skipPushState) {
                let originQuery = `?origin=${encodeURIComponent(this.origin)}`;
                if (this.origin === 'feed' && entry.feed_id) originQuery += `&feed=${entry.feed_id}`;
                if (this.origin === 'category' && entry.category_id) originQuery += `&category=${entry.category_id}`;
                if (this.origin === 'read') originQuery += '&read_only=true';
                if (this.origin === 'starred') originQuery += '&starred_only=true';
                if (this.origin === 'summarized') originQuery += '&has_summary=true';
                const url = `/entries/${entry.id}${originQuery}`;
                if (replaceHistory) {
                    history.replaceState({ entryId: entry.id, index }, '', url);
                } else {
                    history.pushState({ entryId: entry.id, index }, '', url);
                }
            }

            // Auto-mark as read (keep in list — user is viewing in reading pane)
            if (entry.read_at === null) {
                this.markRead(entry.id, true);
            }

            // Handle existing summary
            if (data.summary_status) {
                this._handleSummaryStatus(data.summary_status, entry.id);
            }

        } catch (err) {
            pane.innerHTML = `<div class="reading-pane-content"><p class="muted">Failed to load entry.</p></div>`;
        }
    }

    _resetReadingPaneState() {
        this._stopSummaryPolling();
        this._currentSummary = null;
        this._showingFullContent = false;
        this._originalContent = null;
        this._fullContent = null;
    }

    _renderReadingPaneDetail(pane, data, entryId) {
        const title = this._decodeHtml(data.title) || 'Untitled';
        const feedTitle = this._decodeHtml(data.feed_title);
        const author = this._decodeHtml(data.author);
        const date = data.published_at ? new Date(data.published_at).toLocaleString() : '';
        const content = data.content || '<p class="muted">No content available.</p>';
        const isStarred = data.starred_at !== null;
        const feedIconHtml = data.feed_has_icon
            ? `<img src="/api/feeds/${data.feed_id}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">`
            : '';

        let metaParts = [];
        if (feedTitle) metaParts.push(feedIconHtml + escapeHtml(feedTitle));
        if (author) metaParts.push(escapeHtml(author));
        if (date) metaParts.push(date);

        const hasSave = this.hasSaveServices;
        const hasKagi = this.hasKagiConfigured;

        const backBtn = this._isMobileLayout()
            ? `<div class="reading-pane-back">
                <span data-rp-action="back" class="reading-pane-back-link">&#8592; Back</span>
                <span class="reading-pane-nav">
                    <button type="button" data-rp-action="prev-entry" class="reading-pane-nav-btn" aria-label="Previous entry">&#8249;</button>
                    <button type="button" data-rp-action="next-entry" class="reading-pane-nav-btn" aria-label="Next entry">&#8250;</button>
                </span>
              </div>`
            : '';

        pane.innerHTML = `
            ${backBtn}
            <div class="reading-pane-content">
                <h1 class="reading-pane-title">${escapeHtml(title)}</h1>
                <div class="reading-pane-meta">${metaParts.join(' &middot; ')}</div>
                <div class="reading-pane-actions">
                    <button type="button" class="btn-secondary btn-sm" data-rp-action="toggle-star" data-testid="rp-star-btn">${isStarred ? 'Unstar' : 'Star'}</button>
                    <button type="button" class="btn-secondary btn-sm" data-rp-action="mark-unread" data-testid="rp-mark-unread-btn">Mark Unread</button>
                    ${data.link ? `<button type="button" class="btn-secondary btn-sm" data-rp-action="fetch-full-content" data-testid="rp-fetch-btn">Fetch Full Content</button>` : ''}
                    ${hasKagi && data.link ? `<button type="button" class="btn-secondary btn-sm" data-rp-action="summarize" data-testid="rp-summarize-btn">Summarize</button>` : ''}
                    ${hasSave && data.link ? `<button type="button" class="btn-secondary btn-sm" data-rp-action="save" data-testid="rp-save-btn">Save</button>` : ''}
                    ${data.link ? `<a href="${escapeHtml(data.link)}" target="_blank" rel="noopener noreferrer" class="btn btn-secondary btn-sm">View Original</a>` : ''}
                </div>
                <div class="rp-summary-container d-none" data-testid="rp-summary-container">
                    <div class="summary-box">
                        <div class="summary-actions">
                            <button type="button" class="btn-sm btn-secondary" data-rp-action="copy-summary">Copy</button>
                            <button type="button" class="btn-sm btn-secondary" data-rp-action="dismiss-summary">Dismiss</button>
                        </div>
                        <blockquote class="rp-summary-content"></blockquote>
                    </div>
                </div>
                <article class="reading-pane-article" data-testid="rp-entry-content">${content}</article>
            </div>
        `;

        // Set up action delegation for reading pane buttons (once per pane)
        this._ensureReadingPaneActions(pane);
    }

    _updateReadingPaneNav() {
        const pane = this._getReadingPane();
        if (!pane) return;
        const prevBtn = pane.querySelector('[data-rp-action="prev-entry"]');
        const nextBtn = pane.querySelector('[data-rp-action="next-entry"]');
        if (!prevBtn || !nextBtn) return;

        prevBtn.disabled = this.selectedIndex <= 0;
        nextBtn.disabled = this.selectedIndex >= this.entries.length - 1 && !this.continuation;
    }

    async _navigateReadingPane(direction) {
        const newIndex = this.selectedIndex + direction;

        // Need to loadMore first
        if (newIndex >= this.entries.length && this.continuation) {
            try {
                await this.loadMore();
                if (newIndex < this.entries.length) {
                    this.selectEntry(newIndex);
                    this._loadInReadingPane(newIndex, false, true);
                }
            } catch (err) {
                window.flash?.error('Failed to load more entries.');
            }
            this._updateReadingPaneNav();
            return;
        }

        if (newIndex < 0 || newIndex >= this.entries.length) return;
        this.selectEntry(newIndex);
        this._loadInReadingPane(newIndex, false, true);
        this._updateReadingPaneNav();
    }

    _ensureReadingPaneActions(pane) {
        if (pane._rpActionsSetup) return;
        pane._rpActionsSetup = true;

        pane.addEventListener('click', (e) => {
            const btn = e.target.closest('[data-rp-action]');
            if (!btn) return;

            e.preventDefault();
            const action = btn.dataset.rpAction;

            if (action === 'back') {
                history.back();
                return;
            }
            if (action === 'prev-entry') {
                this._navigateReadingPane(-1);
                return;
            }
            if (action === 'next-entry') {
                this._navigateReadingPane(1);
                return;
            }

            const entryId = this._readingPaneEntry?.id;
            if (!entryId) return;

            switch (action) {
                case 'toggle-star':
                    this._rpToggleStar(entryId);
                    break;
                case 'mark-unread':
                    this._rpMarkUnread(entryId);
                    break;
                case 'fetch-full-content':
                    this._rpFetchFullContent(entryId);
                    break;
                case 'summarize':
                    this._rpSummarize(entryId);
                    break;
                case 'save':
                    this._rpSave(entryId);
                    break;
                case 'copy-summary':
                    this._rpCopySummary(entryId);
                    break;
                case 'dismiss-summary':
                    this._rpDismissSummary(entryId);
                    break;
            }
        });
    }

    // --- Reading Pane Actions ---
    async _rpToggleStar(entryId) {
        const data = this._readingPaneData;
        if (!data) return;

        const isCurrentlyStarred = data.starred_at !== null;
        try {
            const body = new URLSearchParams();
            body.set('i', entryId.toString());
            if (isCurrentlyStarred) {
                body.set('r', 'user/-/state/com.google/starred');
            } else {
                body.set('a', 'user/-/state/com.google/starred');
            }
            const response = await fetch('/reader/api/0/edit-tag', { method: 'POST', body });
            if (!response.ok) throw new Error('Failed to toggle star');

            data.starred_at = isCurrentlyStarred ? null : new Date().toISOString();

            // Update button text
            const pane = this._getReadingPane();
            const btn = pane?.querySelector('[data-rp-action="toggle-star"]');
            if (btn) btn.textContent = data.starred_at ? 'Unstar' : 'Star';

            // Also update in entries list
            this._updateEntryField(entryId, 'starred_at', data.starred_at);
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _rpMarkUnread(entryId) {
        try {
            const body = new URLSearchParams();
            body.set('i', entryId.toString());
            body.set('r', 'user/-/state/com.google/read');
            const response = await fetch('/reader/api/0/edit-tag', { method: 'POST', body });
            if (!response.ok) throw new Error('Failed to mark as unread');
            window.flash.success('Marked as unread.');
            this._updateEntryField(entryId, 'read_at', null);
            this._updateUnreadCount();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _rpFetchFullContent(entryId) {
        const pane = this._getReadingPane();
        const btn = pane?.querySelector('[data-rp-action="fetch-full-content"]');
        if (!btn) return;

        btn.textContent = 'Fetching...';
        btn.disabled = true;

        try {
            const response = await fetch(`/api/entries/${entryId}/fetch-full-content`, { method: 'POST' });
            if (!response.ok) {
                const error = await response.json();
                throw new Error(error.error || 'Failed to fetch content');
            }
            const data = await response.json();
            const articleEl = pane.querySelector('.reading-pane-article');
            if (!this._originalContent) this._originalContent = articleEl.innerHTML;
            this._fullContent = data.sanitized_content || '<p class="muted">No content extracted.</p>';
            articleEl.innerHTML = this._fullContent;
            this._showingFullContent = true;
            btn.style.display = 'none';

            // Add toggle button if not exists
            if (!pane.querySelector('[data-rp-action="toggle-content"]')) {
                btn.insertAdjacentHTML('afterend', ' <button type="button" class="btn-secondary btn-sm" data-rp-action="toggle-content">Show Original</button>');
                pane.querySelector('[data-rp-action="toggle-content"]').addEventListener('click', () => this._rpToggleContent());
            } else {
                pane.querySelector('[data-rp-action="toggle-content"]').textContent = 'Show Original';
            }
        } catch (err) {
            window.flash.error(err.message);
            btn.textContent = 'Fetch Full Content';
        } finally {
            btn.disabled = false;
        }
    }

    _rpToggleContent() {
        const pane = this._getReadingPane();
        if (!pane) return;
        const articleEl = pane.querySelector('.reading-pane-article');
        const toggleBtn = pane.querySelector('[data-rp-action="toggle-content"]');
        if (this._showingFullContent) {
            articleEl.innerHTML = this._originalContent;
            toggleBtn.textContent = 'Show Full Content';
        } else {
            articleEl.innerHTML = this._fullContent;
            toggleBtn.textContent = 'Show Original';
        }
        this._showingFullContent = !this._showingFullContent;
    }

    async _rpSummarize(entryId) {
        const pane = this._getReadingPane();
        const btn = pane?.querySelector('[data-rp-action="summarize"]');
        if (!btn) return;

        btn.textContent = 'Summarizing...';
        btn.disabled = true;

        try {
            const response = await fetch(`/api/entries/${entryId}/summarize`, { method: 'POST' });
            const data = await response.json();
            if (!response.ok) throw new Error(data.error || 'Failed to summarize');

            if (data.status === 'completed' && data.summary_text) {
                this._currentSummary = data.summary_text;
                this._showSummary(entryId);
                btn.textContent = 'Summarize';
                btn.disabled = false;
            } else if (data.status === 'pending' || data.status === 'processing') {
                this._startSummaryPolling(entryId);
            } else if (data.status === 'failed') {
                throw new Error(data.error || 'Summarization failed');
            }
        } catch (err) {
            window.flash.error(err.message);
            btn.textContent = 'Summarize';
            btn.disabled = false;
        }
    }

    async _rpSave(entryId) {
        const pane = this._getReadingPane();
        const btn = pane?.querySelector('[data-rp-action="save"]');
        if (!btn) return;

        btn.textContent = 'Saving...';
        btn.disabled = true;

        try {
            const response = await fetch(`/api/entries/${entryId}/save`, { method: 'POST' });
            const data = await response.json();
            if (!response.ok) throw new Error(data.error || 'Failed to save');

            if (data.all_success) {
                const count = data.results.length;
                window.flash.success(`Saved to ${count} service${count > 1 ? 's' : ''}`);
                btn.textContent = 'Saved!';
                setTimeout(() => { btn.textContent = 'Save'; }, 2000);
            } else {
                const failed = data.results.filter(r => !r.success);
                const succeeded = data.results.filter(r => r.success);
                if (succeeded.length > 0) window.flash.success(`Saved to: ${succeeded.map(r => r.service).join(', ')}`);
                if (failed.length > 0) window.flash.error(`Failed: ${failed.map(r => `${r.service} (${r.message})`).join(', ')}`);
                btn.textContent = 'Save';
            }
        } catch (err) {
            window.flash.error(err.message);
            btn.textContent = 'Save';
        } finally {
            btn.disabled = false;
        }
    }

    async _rpCopySummary(entryId) {
        if (!this._currentSummary || !this._readingPaneData) return;
        const data = this._readingPaneData;
        const cleanTitle = this._sanitizeTitleForTiddlyWiki(this._decodeHtml(data.title) || 'Untitled');
        const link = data.link || '';
        const formattedText = `${cleanTitle}\n\n${link}\n\n${this._currentSummary}`;
        try {
            await navigator.clipboard.writeText(formattedText);
            const pane = this._getReadingPane();
            const btn = pane?.querySelector('[data-rp-action="copy-summary"]');
            if (btn) { btn.textContent = 'Copied!'; setTimeout(() => { btn.textContent = 'Copy'; }, 2000); }
        } catch (err) {
            window.flash.error('Failed to copy to clipboard');
        }
    }

    async _rpDismissSummary(entryId) {
        const pane = this._getReadingPane();
        const container = pane?.querySelector('.rp-summary-container');
        if (container) container.style.display = 'none';
        this._currentSummary = null;
        try {
            await fetch(`/api/entries/${entryId}/summary`, { method: 'DELETE' });
        } catch (err) {
            console.error('Failed to delete summary from cache:', err);
        }
    }

    // --- Summary polling ---
    _handleSummaryStatus(status, entryId) {
        if (status === 'completed') {
            this._loadSummaryFromServer(entryId);
        } else if (status === 'pending' || status === 'processing') {
            const pane = this._getReadingPane();
            const btn = pane?.querySelector('[data-rp-action="summarize"]');
            if (btn) { btn.textContent = 'Summarizing...'; btn.disabled = true; }
            this._startSummaryPolling(entryId);
        }
    }

    async _loadSummaryFromServer(entryId) {
        try {
            const response = await fetch(`/api/entries/${entryId}/summary`);
            if (response.ok) {
                const data = await response.json();
                if (data.status === 'completed' && data.summary_text) {
                    this._currentSummary = data.summary_text;
                    this._showSummary(entryId);
                } else if (data.status === 'pending' || data.status === 'processing') {
                    const pane = this._getReadingPane();
                    const btn = pane?.querySelector('[data-rp-action="summarize"]');
                    if (btn) { btn.textContent = 'Summarizing...'; btn.disabled = true; }
                    this._startSummaryPolling(entryId);
                }
            }
        } catch (err) {
            console.error('Failed to load summary:', err);
        }
    }

    _showSummary(entryId) {
        const pane = this._getReadingPane();
        const container = pane?.querySelector('.rp-summary-container');
        const content = pane?.querySelector('.rp-summary-content');
        if (container && content && this._readingPaneData) {
            const data = this._readingPaneData;
            const cleanTitle = this._sanitizeTitleForTiddlyWiki(this._decodeHtml(data.title) || 'Untitled');
            const link = data.link || '';
            const formattedText = `${cleanTitle}\n\n${link}\n\n${this._currentSummary}`;
            content.textContent = formattedText;
            container.style.display = 'block';
        }
    }

    _startSummaryPolling(entryId) {
        if (this._summaryPollInterval) return;
        this._summaryPollInterval = setInterval(async () => {
            try {
                const response = await fetch(`/api/entries/${entryId}/summary`);
                if (response.status === 404) {
                    this._stopSummaryPolling();
                    const pane = this._getReadingPane();
                    const btn = pane?.querySelector('[data-rp-action="summarize"]');
                    if (btn) { btn.textContent = 'Summarize'; btn.disabled = false; }
                    return;
                }
                if (!response.ok) throw new Error('Failed to check summary status');
                const data = await response.json();
                if (data.status === 'completed' && data.summary_text) {
                    this._stopSummaryPolling();
                    this._currentSummary = data.summary_text;
                    this._showSummary(entryId);
                    const pane = this._getReadingPane();
                    const btn = pane?.querySelector('[data-rp-action="summarize"]');
                    if (btn) { btn.textContent = 'Summarize'; btn.disabled = false; }
                } else if (data.status === 'failed') {
                    this._stopSummaryPolling();
                    window.flash.error(data.error || 'Summarization failed');
                    const pane = this._getReadingPane();
                    const btn = pane?.querySelector('[data-rp-action="summarize"]');
                    if (btn) { btn.textContent = 'Summarize'; btn.disabled = false; }
                }
            } catch (err) {
                this._stopSummaryPolling();
                window.flash.error(err.message);
                const pane = this._getReadingPane();
                const btn = pane?.querySelector('[data-rp-action="summarize"]');
                if (btn) { btn.textContent = 'Summarize'; btn.disabled = false; }
            }
        }, 2000);
    }

    _stopSummaryPolling() {
        if (this._summaryPollInterval) {
            clearInterval(this._summaryPollInterval);
            this._summaryPollInterval = null;
        }
    }

    _sanitizeTitleForTiddlyWiki(title) {
        if (!title) return '';
        return title
            .replace(/\[\[/g, '').replace(/\]\]/g, '')
            .replace(/\[/g, '').replace(/\]/g, '')
            .replace(/\|/g, '')
            .replace(/\{/g, '').replace(/\}/g, '')
            .replace(/</g, '').replace(/>/g, '')
            .trim();
    }

    // --- Close reading pane detail, return to list mode ---
    _closeReadingPaneDetail() {
        this._stopSummaryPolling();
        this._readingPaneEntry = null;
        this._readingPaneData = null;

        const pane = this._getReadingPane();
        if (pane) {
            pane.classList.remove('reading-pane-active');
            pane.innerHTML = '<div class="reading-pane-empty">Select an entry to read</div>';
        }

        // Reset reading pane state
        this._resetReadingPaneState();

        // In unread mode, refresh list to remove read entries
        if (this.isUnreadMode) {
            this.loadEntries();
        }
    }

    _decodeHtml(html) {
        if (!html) return '';
        const textarea = document.createElement('textarea');
        textarea.innerHTML = html;
        return textarea.value;
    }

    // --- UI updates ---
    _updateLoadMore() {
        const btn = this.querySelector('#load-more');
        // Show "Load More" if there's a continuation token
        btn.style.display = this.continuation ? 'block' : 'none';
    }

    _updateEntriesCount() {
        const el = this.querySelector('#entries-count');
        if (this.entries.length > 0) {
            el.textContent = `Showing ${this.entries.length} entries`;
        } else {
            el.textContent = '';
        }
    }

    _updateMarkAbove() {
        const btn = this.querySelector('#mark-above-read');
        if (btn) {
            btn.style.display = this.entries.length > 0 ? 'block' : 'none';
        }
    }

    _updateUnreadCount() {
        // Fetch the actual unread count from GReader API and update sidebar badges
        fetch('/reader/api/0/unread-count')
            .then(r => r.json())
            .then(data => {
                const counts = data.unreadcounts || [];

                // Update total unread badge
                const total = counts.find(u => u.id === 'user/-/state/com.google/reading-list');
                const el = document.getElementById('unread-count');
                if (el) {
                    el.textContent = (total && total.count > 0) ? total.count : '';
                }

                // Update category badges
                const catContainer = document.getElementById('sidebar-categories');
                if (!catContainer) return;
                const catLinks = catContainer.querySelectorAll('a.sidebar-item');
                // Build a map of category numeric ID -> unread count
                const countByLabel = {};
                for (const uc of counts) {
                    if (uc.id.includes('/label/')) {
                        countByLabel[uc.id] = uc.count;
                    }
                }
                for (const link of catLinks) {
                    const badge = link.querySelector('.sidebar-badge');
                    // Extract category label from link text
                    const href = link.getAttribute('href') || '';
                    // Find matching count by checking all label counts
                    const labelId = Object.keys(countByLabel).find(id => {
                        const name = id.split('/label/').pop();
                        const nameSpan = link.querySelector('span:first-child');
                        return nameSpan && nameSpan.textContent === name;
                    });
                    const count = labelId ? countByLabel[labelId] : 0;
                    if (count > 0) {
                        if (badge) {
                            badge.textContent = count;
                        } else {
                            const span = document.createElement('span');
                            span.className = 'sidebar-badge';
                            span.textContent = count;
                            link.appendChild(span);
                        }
                    } else if (badge) {
                        badge.remove();
                    }
                }
            })
            .catch(() => {});
    }

    // --- Entry actions (via Google Reader edit-tag) ---
    /**
     * Mark an entry as read.
     * @param {number} id - Entry ID
     * @param {boolean} keepInList - If true, keep entry in list even in unread mode
     *   (used by auto-mark-as-read when loading in reading pane)
     */
    async markRead(id, keepInList = false) {
        try {
            const body = new URLSearchParams();
            body.set('i', id.toString());
            body.set('a', 'user/-/state/com.google/read');
            const response = await fetch('/reader/api/0/edit-tag', {
                method: 'POST',
                body: body,
            });
            if (!response.ok) throw new Error('Failed to mark as read');

            if (this.isUnreadMode && !keepInList) {
                // Remove from list in unread mode
                this.entries = this.entries.filter(e => e.id !== id);
                this.renderEntries();
                this._updateLoadMore();
                this._updateMarkAbove();
                // Adjust selection
                if (this.selectedIndex >= this.entries.length) {
                    this.selectedIndex = this.entries.length - 1;
                }
                if (this.entries.length > 0 && this.selectedIndex >= 0) {
                    this.selectEntry(this.selectedIndex);
                }
            } else {
                this._updateEntryField(id, 'read_at', new Date().toISOString());
            }
            this._updateUnreadCount();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async markUnread(id) {
        try {
            const body = new URLSearchParams();
            body.set('i', id.toString());
            body.set('r', 'user/-/state/com.google/read');
            const response = await fetch('/reader/api/0/edit-tag', {
                method: 'POST',
                body: body,
            });
            if (!response.ok) throw new Error('Failed to mark as unread');
            this._updateEntryField(id, 'read_at', null);
            this._updateUnreadCount();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async toggleStar(id) {
        const entry = this.entries.find(e => e.id === id);
        if (!entry) return;

        const isCurrentlyStarred = entry.starred_at !== null;

        try {
            const body = new URLSearchParams();
            body.set('i', id.toString());
            if (isCurrentlyStarred) {
                body.set('r', 'user/-/state/com.google/starred');
            } else {
                body.set('a', 'user/-/state/com.google/starred');
            }
            const response = await fetch('/reader/api/0/edit-tag', {
                method: 'POST',
                body: body,
            });
            if (!response.ok) throw new Error('Failed to toggle star');
            this._updateEntryField(id, 'starred_at', isCurrentlyStarred ? null : new Date().toISOString());
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    _updateEntryField(id, field, value) {
        const idx = this.entries.findIndex(e => e.id === id);
        if (idx >= 0) {
            this.entries[idx][field] = value;
            this.renderEntries();
        }
    }

    async markAboveAsRead() {
        if (this.entries.length === 0) return;

        if (!confirm(`Mark all ${this.entries.length} loaded entries as read?`)) return;

        try {
            // Use edit-tag with multiple i= parameters
            const body = new URLSearchParams();
            for (const entry of this.entries) {
                body.append('i', entry.id.toString());
            }
            body.set('a', 'user/-/state/com.google/read');

            const response = await fetch('/reader/api/0/edit-tag', {
                method: 'POST',
                body: body,
            });
            if (!response.ok) throw new Error('Failed to mark entries as read');
            window.flash.success(`Marked ${this.entries.length} entries as read.`);
            this._updateUnreadCount();
            this.loadEntries();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    // --- Keyboard navigation ---
    selectEntry(index) {
        if (this.entries.length === 0) return;

        if (index < 0) index = 0;
        if (index >= this.entries.length) index = this.entries.length - 1;

        const prevSelected = this.querySelector('.entry-item.selected');
        if (prevSelected) prevSelected.classList.remove('selected');

        this.selectedIndex = index;
        const newSelected = this.querySelector(`.entry-item[data-index="${index}"]`);
        if (newSelected) {
            newSelected.classList.add('selected');
            newSelected.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        }
    }

    getSelectedEntry() {
        if (this.selectedIndex >= 0 && this.selectedIndex < this.entries.length) {
            return this.entries[this.selectedIndex];
        }
        return null;
    }

    findNextUnread(direction) {
        if (this.entries.length === 0) return -1;

        if (this.isUnreadMode && !this._readingPaneEntry) {
            // In unread list mode, all visible entries are unread, just move in direction
            const start = this.selectedIndex < 0 ? (direction > 0 ? -1 : this.entries.length) : this.selectedIndex;
            const index = start + direction;
            return (index >= 0 && index < this.entries.length) ? index : -1;
        }

        // When reading pane is active or non-unread mode, check actual read_at status
        const start = this.selectedIndex < 0 ? (direction > 0 ? -1 : this.entries.length) : this.selectedIndex;
        let index = start + direction;
        while (index >= 0 && index < this.entries.length) {
            if (this.entries[index].read_at === null) return index;
            index += direction;
        }
        return -1;
    }

    openSelectedEntry() {
        const entry = this.getSelectedEntry();
        if (!entry) return;

        // If reading pane is visible, load there
        const pane = this._getReadingPane();
        if (pane) {
            this._loadInReadingPane(this.selectedIndex);
            return;
        }

        // Otherwise navigate to entry page (redirect will handle it)
        if (this.selectedIndex >= 0) window._resumeIndex = this.selectedIndex;

        let originQuery = `?origin=${encodeURIComponent(this.origin)}`;
        if (this.origin === 'feed' && entry.feed_id) originQuery += `&feed=${entry.feed_id}`;
        if (this.origin === 'category' && entry.category_id) originQuery += `&category=${entry.category_id}`;
        if (this.origin === 'read') originQuery += '&read_only=true';
        if (this.origin === 'starred') originQuery += '&starred_only=true';
        if (this.origin === 'summarized') originQuery += '&has_summary=true';

        window.location.href = `/entries/${entry.id}${originQuery}`;
    }

    openOriginalLink() {
        const entry = this.getSelectedEntry();
        if (entry && entry.link) {
            this.markRead(entry.id);
            window.open(entry.link, '_blank', 'noopener,noreferrer');
        }
    }

    async toggleSelectedRead() {
        const entry = this.getSelectedEntry();
        if (!entry) return;

        if (this.isUnreadMode) {
            await this.markRead(entry.id);
        } else {
            if (entry.read_at === null) {
                await this.markRead(entry.id);
                // Move to next entry
                if (this.selectedIndex < this.entries.length - 1) {
                    this.selectEntry(this.selectedIndex + 1);
                }
            } else {
                await this.markUnread(entry.id);
            }
        }
    }

    async toggleSelectedStar() {
        const entry = this.getSelectedEntry();
        if (entry) {
            await this.toggleStar(entry.id);
        }
    }

    // --- Register unified keyboard handlers (no list/entry mode split) ---
    registerKeyboardHandlers(extraHandlers = {}) {
        this._extraHandlers = extraHandlers;
        if (this._isMobileLayout()) return;
        const list = this;
        const hasPane = !!this._getReadingPane();

        const baseHelpItems = [
            // Browse group: list navigation
            { key: 'j / n', desc: 'Next entry', group: 'Browse' },
            { key: 'k / p', desc: 'Previous entry', group: 'Browse' },
            { key: 'N', desc: 'Next unread entry', group: 'Browse' },
            { key: 'P', desc: 'Previous unread entry', group: 'Browse' },
            { key: 'g g', desc: 'First entry', group: 'Browse' },
            { key: 'G', desc: 'Last entry', group: 'Browse' },
            { key: 'Enter / o', desc: 'Open entry', group: 'Browse' },
            // Actions group: entry actions
            { key: 'v', desc: 'Open original in new tab', group: 'Actions' },
            { key: 'm', desc: list.isUnreadMode ? 'Mark as read' : 'Toggle read/unread', group: 'Actions' },
            { key: 's', desc: 'Toggle star', group: 'Actions' },
            { key: 'r', desc: 'Refresh list', group: 'Actions' },
        ];

        // Reading pane shortcuts (only shown when pane exists)
        if (hasPane) {
            baseHelpItems.push(
                { key: 'Space', desc: 'Scroll down', group: 'Reading Pane' },
                { key: 'Shift+Space', desc: 'Scroll up', group: 'Reading Pane' },
                { key: 'f', desc: 'Toggle full content', group: 'Reading Pane' },
                { key: 'u', desc: 'Mark as unread', group: 'Reading Pane' },
                { key: 'b', desc: 'Save to bookmarks', group: 'Reading Pane' },
                { key: 'z', desc: 'Toggle Kagi summary', group: 'Reading Pane' },
                { key: 'Esc', desc: 'Close reading pane', group: 'Reading Pane' },
            );
        }

        if (this.showFeed && !hasPane) {
            baseHelpItems.push({ key: 'f', desc: 'Go to feed page', group: 'Actions' });
        }
        if (this.showCategory) {
            baseHelpItems.push({ key: 'c', desc: 'Go to category page', group: 'Actions' });
        }

        const helpItems = (extraHandlers.helpItems || []).length > 0
            ? [...baseHelpItems, ...extraHandlers.helpItems]
            : baseHelpItems;

        window.keyboard.init('list');
        window.keyboard.setHelpItems(helpItems);
        window.keyboard.registerHandlers({
            handleCombo(combo) {
                if (combo === 'g g') {
                    list.selectEntry(0);
                    const pane = list._getReadingPane();
                    if (pane && list.entries.length > 0) list._loadInReadingPane(0);
                    return true;
                }
                if (extraHandlers.handleCombo) {
                    return extraHandlers.handleCombo(combo);
                }
                return false;
            },
            handleKey(key, shiftKey) {
                const pane = list._getReadingPane();
                const rpEntry = list._readingPaneEntry;
                const rpEntryId = rpEntry?.id;

                switch (key) {
                    // --- Navigation (always moves in the entry list) ---
                    case 'j':
                    case 'n': {
                        const nextIdx = list.selectedIndex + 1;
                        if (nextIdx < list.entries.length) {
                            list.selectEntry(nextIdx);
                            if (pane) list._loadInReadingPane(nextIdx);
                        }
                        return true;
                    }
                    case 'k':
                    case 'p': {
                        const prevIdx = list.selectedIndex - 1;
                        if (prevIdx >= 0) {
                            list.selectEntry(prevIdx);
                            if (pane) list._loadInReadingPane(prevIdx);
                        }
                        return true;
                    }
                    case 'G':
                        if (list.entries.length > 0) {
                            const lastIdx = list.entries.length - 1;
                            list.selectEntry(lastIdx);
                            if (pane) list._loadInReadingPane(lastIdx);
                        }
                        return true;
                    case 'N': {
                        const next = list.findNextUnread(1);
                        if (next >= 0) {
                            list.selectEntry(next);
                            if (pane) list._loadInReadingPane(next);
                        }
                        return true;
                    }
                    case 'P': {
                        const prev = list.findNextUnread(-1);
                        if (prev >= 0) {
                            list.selectEntry(prev);
                            if (pane) list._loadInReadingPane(prev);
                        }
                        return true;
                    }

                    // --- Open / original link ---
                    case 'Enter':
                    case 'o':
                        list.openSelectedEntry();
                        return true;
                    case 'v':
                        // If reading pane has content, open that link; otherwise use selected entry
                        if (list._readingPaneData?.link) {
                            window.open(list._readingPaneData.link, '_blank', 'noopener,noreferrer');
                        } else {
                            list.openOriginalLink();
                        }
                        return true;

                    // --- Read/star actions ---
                    case 'm':
                        list.toggleSelectedRead();
                        return true;
                    case 's':
                        if (rpEntryId) {
                            list._rpToggleStar(rpEntryId);
                        } else {
                            list.toggleSelectedStar();
                        }
                        return true;
                    case 'u':
                        if (rpEntryId) list._rpMarkUnread(rpEntryId);
                        return true;

                    // --- Reading pane scrolling ---
                    case ' ':
                        if (pane && rpEntry) {
                            if (shiftKey) {
                                pane.scrollBy({ top: -pane.clientHeight * 0.8, behavior: 'smooth' });
                            } else {
                                pane.scrollBy({ top: pane.clientHeight * 0.8, behavior: 'smooth' });
                            }
                            return true;
                        }
                        return false;

                    // --- Reading pane content actions ---
                    case 'f':
                        if (pane && rpEntryId) {
                            // Toggle full content in reading pane
                            if (list._fullContent) {
                                list._rpToggleContent();
                            } else {
                                const fetchBtn = pane.querySelector('[data-rp-action="fetch-full-content"]');
                                if (fetchBtn && !fetchBtn.disabled) list._rpFetchFullContent(rpEntryId);
                            }
                            return true;
                        }
                        // No reading pane content — go to feed page
                        if (list.showFeed) {
                            const entryF = list.getSelectedEntry();
                            if (entryF) {
                                window.rdrsNavigate(`/feeds/${entryF.feed_id}/entries`);
                            }
                            return true;
                        }
                        break;
                    case 'b':
                        if (list.hasSaveServices && rpEntryId && list._readingPaneData?.link) {
                            const saveBtn = pane?.querySelector('[data-rp-action="save"]');
                            if (saveBtn && !saveBtn.disabled) list._rpSave(rpEntryId);
                        }
                        return true;
                    case 'z':
                        if (rpEntryId) {
                            if (list._currentSummary) {
                                list._rpDismissSummary(rpEntryId);
                            } else if (list.hasKagiConfigured && list._readingPaneData?.link) {
                                const summarizeBtn = pane?.querySelector('[data-rp-action="summarize"]');
                                if (summarizeBtn && !summarizeBtn.disabled) list._rpSummarize(rpEntryId);
                            }
                        }
                        return true;

                    // --- Other ---
                    case 'r':
                        list.loadEntries();
                        return true;
                    case 'c':
                        if (list.showCategory) {
                            const entryC = list.getSelectedEntry();
                            if (entryC) {
                                window.rdrsNavigate(`/categories/${entryC.category_id}/entries`);
                            }
                            return true;
                        }
                        break;
                    case 'Escape':
                        if (pane && rpEntry) {
                            // Clear reading pane content only — no reload, no history change
                            list._stopSummaryPolling();
                            list._readingPaneEntry = null;
                            list._readingPaneData = null;
                            list._resetReadingPaneState();
                            pane.classList.remove('reading-pane-active');
                            pane.innerHTML = '<div class="reading-pane-empty">Select an entry to read</div>';
                            return true;
                        }
                        break;
                }
                // Delegate to extra handlers
                if (extraHandlers.handleKey) {
                    return extraHandlers.handleKey(key, shiftKey);
                }
                return false;
            }
        });
    }

    /** Update API params dynamically (e.g., for filter changes). */
    setApiParams(params) {
        this.setAttribute('api-params', JSON.stringify(params));
    }

    /** Show an empty/placeholder message without loading data. */
    showEmpty(message) {
        this.entries = [];
        this.continuation = null;
        this.selectedIndex = -1;
        const container = this.querySelector('#entries-list');
        if (container) {
            container.innerHTML = `<p class="muted entries-status-msg">${escapeHtml(message || this.emptyMessage)}</p>`;
        }
        this._updateLoadMore();
        this._updateEntriesCount();
        this._updateMarkAbove();
    }
}

customElements.define('rdrs-entry-list', RdrsEntryList);
