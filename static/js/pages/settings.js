// <rdrs-settings-page> — CSR replacement for the SSR /settings page.
// Read-only server configuration loaded from GET /api/server-config.

import { escapeHtml } from '/static/js/utils.js';

class RdrsSettingsPage extends HTMLElement {
    connectedCallback() {
        this._renderShell();
        this.load();
    }

    _renderShell() {
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="settings"></rdrs-sidebar>
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <h1>Settings</h1>
    <div id="settings-body">
        <p class="muted">Loading&hellip;</p>
    </div>
    </div>
</main>
</div>`;
    }

    async load() {
        try {
            const r = await fetch('/api/server-config', { credentials: 'same-origin' });
            if (!r.ok) throw new Error(`Failed to load configuration (${r.status})`);
            const cfg = await r.json();
            this._render(cfg);
        } catch (err) {
            this.querySelector('#settings-body').innerHTML =
                `<p class="error">${escapeHtml(err.message || 'Failed to load configuration')}</p>`;
        }
    }

    _render(cfg) {
        const yes = '<span class="success-text">Yes</span>';
        const no = '<span class="muted">No</span>';
        this.querySelector('#settings-body').innerHTML = `
<p class="muted">Version: <code>${escapeHtml(cfg.git_version)}</code></p>

<h2>Configuration</h2>
<p class="muted">These settings are configured via environment variables and cannot be changed at runtime.</p>

<h3>HTTP Client</h3>
<table>
    <tbody>
        <tr>
            <th class="settings-th">User Agent</th>
            <td>
                <code>${escapeHtml(cfg.user_agent)}</code>
                <span class="muted">(${cfg.user_agent_is_default ? 'default' : 'custom'})</span>
            </td>
        </tr>
    </tbody>
</table>

<h3>User Registration</h3>
<table>
    <tbody>
        <tr>
            <th class="settings-th">Signup Enabled</th>
            <td>${cfg.signup_enabled ? yes : no}</td>
        </tr>
        <tr>
            <th class="settings-th">Multi-User Mode</th>
            <td>${cfg.multi_user_enabled ? yes : no}</td>
        </tr>
    </tbody>
</table>

<h3>Environment Variables</h3>
<p class="muted">Configure these environment variables to customize RDRS:</p>
<table class="mobile-cards-settings">
    <thead>
        <tr>
            <th class="settings-th">Variable</th>
            <th class="settings-th">Description</th>
            <th class="settings-th">Default</th>
        </tr>
    </thead>
    <tbody>
        <tr>
            <td><code>DATABASE_URL</code></td>
            <td data-label="Description">SQLite database file path</td>
            <td data-label="Default"><code>rdrs.sqlite3</code></td>
        </tr>
        <tr>
            <td><code>SERVER_PORT</code></td>
            <td data-label="Description">HTTP server port</td>
            <td data-label="Default"><code>3000</code></td>
        </tr>
        <tr>
            <td><code>USER_AGENT</code></td>
            <td data-label="Description">User agent for HTTP requests</td>
            <td data-label="Default"><code>RDRS/{version} (...)</code></td>
        </tr>
        <tr>
            <td><code>SIGNUP_ENABLED</code></td>
            <td data-label="Description">Allow new user registration</td>
            <td data-label="Default"><code>false</code></td>
        </tr>
        <tr>
            <td><code>MULTI_USER_ENABLED</code></td>
            <td data-label="Description">Allow multiple users</td>
            <td data-label="Default"><code>false</code></td>
        </tr>
        <tr>
            <td><code>IMAGE_PROXY_SECRET</code></td>
            <td data-label="Description">Secret key for image proxy URLs</td>
            <td data-label="Default"><em>(auto-generated)</em></td>
        </tr>
    </tbody>
</table>`;
    }
}

customElements.define('rdrs-settings-page', RdrsSettingsPage);
