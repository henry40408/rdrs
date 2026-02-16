// <rdrs-entry-list> — Paginated entry list with keyboard navigation (Light DOM)
import { escapeHtml, decodeHtml, formatDate, formatDateTime, highlightText, getContentSnippet } from '/static/js/utils.js';

class RdrsEntryList extends HTMLElement {
    constructor() {
        super();
        this.entries = [];
        this.currentOffset = 0;
        this.total = 0;
        this.selectedIndex = -1;
        this._search = '';
    }

    connectedCallback() {
        this._render();
        this._setupDelegation();
        this._setupPersistedRestore();
        // Pages that need to set API params before loading should add no-auto-load
        if (!this.hasAttribute('no-auto-load')) {
            this.loadEntries();
        }
    }

    // --- Attributes ---
    get apiUrl() { return this.getAttribute('api-url') || '/api/entries'; }
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

    // --- Initial render (skeleton) ---
    _render() {
        this.innerHTML = `
<style>
.entries-list-refreshing {
    position: relative;
    opacity: 0.5;
    pointer-events: none;
}
</style>
<div id="entries-list" data-testid="entries-list">
    <p class="muted">Loading...</p>
</div>
<div id="load-more" class="hidden-mt4">
    <button type="button" data-testid="load-more-btn">Load More</button>
</div>
${this.showMarkAbove ? `<div id="mark-above-read" class="hidden-mt4">
    <button type="button" data-testid="mark-above-btn">Mark Above as Read</button>
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

            // Feed link
            if (target.matches('[data-feed-id]') && !target.hasAttribute('data-action')) {
                e.preventDefault();
                window.location.href = `/feeds/${target.dataset.feedId}/entries`;
                return;
            }

            // Category link
            if (target.matches('[data-category-id]') && !target.hasAttribute('data-action')) {
                e.preventDefault();
                window.location.href = `/categories/${target.dataset.categoryId}/entries`;
                return;
            }

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
    }

    // --- bfcache restore ---
    _setupPersistedRestore() {
        window.setupPersistedRestore(
            () => this.loadEntries(),
            () => this.entries,
            (idx) => this.selectEntry(idx)
        );
    }

    // --- Data loading ---
    async loadEntries(reset = true) {
        const container = this.querySelector('#entries-list');
        container.classList.add('entries-list-refreshing');
        if (window.loading) window.loading.start();

        if (reset) {
            this.currentOffset = 0;
            this.entries = [];
        }

        const params = new URLSearchParams();
        params.set('limit', this.entriesPerPage);
        params.set('offset', this.currentOffset);

        // Merge in configured API params
        const extra = this.apiParams;
        for (const [k, v] of Object.entries(extra)) {
            params.set(k, v);
        }

        // Search term
        if (this._search) {
            params.set('search', this._search);
        }

        const url = `${this.apiUrl}?${params.toString()}`;

        try {
            const response = await fetch(url, { cache: 'no-store' });
            if (!response.ok) throw new Error('Failed to load entries');
            const data = await response.json();

            if (reset) {
                this.entries = data.entries;
            } else {
                this.entries = this.entries.concat(data.entries);
            }
            this.total = data.total;

            this.renderEntries();
            this._updateLoadMore();
            this._updateEntriesCount();
            this._updateMarkAbove();

            if (this.isUnreadMode) {
                this._updateUnreadCount();
            }
        } catch (err) {
            container.innerHTML = '<p class="muted">Failed to load entries</p>';
        } finally {
            container.classList.remove('entries-list-refreshing');
            if (window.loading) window.loading.stop();
        }
    }

    async loadMore() {
        this.currentOffset += this.entriesPerPage;
        await this.loadEntries(false);
    }

    // --- Rendering ---
    renderEntries() {
        const container = this.querySelector('#entries-list');

        if (this.entries.length === 0) {
            container.innerHTML = `<p class="muted">${escapeHtml(this.emptyMessage)}</p>`;
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
                summaryBadgeHtml = '<span title="Has Summary" class="summary-badge">S</span>';
            } else if (summaryStatus === 'pending') {
                summaryBadgeHtml = '<span title="Pending" class="summary-badge-pending">P</span>';
            } else if (summaryStatus === 'processing') {
                summaryBadgeHtml = '<span title="Processing" class="summary-badge-processing">\u2026</span>';
            } else if (summaryStatus === 'failed') {
                summaryBadgeHtml = '<span title="Failed" class="summary-badge-failed">F</span>';
            }

            // Title with optional search highlight
            const titleHtml = search ? highlightText(title, search) : escapeHtml(title);

            // Origin query for entry link
            let originQuery = `?origin=${encodeURIComponent(origin)}`;
            if (origin === 'feed' && entry.feed_id) originQuery += `&feed=${entry.feed_id}`;
            if (origin === 'category' && entry.category_id) originQuery += `&category=${entry.category_id}`;

            // Content snippet for search
            let contentSnippetHtml = '';
            if (search && entry.content) {
                const snippet = getContentSnippet(entry.content, search);
                if (snippet) {
                    contentSnippetHtml = `<div class="muted content-snippet">${highlightText(snippet, search)}</div>`;
                }
            }

            // Meta
            let metaParts = [];
            if (this.showFeed) {
                const feedLink = origin === 'search'
                    ? `<a href="/feeds/${entry.feed_id}/entries">${escapeHtml(feedTitle)}</a>`
                    : `<a href="#" data-feed-id="${entry.feed_id}">${escapeHtml(feedTitle)}</a>`;
                metaParts.push(feedIconHtml + feedLink);
            }
            if (this.showCategory) {
                const catLink = origin === 'search'
                    ? `<a href="/categories/${entry.category_id}/entries">${escapeHtml(entry.category_name)}</a>`
                    : `<a href="#" data-category-id="${entry.category_id}">${escapeHtml(entry.category_name)}</a>`;
                metaParts.push(catLink);
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
                    <a href="/entries/${entry.id}${originQuery}" class="entry-item-title ${isRead ? 'entry-title-normal' : 'entry-title-bold'}" data-testid="entry-title-link">${titleHtml}</a>
                    ${isStarred ? '<span title="Starred">*</span>' : ''}
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

    // --- UI updates ---
    _updateLoadMore() {
        const btn = this.querySelector('#load-more');
        btn.style.display = this.entries.length < this.total ? 'block' : 'none';
    }

    _updateEntriesCount() {
        const el = this.querySelector('#entries-count');
        if (this.entries.length > 0 || this.total > 0) {
            el.textContent = `Showing ${this.entries.length} of ${this.total} entries`;
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
        const el = document.getElementById('unread-count');
        if (el) el.textContent = this.total;
    }

    // --- Entry actions ---
    async markRead(id) {
        try {
            const response = await fetch(`/api/entries/${id}/read`, { method: 'PUT' });
            if (!response.ok) throw new Error('Failed to mark as read');
            const updated = await response.json();

            if (this.isUnreadMode) {
                // Remove from list in unread mode
                this.entries = this.entries.filter(e => e.id !== id);
                this.total--;
                this.renderEntries();
                this._updateLoadMore();
                this._updateUnreadCount();
                this._updateMarkAbove();
                // Adjust selection
                if (this.selectedIndex >= this.entries.length) {
                    this.selectedIndex = this.entries.length - 1;
                }
                if (this.entries.length > 0 && this.selectedIndex >= 0) {
                    this.selectEntry(this.selectedIndex);
                }
            } else {
                this._updateEntryInList(id, updated);
            }
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async markUnread(id) {
        try {
            const response = await fetch(`/api/entries/${id}/unread`, { method: 'PUT' });
            if (!response.ok) throw new Error('Failed to mark as unread');
            const updated = await response.json();
            this._updateEntryInList(id, updated);
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async toggleStar(id) {
        try {
            const response = await fetch(`/api/entries/${id}/star`, { method: 'PUT' });
            if (!response.ok) throw new Error('Failed to toggle star');
            const updated = await response.json();
            this._updateEntryInList(id, updated);
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    _updateEntryInList(id, updated) {
        const idx = this.entries.findIndex(e => e.id === id);
        if (idx >= 0) {
            this.entries[idx].read_at = updated.read_at;
            this.entries[idx].starred_at = updated.starred_at;
            this.renderEntries();
        }
    }

    async markAboveAsRead() {
        if (this.entries.length === 0) return;

        if (!confirm(`Mark all ${this.entries.length} loaded entries as read?`)) return;

        const entryIds = this.entries.map(e => e.id);

        try {
            const response = await fetch('/api/entries/mark-read-by-ids', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ entry_ids: entryIds })
            });
            if (!response.ok) throw new Error('Failed to mark entries as read');
            const result = await response.json();
            window.flash.success(`Marked ${result.marked_count} entries as read.`);
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

        if (this.isUnreadMode) {
            // In unread mode all entries are unread, just move in direction
            const start = this.selectedIndex < 0 ? (direction > 0 ? -1 : this.entries.length) : this.selectedIndex;
            const index = start + direction;
            return (index >= 0 && index < this.entries.length) ? index : -1;
        }

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
        if (this.selectedIndex >= 0) window._resumeIndex = this.selectedIndex;

        let originQuery = `?origin=${encodeURIComponent(this.origin)}`;
        if (this.origin === 'feed' && entry.feed_id) originQuery += `&feed=${entry.feed_id}`;
        if (this.origin === 'category' && entry.category_id) originQuery += `&category=${entry.category_id}`;

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

    // --- Register standard keyboard handlers ---
    registerKeyboardHandlers(extraHandlers = {}) {
        const list = this;

        const baseHelpItems = [
            { key: 'j', desc: 'Next entry' },
            { key: 'k', desc: 'Previous entry' },
            { key: 'g g', desc: 'First entry' },
            { key: 'G', desc: 'Last entry' },
            { key: 'n', desc: 'Next unread entry' },
            { key: 'N', desc: 'Previous unread entry' },
            { key: 'Enter / o', desc: 'Open entry' },
            { key: 'v', desc: 'Open original in new tab' },
            { key: 'm', desc: list.isUnreadMode ? 'Mark as read' : 'Toggle read/unread' },
            { key: 's', desc: 'Toggle star' },
            { key: 'r', desc: 'Refresh list' },
        ];

        if (this.showFeed) {
            baseHelpItems.push({ key: 'f', desc: 'Go to feed page (requires selection)' });
        }
        if (this.showCategory) {
            baseHelpItems.push({ key: 'c', desc: 'Go to category page (requires selection)' });
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
                    return true;
                }
                if (extraHandlers.handleCombo) {
                    return extraHandlers.handleCombo(combo);
                }
                return false;
            },
            handleKey(key, shiftKey) {
                switch (key) {
                    case 'j':
                        list.selectEntry(list.selectedIndex + 1);
                        return true;
                    case 'k':
                        list.selectEntry(list.selectedIndex - 1);
                        return true;
                    case 'G':
                        if (list.entries.length > 0) list.selectEntry(list.entries.length - 1);
                        return true;
                    case 'n': {
                        const next = list.findNextUnread(1);
                        if (next >= 0) list.selectEntry(next);
                        return true;
                    }
                    case 'N': {
                        const prev = list.findNextUnread(-1);
                        if (prev >= 0) list.selectEntry(prev);
                        return true;
                    }
                    case 'Enter':
                    case 'o':
                        list.openSelectedEntry();
                        return true;
                    case 'v':
                        list.openOriginalLink();
                        return true;
                    case 'm':
                        list.toggleSelectedRead();
                        return true;
                    case 's':
                        list.toggleSelectedStar();
                        return true;
                    case 'r':
                        list.loadEntries();
                        return true;
                    case 'c':
                        if (list.showCategory) {
                            const entryC = list.getSelectedEntry();
                            if (entryC) {
                                window.location.href = `/categories/${entryC.category_id}/entries`;
                            }
                            return true;
                        }
                        break;
                    case 'f':
                        if (list.showFeed) {
                            const entryF = list.getSelectedEntry();
                            if (entryF) {
                                window.location.href = `/feeds/${entryF.feed_id}/entries`;
                            }
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
        this.total = 0;
        this.selectedIndex = -1;
        const container = this.querySelector('#entries-list');
        if (container) {
            container.innerHTML = `<p class="muted">${escapeHtml(message || this.emptyMessage)}</p>`;
        }
        this._updateLoadMore();
        this._updateEntriesCount();
        this._updateMarkAbove();
    }
}

customElements.define('rdrs-entry-list', RdrsEntryList);
