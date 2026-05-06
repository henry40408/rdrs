// <rdrs-feeds-page> — CSR replacement for the SSR feeds page.
//
// Reads:
//   GET  /api/feeds?filter=&sort=&category=  — returns feeds + categories
//        with computed has_icon / freshness / relative-time fields, plus
//        active filter/sort/category echoed back.
//
// Writes (existing endpoints, no changes):
//   POST /reader/api/0/subscription/edit       — add / edit / delete
//   POST /reader/api/0/subscription/import     — OPML import
//   GET  /reader/api/0/subscription/export     — OPML export (anchor link)
//   POST /api/feeds/fetch-metadata             — discover + populate
//   POST /api/feeds/{id}/refresh               — manual sync
//
// DOM hooks (testid + class) match the old SSR template so e2e/CSS stay
// green: feed-url-input, feed-category-select, add-feed-btn, feeds-table,
// flash-message, table.mobile-cards, [data-action], #edit-modal,
// #import-modal, etc.

import { escapeHtml } from '/static/js/utils.js';

class RdrsFeedsPage extends HTMLElement {
    connectedCallback() {
        this._activeSyncs = new Set();
        this._renderShell();
        this._renderRows({ loading: true });
        this.load();
    }

    _renderShell() {
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="feeds"></rdrs-sidebar>
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <h1>Feeds</h1>
    <div class="feeds-toolbar">
        <a href="/reader/api/0/subscription/export" class="btn btn-secondary">Export OPML</a>
        <button type="button" class="btn-secondary" data-rdrs-action="show-import">Import OPML</button>
    </div>
    <hr>
    <form id="add-form" data-rdrs-add-form>
        <div class="form-group">
            <label for="url">Feed URL</label>
            <input type="text" id="url" name="url" placeholder="https://example.com/feed.xml or https://example.com" required data-testid="feed-url-input">
        </div>
        <div class="form-group">
            <label for="category">Category</label>
            <select id="category" name="category" required data-testid="feed-category-select">
                <option value="">Loading&hellip;</option>
            </select>
        </div>
        <button type="submit" id="add-btn" data-testid="add-feed-btn">Add Feed</button>
    </form>
    <hr>
    <div id="sync-status-bar"><span id="sync-status-text"></span></div>
    <div class="filter-bar">
        <div class="form-group form-group-inline">
            <label for="filter-category">Category</label>
            <select id="filter-category" class="select-auto" data-rdrs-nav></select>
        </div>
        <div class="form-group form-group-inline">
            <label for="sort-by">Sort</label>
            <select id="sort-by" class="select-auto" data-rdrs-nav></select>
        </div>
        <div class="form-group form-group-inline feed-filter-links" id="feed-filter-links"></div>
    </div>
    <table class="mobile-cards">
        <thead>
            <tr>
                <th>Title</th>
                <th>Category</th>
                <th>Unread</th>
                <th>Actions</th>
            </tr>
        </thead>
        <tbody id="feeds-table" data-testid="feeds-table"></tbody>
    </table>
    <dialog id="import-modal">
        <h2 class="modal-title">Import OPML</h2>
        <form id="import-form" data-rdrs-import-form>
            <div class="form-group">
                <label for="import-file">Upload .opml file</label>
                <input type="file" id="import-file" accept=".opml,.xml">
            </div>
            <div class="form-group">
                <label for="import-content">Or paste OPML content</label>
                <textarea id="import-content" rows="10" class="textarea-full"></textarea>
            </div>
            <div class="modal-actions">
                <button type="submit" id="import-btn">Import</button>
                <button type="button" class="btn-secondary" data-rdrs-action="close-import">Cancel</button>
            </div>
        </form>
    </dialog>
    <dialog id="edit-modal">
        <h2 class="modal-title">Edit Feed</h2>
        <form id="edit-form" data-rdrs-edit-form>
            <input type="hidden" id="edit-id">
            <div class="form-group">
                <label for="edit-url">Feed URL</label>
                <div class="feed-edit-url-row">
                    <input type="text" id="edit-url" name="url" required>
                    <button type="button" class="btn-secondary" data-rdrs-action="fetch-metadata">Fetch</button>
                </div>
            </div>
            <div class="form-group">
                <label for="edit-title">Title</label>
                <input type="text" id="edit-title" name="title">
            </div>
            <div class="form-group">
                <label for="edit-description">Description</label>
                <input type="text" id="edit-description" name="description">
            </div>
            <div class="form-group">
                <label for="edit-site-url">Site URL</label>
                <input type="text" id="edit-site-url" name="site_url">
            </div>
            <div class="form-group">
                <label for="edit-category">Category</label>
                <select id="edit-category" name="category" required></select>
            </div>
            <details class="feed-http-settings">
                <summary>HTTP Settings</summary>
                <div class="feed-http-settings-body">
                    <div class="form-group">
                        <label for="edit-custom-user-agent">Custom User Agent</label>
                        <input type="text" id="edit-custom-user-agent" name="custom_user_agent" placeholder="Leave empty to use global default">
                    </div>
                    <div class="form-group">
                        <label for="edit-custom-referrer">Custom Referrer</label>
                        <input type="text" id="edit-custom-referrer" name="custom_referrer" placeholder="Leave empty to not send Referer header">
                        <div class="feed-http-hint">Some image servers require a specific Referer header to serve images</div>
                    </div>
                    <div class="form-group">
                        <label>
                            <input type="checkbox" id="edit-http2-disabled" name="http2_disabled">
                            Disable HTTP/2
                        </label>
                        <div class="feed-http-hint">Enable this if the feed server has HTTP/2 compatibility issues</div>
                    </div>
                </div>
            </details>
            <div class="modal-actions">
                <button type="submit">Save</button>
                <button type="button" class="btn-secondary" data-rdrs-action="close-edit">Cancel</button>
            </div>
        </form>
    </dialog>
    </div>
</main>
</div>`;

        this.addEventListener('click', (e) => this._onClick(e));
        this.addEventListener('change', (e) => this._onChange(e));
        this.querySelector('#add-form').addEventListener('submit', (e) => this._onAdd(e));
        this.querySelector('#edit-form').addEventListener('submit', (e) => this._onSave(e));
        this.querySelector('#import-form').addEventListener('submit', (e) => this._onImport(e));
        this.querySelector('#edit-modal').addEventListener('click', (e) => {
            if (e.target.id === 'edit-modal') e.target.close();
        });
        this.querySelector('#import-modal').addEventListener('click', (e) => {
            if (e.target.id === 'import-modal') e.target.close();
        });
        this.querySelector('#import-file').addEventListener('change', (e) => this._loadOpmlFile(e.target));
    }

    async load() {
        const qs = window.location.search || '';
        try {
            const r = await fetch('/api/feeds' + qs, { credentials: 'same-origin' });
            if (!r.ok) throw new Error(`Failed to load feeds (${r.status})`);
            const data = await r.json();
            this._data = data;
            this._populateCategorySelects(data.categories);
            this._populateFilterBar(data);
            this._renderRows({ feeds: data.feeds });
        } catch (err) {
            this._renderRows({ error: err.message || 'Failed to load feeds' });
        }
    }

    _populateCategorySelects(categories) {
        const addSelect = this.querySelector('#category');
        const editSelect = this.querySelector('#edit-category');
        const opts = categories.length === 0
            ? `<option value="">No categories available</option>`
            : categories.map(c => `<option value="user/-/label/${escapeHtml(c.name)}">${escapeHtml(c.name)}</option>`).join('');
        addSelect.innerHTML = opts;
        editSelect.innerHTML = categories.map(c => `<option value="user/-/label/${escapeHtml(c.name)}">${escapeHtml(c.name)}</option>`).join('');
    }

    _populateFilterBar(data) {
        const { categories, total_feed_count, active_filter, active_sort, active_category } = data;

        const filterCat = this.querySelector('#filter-category');
        const allOpt = `<option value="/feeds?filter=${active_filter}&sort=${active_sort}">All Categories (${total_feed_count})</option>`;
        const catOpts = categories.map(c => {
            const sel = active_category === c.id ? ' selected' : '';
            return `<option value="/feeds?category=${c.id}&filter=${active_filter}&sort=${active_sort}"${sel}>${escapeHtml(c.name)} (${c.feed_count})</option>`;
        }).join('');
        filterCat.innerHTML = allOpt + catOpts;

        const sortBy = this.querySelector('#sort-by');
        const catParam = active_category != null ? `category=${active_category}&` : '';
        const sortItems = [
            { v: 'title', label: 'Title' },
            { v: 'unread', label: 'Unread Count' },
            { v: 'category', label: 'Category' },
        ];
        sortBy.innerHTML = sortItems.map(s => {
            const sel = active_sort === s.v ? ' selected' : '';
            return `<option value="/feeds?${catParam}filter=${active_filter}&sort=${s.v}"${sel}>${s.label}</option>`;
        }).join('');

        const links = this.querySelector('#feed-filter-links');
        const filters = [
            { v: 'all', label: 'All' },
            { v: 'errors', label: 'Errors' },
            { v: 'stale', label: 'Stale' },
        ];
        links.innerHTML = filters.map(f => {
            const cls = active_filter === f.v ? ' active' : '';
            return `<a href="/feeds?${catParam}sort=${active_sort}&filter=${f.v}" class="feed-filter-link${cls}">${f.label}</a>`;
        }).join('');
    }

    _renderRows(state) {
        const tbody = this.querySelector('#feeds-table');
        if (!tbody) return;
        const { loading, error, feeds } = state;
        if (loading) {
            tbody.innerHTML = `<tr><td colspan="4" class="muted">Loading&hellip;</td></tr>`;
            return;
        }
        if (error) {
            tbody.innerHTML = `<tr><td colspan="4">${escapeHtml(error)}</td></tr>`;
            return;
        }
        if (!feeds || feeds.length === 0) {
            tbody.innerHTML = `<tr><td colspan="4" class="muted">No feeds yet.</td></tr>`;
            return;
        }
        tbody.innerHTML = feeds.map(feed => {
            const errCls = feed.fetch_error ? ' class="feed-error-no-border"' : '';
            const errCellCls = feed.fetch_error ? ' feed-error-no-border' : '';
            const titleSafe = escapeHtml(feed.title);
            const urlSafe = escapeHtml(feed.url);
            const catSafe = escapeHtml(feed.category_name);
            const fetchedRel = escapeHtml(feed.fetched_at_relative);
            const fetchedDt = escapeHtml(feed.fetched_at_datetime);
            const updatedRel = escapeHtml(feed.feed_updated_at_relative);
            const updatedDt = escapeHtml(feed.feed_updated_at_datetime);
            const freshClass = escapeHtml(feed.freshness_class);
            const icon = feed.has_icon
                ? `<img src="/api/feeds/${feed.id}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">`
                : '';
            const errorRow = feed.fetch_error
                ? `
            <tr class="error-row" data-feed-id="${feed.id}" data-is-error-row="true">
                <td colspan="4" class="error-text feed-error-cell">Error: ${escapeHtml(feed.fetch_error)}</td>
            </tr>`
                : '';
            return `
            <tr id="row-feed-${feed.id}" data-feed-id="${feed.id}"${errCls}>
                <td data-label="Title" class="${errCellCls.trim()}">
                    <div class="feed-title-cell">
                        <div>${icon}<span title="${urlSafe}">${titleSafe}</span></div>
                        <div class="feed-health-info">
                            <span class="muted" title="${fetchedDt}">Fetched: ${fetchedRel}</span>
                            &middot;
                            <span class="${freshClass}" title="${updatedDt}">Updated: ${updatedRel}</span>
                        </div>
                    </div>
                </td>
                <td data-label="Category" class="${errCellCls.trim()}">${catSafe}</td>
                <td data-label="Unread" class="${errCellCls.trim()}">${feed.unread_count > 0 ? `<strong>${feed.unread_count}</strong>` : '0'}</td>
                <td class="actions${errCellCls}">
                    <a href="/feeds/${feed.id}/entries">entries</a>
                    <a href="#" data-action="refresh" data-feed-id="${feed.id}" id="refresh-${feed.id}">refresh</a>
                    <a href="#" data-action="edit" data-feed-id="${feed.id}">edit</a>
                    <a href="#" data-action="delete" data-feed-id="${feed.id}">delete</a>
                </td>
            </tr>${errorRow}`;
        }).join('');
    }

    _onChange(e) {
        const target = e.target.closest('[data-rdrs-nav]');
        if (!target) return;
        if (target.value) window.location.href = target.value;
    }

    _onClick(e) {
        const action = e.target.closest('[data-action]')?.dataset.action;
        const utility = e.target.closest('[data-rdrs-action]')?.dataset.rdrsAction;
        if (action) {
            e.preventDefault();
            const feedId = parseInt(e.target.closest('[data-feed-id]').dataset.feedId, 10);
            if (action === 'refresh') this._refresh(feedId);
            else if (action === 'edit') this._showEdit(feedId);
            else if (action === 'delete') this._delete(feedId);
        } else if (utility) {
            e.preventDefault();
            if (utility === 'show-import') {
                this.querySelector('#import-content').value = '';
                this.querySelector('#import-file').value = '';
                this.querySelector('#import-modal').showModal();
            } else if (utility === 'close-import') {
                this.querySelector('#import-modal').close();
            } else if (utility === 'close-edit') {
                this.querySelector('#edit-modal').close();
            } else if (utility === 'fetch-metadata') {
                this._fetchMetadata();
            }
        }
    }

    _findFeed(id) {
        return this._data?.feeds.find(f => f.id === id);
    }

    async _onAdd(e) {
        e.preventDefault();
        const urlInput = this.querySelector('#url');
        const catSelect = this.querySelector('#category');
        const btn = this.querySelector('#add-btn');
        const url = urlInput.value.trim();
        const cat = catSelect.value;
        if (!url) return window.flash.error('URL cannot be empty');
        if (!cat) return window.flash.error('Please select a category');
        btn.textContent = 'Adding...';
        btn.disabled = true;
        try {
            const body = new URLSearchParams();
            body.set('ac', 'subscribe');
            body.set('s', `feed/${url}`);
            body.set('a', cat);
            const r = await fetch('/reader/api/0/subscription/edit', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to add feed');
            urlInput.value = '';
            window.flash.redirect(window.location.pathname + window.location.search, 'success', 'Feed added.');
        } catch (err) {
            window.flash.error(err.message);
        } finally {
            btn.textContent = 'Add Feed';
            btn.disabled = false;
        }
    }

    _showEdit(id) {
        const feed = this._findFeed(id);
        if (!feed) return;
        this.querySelector('#edit-id').value = `feed/${feed.url}`;
        this.querySelector('#edit-url').value = feed.url;
        this.querySelector('#edit-title').value = feed.title || '';
        this.querySelector('#edit-description').value = feed.description || '';
        this.querySelector('#edit-site-url').value = feed.site_url || '';
        const catSelect = this.querySelector('#edit-category');
        const target = `user/-/label/${feed.category_name}`;
        for (const opt of catSelect.options) {
            if (opt.value === target) { opt.selected = true; break; }
        }
        this.querySelector('#edit-custom-user-agent').value = feed.custom_user_agent || '';
        this.querySelector('#edit-custom-referrer').value = feed.custom_referrer || '';
        this.querySelector('#edit-http2-disabled').checked = !!feed.http2_disabled;
        this.querySelector('#edit-modal').showModal();
    }

    async _fetchMetadata() {
        const urlInput = this.querySelector('#edit-url');
        const url = urlInput.value.trim();
        if (!url) return window.flash.error('URL cannot be empty');
        if (!confirm('Fetch metadata from URL? This will overwrite current values.')) return;
        try {
            const r = await fetch('/api/feeds/fetch-metadata', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ url }),
            });
            if (!r.ok) {
                const err = await r.json().catch(() => ({}));
                throw new Error(err.error || 'Failed to fetch metadata');
            }
            const md = await r.json();
            this.querySelector('#edit-url').value = md.feed_url;
            this.querySelector('#edit-title').value = md.title || '';
            this.querySelector('#edit-description').value = md.description || '';
            this.querySelector('#edit-site-url').value = md.site_url || '';
            window.flash.success('Metadata fetched.');
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _onSave(e) {
        e.preventDefault();
        const streamId = this.querySelector('#edit-id').value;
        const url = this.querySelector('#edit-url').value.trim();
        if (!url) return window.flash.error('URL cannot be empty');
        const title = this.querySelector('#edit-title').value.trim();
        const description = this.querySelector('#edit-description').value.trim();
        const siteUrl = this.querySelector('#edit-site-url').value.trim();
        const category = this.querySelector('#edit-category').value;
        const ua = this.querySelector('#edit-custom-user-agent').value.trim();
        const referrer = this.querySelector('#edit-custom-referrer').value.trim();
        const http2Disabled = this.querySelector('#edit-http2-disabled').checked;
        try {
            const body = new URLSearchParams();
            body.set('ac', 'edit');
            body.set('s', streamId);
            if (title) body.set('t', title);
            if (category) body.set('a', category);
            body.set('description', description);
            body.set('site_url', siteUrl);
            body.set('custom_user_agent', ua);
            body.set('custom_referrer', referrer);
            body.set('http2_disabled', http2Disabled ? 'true' : 'false');
            const r = await fetch('/reader/api/0/subscription/edit', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to update feed');
            this.querySelector('#edit-modal').close();
            window.flash.redirect(window.location.pathname + window.location.search, 'success', 'Feed updated.');
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _delete(id) {
        const feed = this._findFeed(id);
        if (!feed) return;
        if (!confirm(`Delete feed "${feed.title}"? This cannot be undone.`)) return;
        try {
            const body = new URLSearchParams();
            body.set('ac', 'unsubscribe');
            body.set('s', `feed/${feed.url}`);
            const r = await fetch('/reader/api/0/subscription/edit', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to delete feed');
            window.flash.redirect(window.location.pathname + window.location.search, 'success', `Feed "${feed.title}" deleted.`);
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _refresh(id) {
        const btn = this.querySelector(`#refresh-${id}`);
        const row = btn?.closest('tr');
        const original = btn?.textContent;
        if (btn) {
            btn.textContent = '...';
        }
        row?.classList.add('feed-row-syncing');
        this._activeSyncs.add(id);
        this._updateSyncStatus();
        try {
            const r = await fetch(`/api/feeds/${id}/refresh`, { method: 'POST' });
            if (!r.ok) {
                const err = await r.json().catch(() => ({}));
                throw new Error(err.error || 'Failed to refresh feed');
            }
            const result = await r.json();
            window.flash.redirect(
                window.location.pathname + window.location.search,
                'success',
                `Refreshed: ${result.new_entries} new, ${result.updated_entries} updated.`
            );
        } catch (err) {
            window.flash.error(err.message);
            if (btn) btn.textContent = original;
            row?.classList.remove('feed-row-syncing');
            this._activeSyncs.delete(id);
            this._updateSyncStatus();
        }
    }

    _updateSyncStatus() {
        const bar = this.querySelector('#sync-status-bar');
        const text = this.querySelector('#sync-status-text');
        if (!bar || !text) return;
        if (this._activeSyncs.size > 0) {
            bar.classList.add('active');
            text.textContent = this._activeSyncs.size === 1
                ? 'Syncing 1 feed...'
                : `Syncing ${this._activeSyncs.size} feeds...`;
        } else {
            bar.classList.remove('active');
        }
    }

    _loadOpmlFile(input) {
        const file = input.files[0];
        if (!file) return;
        const reader = new FileReader();
        reader.onload = (e) => { this.querySelector('#import-content').value = e.target.result; };
        reader.onerror = () => window.flash.error('Failed to read file');
        reader.readAsText(file);
    }

    async _onImport(e) {
        e.preventDefault();
        const content = this.querySelector('#import-content').value.trim();
        if (!content) return window.flash.error('Please paste OPML content or upload a file');
        const btn = this.querySelector('#import-btn');
        btn.textContent = 'Importing...';
        btn.disabled = true;
        try {
            const r = await fetch('/reader/api/0/subscription/import', {
                method: 'POST',
                headers: { 'Content-Type': 'application/xml' },
                body: content,
            });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to import OPML');
            this.querySelector('#import-modal').close();
            window.flash.redirect(window.location.pathname, 'success', 'OPML imported successfully.');
        } catch (err) {
            window.flash.error(err.message);
        } finally {
            btn.textContent = 'Import';
            btn.disabled = false;
        }
    }
}

customElements.define('rdrs-feeds-page', RdrsFeedsPage);
