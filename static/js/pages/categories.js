// <rdrs-categories-page> — CSR replacement for the SSR categories page.
//
// CRUD goes through the existing GReader endpoints (which are also used by
// FreshRSS-compatible clients):
//   GET  /reader/api/0/tag/list           — list categories
//   GET  /reader/api/0/subscription/list  — feeds (used to count per category)
//   POST /reader/api/0/rename-tag         — create + rename
//   POST /reader/api/0/disable-tag        — delete
//
// DOM/testid hooks match the old SSR template (categories-table,
// category-name-input, add-category-btn, rename/save/cancel/delete links,
// table.mobile-cards) so e2e specs and CSS stay green.

import { escapeHtml } from '/static/js/utils.js';

class RdrsCategoriesPage extends HTMLElement {
    connectedCallback() {
        this._renderShell();
        this._renderRows({ loading: true });
        this.load();
    }

    /// Render the full page structure once. Subsequent updates touch only
    /// the tbody — that way `<rdrs-flash>` and its messages survive reloads.
    _renderShell() {
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="categories"></rdrs-sidebar>
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <h1>Categories</h1>
    <form id="add-form" data-rdrs-add-form>
        <div class="form-group">
            <label for="name">New Category</label>
            <input type="text" id="name" name="name" placeholder="Category name" required maxlength="100" data-testid="category-name-input">
        </div>
        <button type="submit" data-testid="add-category-btn">Add Category</button>
    </form>
    <hr>
    <table class="mobile-cards">
        <thead>
            <tr>
                <th>Name</th>
                <th>Feeds</th>
                <th>Actions</th>
            </tr>
        </thead>
        <tbody id="categories-table" data-testid="categories-table"></tbody>
    </table>
    </div>
</main>
</div>`;

        this.querySelector('[data-rdrs-add-form]').addEventListener('submit', (e) => this._onAdd(e));
        this.querySelector('#categories-table').addEventListener('click', (e) => this._onTableClick(e));
    }

    _renderRows(state) {
        const tbody = this.querySelector('#categories-table');
        if (!tbody) return;
        const { loading, error, items } = state;
        if (loading) {
            tbody.innerHTML = `<tr><td colspan="3" class="muted" data-testid="categories-loading">Loading&hellip;</td></tr>`;
            return;
        }
        if (error) {
            tbody.innerHTML = `<tr><td colspan="3">${escapeHtml(error)}</td></tr>`;
            return;
        }
        if (!items || items.length === 0) {
            tbody.innerHTML = `<tr><td colspan="3" class="muted">No categories yet.</td></tr>`;
            return;
        }
        tbody.innerHTML = items.map((cat, index) => {
            const safeId = escapeHtml(cat.id);
            const safeName = escapeHtml(cat.name);
            return `
            <tr id="row-${index}" data-tag-id="${safeId}">
                <td data-label="Name">
                    <span class="cat-name">${safeName}</span>
                    <input type="text" class="cat-edit-input" value="${safeName}" style="display:none;" maxlength="100">
                </td>
                <td data-label="Feeds">${cat.feedCount}</td>
                <td class="actions">
                    <a href="/feeds?category=${cat.numericId}">feeds</a>
                    <a href="/categories/${cat.numericId}/entries">entries</a>
                    <a href="#" class="edit-btn" data-action="rename" data-index="${index}">rename</a>
                    <a href="#" class="save-btn" data-action="save" data-index="${index}" style="display:none;">save</a>
                    <a href="#" class="cancel-btn" data-action="cancel" data-index="${index}" style="display:none;">cancel</a>
                    <a href="#" data-action="delete">delete</a>
                </td>
            </tr>`;
        }).join('');
    }

    async load() {
        try {
            const [tagResp, subResp] = await Promise.all([
                fetch('/reader/api/0/tag/list', { credentials: 'same-origin' }),
                fetch('/reader/api/0/subscription/list', { credentials: 'same-origin' }),
            ]);
            if (!tagResp.ok) throw new Error('Failed to load categories');
            const tagData = await tagResp.json();
            const items = (tagData.tags || [])
                .filter(tag => typeof tag.id === 'string' && tag.id.includes('/label/'))
                .map(tag => {
                    const name = tag.id.split('/label/').pop();
                    const numericId = parseInt(tag.sortid, 16);
                    return { id: tag.id, name, numericId };
                });

            let subs = [];
            if (subResp.ok) {
                const sd = await subResp.json();
                subs = (sd.subscriptions || []).map(s => ({
                    categoryId: s.categories && s.categories.length > 0 ? s.categories[0].id : '',
                }));
            }
            for (const it of items) {
                it.feedCount = subs.filter(s => s.categoryId === it.id).length;
            }

            this._items = items;
            this._renderRows({ items });
        } catch (err) {
            this._renderRows({ error: 'Failed to load categories' });
        }
    }

    async _onAdd(e) {
        e.preventDefault();
        const input = this.querySelector('#name');
        const name = input.value.trim();
        if (!name) {
            window.flash.error('Category name cannot be empty');
            return;
        }
        const tagId = `user/-/label/${name}`;
        try {
            const body = new URLSearchParams();
            body.set('s', tagId);
            body.set('dest', tagId);
            const r = await fetch('/reader/api/0/rename-tag', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to create category');
            input.value = '';
            window.flash.success('Category created.');
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    _onTableClick(e) {
        const target = e.target.closest('[data-action]');
        if (!target) return;
        e.preventDefault();
        const action = target.dataset.action;
        const row = target.closest('tr');
        const index = parseInt(target.dataset.index, 10);
        if (action === 'rename') this._startEdit(index);
        else if (action === 'save') this._saveEdit(index);
        else if (action === 'cancel') this._cancelEdit(index);
        else if (action === 'delete') {
            const tagId = row.dataset.tagId;
            const name = row.querySelector('.cat-name').textContent;
            this._delete(tagId, name);
        }
    }

    _startEdit(index) {
        const row = this.querySelector(`#row-${index}`);
        if (!row) return;
        row.querySelector('.cat-name').style.display = 'none';
        row.querySelector('.cat-edit-input').style.display = 'inline';
        row.querySelector('.edit-btn').style.display = 'none';
        row.querySelector('.save-btn').style.display = 'inline';
        row.querySelector('.cancel-btn').style.display = 'inline';
        row.querySelector('.cat-edit-input').focus();
    }

    _cancelEdit(index) {
        const row = this.querySelector(`#row-${index}`);
        if (!row) return;
        const nameSpan = row.querySelector('.cat-name');
        const input = row.querySelector('.cat-edit-input');
        input.value = nameSpan.textContent;
        nameSpan.style.display = 'inline';
        input.style.display = 'none';
        row.querySelector('.edit-btn').style.display = 'inline';
        row.querySelector('.save-btn').style.display = 'none';
        row.querySelector('.cancel-btn').style.display = 'none';
    }

    async _saveEdit(index) {
        const row = this.querySelector(`#row-${index}`);
        if (!row) return;
        const tagId = row.dataset.tagId;
        const newName = row.querySelector('.cat-edit-input').value.trim();
        if (!newName) {
            window.flash.error('Category name cannot be empty');
            return;
        }
        try {
            const body = new URLSearchParams();
            body.set('s', tagId);
            body.set('dest', `user/-/label/${newName}`);
            const r = await fetch('/reader/api/0/rename-tag', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to update category');
            window.flash.success('Category renamed.');
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _delete(tagId, name) {
        const message = `Delete category "${name}"? This cannot be undone.`;
        if (!confirm(message)) return;
        try {
            const body = new URLSearchParams();
            body.set('s', tagId);
            const r = await fetch('/reader/api/0/disable-tag', { method: 'POST', body });
            if (!r.ok) throw new Error((await r.text()) || 'Failed to delete category');
            window.flash.success(`Category "${name}" deleted.`);
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }
}

customElements.define('rdrs-categories-page', RdrsCategoriesPage);
