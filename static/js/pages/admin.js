// <rdrs-admin-page> — CSR replacement for the SSR admin panel.
// Loads users from /api/admin/users; identifies "self" rows from /api/me
// (covers both the current effective user and the original admin while
// masquerading) so destructive actions are hidden for them.

import { escapeHtml } from '/static/js/utils.js';

class RdrsAdminPage extends HTMLElement {
    connectedCallback() {
        this._renderShell();
        this._renderRows({ loading: true });
        this.load();
    }

    _renderShell() {
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="admin"></rdrs-sidebar>
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <h1>Admin Panel</h1>
    <table class="mobile-cards">
        <thead>
            <tr>
                <th>ID</th><th>Username</th><th>Role</th><th>Status</th><th>Created</th><th>Actions</th>
            </tr>
        </thead>
        <tbody id="users-table"></tbody>
    </table>
    </div>
</main>
</div>`;
        this.querySelector('#users-table').addEventListener('click', (e) => this._onClick(e));
    }

    async load() {
        try {
            const [meR, usersR] = await Promise.all([
                fetch('/api/me', { credentials: 'same-origin' }),
                fetch('/api/admin/users', { credentials: 'same-origin' }),
            ]);
            if (!meR.ok || !usersR.ok) throw new Error('Failed to load users');
            this._me = await meR.json();
            this._users = await usersR.json();
            this._renderRows({ users: this._users });
        } catch (err) {
            this._renderRows({ error: err.message || 'Failed to load users' });
        }
    }

    _renderRows(state) {
        const tbody = this.querySelector('#users-table');
        if (!tbody) return;
        const { loading, error, users } = state;
        if (loading) {
            tbody.innerHTML = `<tr><td colspan="6" class="muted">Loading&hellip;</td></tr>`;
            return;
        }
        if (error) {
            tbody.innerHTML = `<tr><td colspan="6">${escapeHtml(error)}</td></tr>`;
            return;
        }
        if (!users || users.length === 0) {
            tbody.innerHTML = `<tr><td colspan="6" class="muted">No users found.</td></tr>`;
            return;
        }
        const me = this._me || {};
        const lockedIds = new Set([me.id, me.original_user_id].filter(v => v != null));
        tbody.innerHTML = users.map(u => {
            const isDisabled = !!u.disabled_at;
            const role = escapeHtml(u.role);
            const statusCell = isDisabled
                ? '<span class="error-text">disabled</span>'
                : '<span class="success-text">active</span>';
            const created = u.created_at ? escapeHtml(String(u.created_at).slice(0, 10)) : '';
            const isSelf = lockedIds.has(u.id);
            const actions = isSelf
                ? `<span class="muted">(you)</span>`
                : `
                    <a href="#" data-action="toggle-role" data-user-id="${u.id}" data-role="${role}">${u.role === 'admin' ? 'demote' : 'promote'}</a>
                    <a href="#" data-action="toggle-disabled" data-user-id="${u.id}" data-disabled="${isDisabled}">${isDisabled ? 'enable' : 'disable'}</a>
                    <a href="#" data-action="masquerade" data-user-id="${u.id}">view as</a>
                    <a href="#" data-action="delete" data-user-id="${u.id}">delete</a>`;
            return `
            <tr>
                <td data-label="ID">${u.id}</td>
                <td data-label="Username">${escapeHtml(u.username)}</td>
                <td data-label="Role">${role}</td>
                <td data-label="Status">${statusCell}</td>
                <td data-label="Created">${created}</td>
                <td class="actions">${actions}</td>
            </tr>`;
        }).join('');
    }

    _onClick(e) {
        const target = e.target.closest('[data-action]');
        if (!target) return;
        e.preventDefault();
        const action = target.dataset.action;
        const userId = parseInt(target.dataset.userId, 10);
        if (action === 'toggle-role') this._toggleRole(userId, target.dataset.role);
        else if (action === 'toggle-disabled') this._toggleDisabled(userId, target.dataset.disabled === 'true');
        else if (action === 'masquerade') this._masquerade(userId);
        else if (action === 'delete') {
            const username = target.closest('tr').querySelector('[data-label="Username"]').textContent;
            this._delete(userId, username);
        }
    }

    async _toggleRole(userId, currentRole) {
        const newRole = currentRole === 'admin' ? 'user' : 'admin';
        try {
            const r = await fetch(`/api/admin/users/${userId}`, {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ role: newRole }),
            });
            if (!r.ok) {
                const data = await r.json().catch(() => ({}));
                throw new Error(data.error || 'Failed to update role');
            }
            window.flash.success(`Role updated to ${newRole}.`);
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _toggleDisabled(userId, isCurrentlyDisabled) {
        try {
            const r = await fetch(`/api/admin/users/${userId}`, {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ disabled: !isCurrentlyDisabled }),
            });
            if (!r.ok) {
                const data = await r.json().catch(() => ({}));
                throw new Error(data.error || 'Failed to update user status');
            }
            window.flash.success(isCurrentlyDisabled ? 'User enabled.' : 'User disabled.');
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _masquerade(userId) {
        try {
            const r = await fetch(`/api/admin/masquerade/${userId}`, { method: 'POST' });
            if (!r.ok) {
                const data = await r.json().catch(() => ({}));
                throw new Error(data.error || 'Failed to start masquerade');
            }
            window.flash.redirect('/', 'info', 'Now viewing as another user.');
        } catch (err) {
            window.flash.error(err.message);
        }
    }

    async _delete(userId, username) {
        if (!confirm(`Delete user "${username}"? This cannot be undone.`)) return;
        try {
            const r = await fetch(`/api/admin/users/${userId}`, { method: 'DELETE' });
            if (!r.ok) {
                const data = await r.json().catch(() => ({}));
                throw new Error(data.error || 'Failed to delete user');
            }
            window.flash.success(`User "${username}" deleted.`);
            this.load();
        } catch (err) {
            window.flash.error(err.message);
        }
    }
}

customElements.define('rdrs-admin-page', RdrsAdminPage);
