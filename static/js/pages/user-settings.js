// <rdrs-user-settings-page> — CSR replacement for the SSR /user-settings page.
//
// Loads bundled payload from /api/me + /api/user-settings on mount and
// renders multiple sections (account, GReader URLs, password, passkeys,
// display prefs, integrations). Mutations flow through the existing
// per-resource endpoints; passkey CRUD is delegated to /api/passkeys/*.

import { escapeHtml } from '/static/js/utils.js';

function base64urlToBuffer(base64url) {
    const padding = '='.repeat((4 - base64url.length % 4) % 4);
    const base64 = base64url.replace(/-/g, '+').replace(/_/g, '/') + padding;
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return bytes.buffer;
}

function bufferToBase64url(buffer) {
    const bytes = new Uint8Array(buffer);
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
}

class RdrsUserSettingsPage extends HTMLElement {
    connectedCallback() {
        this._renderShell();
        this.load();
    }

    _renderShell() {
        this.innerHTML = `
<div class="app-layout">
<rdrs-sidebar active="user-settings"></rdrs-sidebar>
<main class="main-content">
    <div class="page-content">
    <rdrs-flash class="flash-container"></rdrs-flash>
    <h1>User Settings</h1>
    <div id="user-settings-body">
        <p class="muted">Loading&hellip;</p>
    </div>
    </div>
</main>
</div>`;
    }

    async load() {
        try {
            const [meR, settingsR] = await Promise.all([
                fetch('/api/me', { credentials: 'same-origin' }),
                fetch('/api/user-settings', { credentials: 'same-origin' }),
            ]);
            if (!meR.ok || !settingsR.ok) throw new Error('Failed to load settings');
            this._me = await meR.json();
            this._settings = await settingsR.json();
            this._render();
        } catch (err) {
            this.querySelector('#user-settings-body').innerHTML =
                `<p class="error">${escapeHtml(err.message || 'Failed to load settings')}</p>`;
        }
    }

    _render() {
        const me = this._me;
        const s = this._settings;
        const username = escapeHtml(me.username);
        const role = escapeHtml(me.role);
        const created = escapeHtml(me.created_at);
        const loggedIn = escapeHtml(me.session_created_at);

        this.querySelector('#user-settings-body').innerHTML = `
<h2>Account Information</h2>
<table>
  <tr><th class="settings-th">Username</th><td>${username}</td></tr>
  <tr><th class="settings-th">Role</th><td>${role}</td></tr>
  <tr><th class="settings-th">Registered</th><td>${created}</td></tr>
  <tr><th class="settings-th">Logged In</th><td>${loggedIn}</td></tr>
</table>

<h2>RSS Client (Google Reader API)</h2>
<p class="muted">Connect any RSS reader that supports the Google Reader API (e.g., Reeder, NetNewsWire, FeedMe, Read You, FreshRSS).</p>
<table>
  <tr><th class="settings-th">Server URL</th><td><code id="greader-server-url"></code></td></tr>
  <tr><th class="settings-th">Username</th><td><code>${username}</code></td></tr>
  <tr><th class="settings-th">Password</th><td class="muted">Your RDRS password</td></tr>
</table>
<p class="muted">
  In your RSS client, choose "Google Reader" or "FreshRSS" as the account type and enter the server URL above with your credentials.
  Some clients may require the FreshRSS-compatible URL: <code id="greader-freshrss-url"></code>
</p>

<hr>

<h2>Change Password</h2>
<div id="password-error" class="error" style="display:none"></div>
<form id="change-password-form">
  <div class="form-group">
    <label for="current-password">Current Password</label>
    <input type="password" id="current-password" required autocomplete="current-password">
  </div>
  <div class="form-group">
    <label for="new-password">New Password</label>
    <input type="password" id="new-password" required minlength="6" autocomplete="new-password">
  </div>
  <div class="form-group">
    <label for="confirm-password">Confirm New Password</label>
    <input type="password" id="confirm-password" required minlength="6" autocomplete="new-password">
  </div>
  <button type="submit">Change Password</button>
</form>

<hr>

<h2>Passkeys</h2>
<p class="muted">Passkeys let you sign in without a password using your device's biometrics or security key.</p>
<div id="passkey-unsupported" style="display:none">
    <p class="error">Your browser does not support passkeys.</p>
</div>
<div id="passkey-section" style="display:none">
    <div id="passkey-error" class="error" style="display:none"></div>
    <h3>Registered Passkeys</h3>
    <div id="passkeys-list"><p class="muted">Loading...</p></div>
    <h3>Register New Passkey</h3>
    <form id="register-passkey-form">
        <div class="form-group">
            <label for="passkey-name">Passkey Name</label>
            <input type="text" id="passkey-name" required placeholder="e.g., MacBook Touch ID">
        </div>
        <button type="submit" id="register-passkey-btn">Register Passkey</button>
    </form>
</div>

<hr>

<h2>Display Preferences</h2>
<div id="settings-error" class="error" style="display:none"></div>
<form id="settings-form">
  <div class="form-group">
      <label for="theme-select">Theme</label>
      <select id="theme-select" data-testid="theme-select">
          <option value="system"${!s.theme ? ' selected' : ''}>System (auto)</option>
          <option value="light"${s.theme === 'light' ? ' selected' : ''}>Light</option>
          <option value="dark"${s.theme === 'dark' ? ' selected' : ''}>Dark</option>
      </select>
  </div>
  <div class="form-group">
    <label for="entries-per-page">Entries per page</label>
    <input type="number" id="entries-per-page" value="${s.entries_per_page}" min="10" max="100" required>
    <span class="muted" style="font-size:var(--font-xs);">(10-100)</span>
  </div>
  <button type="submit">Save Preferences</button>
</form>

<hr>

<h2>Integrations</h2>
<p class="muted">Connect external services to save articles.</p>

<h3>Linkding</h3>
<p class="muted">
  <a href="https://github.com/sissbruecker/linkding" target="_blank" rel="noopener noreferrer">Linkding</a>
  is a self-hosted bookmark manager.
  <span id="linkding-status" class="success-text"${s.linkding_configured ? '' : ' style="display:none"'}>Configured</span>
</p>
<div id="linkding-error" class="error" style="display:none"></div>
<form id="linkding-form">
  <div class="form-group">
    <label for="linkding-api-url">API URL</label>
    <input type="url" id="linkding-api-url" value="${escapeHtml(s.linkding_api_url)}" placeholder="https://linkding.example.com">
  </div>
  <div class="form-group">
    <label for="linkding-api-token">API Token</label>
    <input type="password" id="linkding-api-token" placeholder="${s.linkding_configured ? '(unchanged)' : 'Enter your API token'}">
  </div>
  <button type="submit">Save Linkding Settings</button>
  <button type="button" id="linkding-clear-btn" class="btn-secondary"${s.linkding_configured ? '' : ' style="display:none"'}>Clear</button>
</form>

<h3>Kagi Universal Summarizer</h3>
<p class="muted">
  <a href="https://kagi.com/summarizer" target="_blank" rel="noopener noreferrer">Kagi Universal Summarizer</a>
  provides AI-powered article summaries.
  <span id="kagi-status" class="success-text"${s.kagi_configured ? '' : ' style="display:none"'}>Configured</span>
</p>
<div id="kagi-error" class="error" style="display:none"></div>
<form id="kagi-form">
  <div class="form-group">
    <label for="kagi-session-link">Session Link</label>
    <input type="text" id="kagi-session-link" placeholder="${s.kagi_configured ? '(unchanged)' : 'Paste your session link'}">
  </div>
  <div class="form-group">
    <label for="kagi-language">Target Language</label>
    <select id="kagi-language">
      <option value="">Auto-detect</option>
      ${[
          ['EN', 'English'], ['ZH-HANT', '繁體中文'], ['ZH-CN', '简体中文'],
          ['JA', '日本語'], ['KO', '한국어'], ['DE', 'Deutsch'], ['FR', 'Français'],
          ['ES', 'Español'], ['PT', 'Português'],
      ].map(([v, label]) =>
          `<option value="${v}"${s.kagi_language === v ? ' selected' : ''}>${label}</option>`
      ).join('')}
    </select>
  </div>
  <button type="submit">Save Kagi Settings</button>
  <button type="button" id="kagi-clear-btn" class="btn-secondary"${s.kagi_configured ? '' : ' style="display:none"'}>Clear</button>
</form>`;

        this.querySelector('#greader-server-url').textContent = window.location.origin;
        this.querySelector('#greader-freshrss-url').textContent = window.location.origin + '/api/greader.php';

        this.querySelector('#change-password-form').addEventListener('submit', (e) => this._onChangePassword(e));
        this.querySelector('#settings-form').addEventListener('submit', (e) => this._onSavePrefs(e));
        this.querySelector('#theme-select').addEventListener('change', (e) => this._previewTheme(e.target.value));
        this.querySelector('#linkding-form').addEventListener('submit', (e) => this._onSaveLinkding(e));
        this.querySelector('#linkding-clear-btn').addEventListener('click', () => this._onClearLinkding());
        this.querySelector('#kagi-form').addEventListener('submit', (e) => this._onSaveKagi(e));
        this.querySelector('#kagi-clear-btn').addEventListener('click', () => this._onClearKagi());

        this._initPasskeys();
    }

    _previewTheme(theme) {
        if (theme === 'system' || !theme) {
            document.documentElement.removeAttribute('data-theme');
        } else {
            document.documentElement.setAttribute('data-theme', theme);
        }
    }

    async _onChangePassword(e) {
        e.preventDefault();
        const errorDiv = this.querySelector('#password-error');
        errorDiv.style.display = 'none';
        const cur = this.querySelector('#current-password').value;
        const next = this.querySelector('#new-password').value;
        const confirm = this.querySelector('#confirm-password').value;
        if (next !== confirm) {
            errorDiv.textContent = 'New passwords do not match';
            errorDiv.style.display = 'block';
            return;
        }
        try {
            const r = await fetch('/api/user/password', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ current_password: cur, new_password: next }),
            });
            if (r.ok) {
                window.flash.redirect('/login', 'success', 'Password changed successfully. Please login with your new password.');
            } else {
                const data = await r.json().catch(() => ({}));
                errorDiv.textContent = data.error || 'Failed to change password';
                errorDiv.style.display = 'block';
            }
        } catch {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.style.display = 'block';
        }
    }

    async _onSavePrefs(e) {
        e.preventDefault();
        const errorDiv = this.querySelector('#settings-error');
        errorDiv.style.display = 'none';
        const epp = parseInt(this.querySelector('#entries-per-page').value, 10);
        const theme = this.querySelector('#theme-select').value;
        if (epp < 10 || epp > 100) {
            errorDiv.textContent = 'Entries per page must be between 10 and 100';
            errorDiv.style.display = 'block';
            return;
        }
        try {
            const r = await fetch('/api/user/settings', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ entries_per_page: epp }),
            });
            if (!r.ok) {
                const data = await r.json().catch(() => ({}));
                errorDiv.textContent = data.error || 'Failed to save preferences';
                errorDiv.style.display = 'block';
                return;
            }
            await window.theme.syncToServer(theme);
            window.flash.success('Preferences saved successfully.');
        } catch {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.style.display = 'block';
        }
    }

    async _onSaveLinkding(e) {
        e.preventDefault();
        const errorDiv = this.querySelector('#linkding-error');
        errorDiv.style.display = 'none';
        const apiUrl = this.querySelector('#linkding-api-url').value.trim();
        const apiToken = this.querySelector('#linkding-api-token').value;
        if (apiUrl && !apiToken && !this._settings.linkding_configured) {
            errorDiv.textContent = 'API token is required';
            errorDiv.style.display = 'block';
            return;
        }
        try {
            const body = {};
            if (apiUrl) body.api_url = apiUrl;
            if (apiToken) body.api_token = apiToken;
            const r = await fetch('/api/user/settings/linkding', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            if (r.ok) {
                const data = await r.json();
                window.flash.success(data.configured ? 'Linkding settings saved successfully.' : 'Linkding settings cleared.');
                setTimeout(() => location.reload(), 1000);
            } else {
                const data = await r.json().catch(() => ({}));
                errorDiv.textContent = data.error || 'Failed to save Linkding settings';
                errorDiv.style.display = 'block';
            }
        } catch {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.style.display = 'block';
        }
    }

    async _onClearLinkding() {
        if (!confirm('Clear Linkding settings?')) return;
        try {
            const r = await fetch('/api/user/settings/linkding', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({}),
            });
            if (r.ok) {
                window.flash.success('Linkding settings cleared.');
                setTimeout(() => location.reload(), 1000);
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to clear Linkding settings');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    async _onSaveKagi(e) {
        e.preventDefault();
        const errorDiv = this.querySelector('#kagi-error');
        errorDiv.style.display = 'none';
        const sessionLink = this.querySelector('#kagi-session-link').value;
        const language = this.querySelector('#kagi-language').value;
        if (!sessionLink && !this._settings.kagi_configured) {
            errorDiv.textContent = 'Session link is required';
            errorDiv.style.display = 'block';
            return;
        }
        try {
            const body = {};
            if (sessionLink) body.session_link = sessionLink;
            body.language = language || null;
            const r = await fetch('/api/user/settings/kagi', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            if (r.ok) {
                const data = await r.json();
                window.flash.success(data.configured ? 'Kagi settings saved successfully.' : 'Kagi settings cleared.');
                setTimeout(() => location.reload(), 1000);
            } else {
                const data = await r.json().catch(() => ({}));
                errorDiv.textContent = data.error || 'Failed to save Kagi settings';
                errorDiv.style.display = 'block';
            }
        } catch {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.style.display = 'block';
        }
    }

    async _onClearKagi() {
        if (!confirm('Clear Kagi settings?')) return;
        try {
            const r = await fetch('/api/user/settings/kagi', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({}),
            });
            if (r.ok) {
                window.flash.success('Kagi settings cleared.');
                setTimeout(() => location.reload(), 1000);
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to clear Kagi settings');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    _initPasskeys() {
        const supported = window.PublicKeyCredential !== undefined;
        if (!supported) {
            this.querySelector('#passkey-unsupported').style.display = 'block';
            return;
        }
        this.querySelector('#passkey-section').style.display = 'block';
        this.querySelector('#register-passkey-form').addEventListener('submit', (e) => this._onRegisterPasskey(e));
        this._loadPasskeys();
    }

    async _loadPasskeys() {
        const list = this.querySelector('#passkeys-list');
        try {
            const r = await fetch('/api/passkeys', { credentials: 'same-origin' });
            if (!r.ok) throw new Error();
            const data = await r.json();
            if (data.passkeys.length === 0) {
                list.innerHTML = '<p class="muted">No passkeys registered yet.</p>';
                return;
            }
            list.innerHTML = '<table><thead><tr><th>Name</th><th>Created</th><th>Last Used</th><th>Actions</th></tr></thead><tbody>'
                + data.passkeys.map(p => `
                <tr id="passkey-row-${p.id}">
                    <td><span id="passkey-name-${p.id}">${escapeHtml(p.name)}</span></td>
                    <td>${escapeHtml(p.created_at)}</td>
                    <td>${escapeHtml(p.last_used_at || 'Never')}</td>
                    <td class="actions">
                        <a href="#" data-passkey-action="rename" data-passkey-id="${p.id}">Rename</a>
                        <a href="#" data-passkey-action="delete" data-passkey-id="${p.id}">Delete</a>
                    </td>
                </tr>`).join('')
                + '</tbody></table>';
            list.querySelectorAll('[data-passkey-action]').forEach(el => {
                el.addEventListener('click', (e) => {
                    e.preventDefault();
                    const id = parseInt(el.dataset.passkeyId, 10);
                    if (el.dataset.passkeyAction === 'rename') this._renamePasskey(id);
                    else if (el.dataset.passkeyAction === 'delete') this._deletePasskey(id);
                });
            });
        } catch {
            list.innerHTML = '<p class="error">Failed to load passkeys.</p>';
        }
    }

    async _renamePasskey(id) {
        const cur = this.querySelector(`#passkey-name-${id}`).textContent;
        const next = prompt('Enter new name:', cur);
        if (!next || next === cur) return;
        try {
            const r = await fetch(`/api/passkeys/${id}`, {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ name: next }),
            });
            if (r.ok) {
                window.flash.success('Passkey renamed successfully.');
                this._loadPasskeys();
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to rename passkey');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    async _deletePasskey(id) {
        if (!confirm('Are you sure you want to delete this passkey?')) return;
        try {
            const r = await fetch(`/api/passkeys/${id}`, { method: 'DELETE' });
            if (r.ok) {
                window.flash.success('Passkey deleted successfully.');
                this._loadPasskeys();
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to delete passkey');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    async _onRegisterPasskey(e) {
        e.preventDefault();
        const errorDiv = this.querySelector('#passkey-error');
        const btn = this.querySelector('#register-passkey-btn');
        const nameInput = this.querySelector('#passkey-name');
        errorDiv.style.display = 'none';
        const name = nameInput.value.trim();
        if (!name) {
            errorDiv.textContent = 'Passkey name is required';
            errorDiv.style.display = 'block';
            return;
        }
        try {
            btn.disabled = true;
            btn.textContent = 'Registering...';
            const startR = await fetch('/api/passkey/register/start', {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
            });
            if (!startR.ok) {
                const data = await startR.json().catch(() => ({}));
                throw new Error(data.error || 'Failed to start registration');
            }
            const { options } = await startR.json();
            const publicKey = {
                ...options.publicKey,
                challenge: base64urlToBuffer(options.publicKey.challenge),
                user: { ...options.publicKey.user, id: base64urlToBuffer(options.publicKey.user.id) },
                excludeCredentials: options.publicKey.excludeCredentials?.map(c => ({
                    ...c, id: base64urlToBuffer(c.id),
                })) || [],
            };
            const credential = await navigator.credentials.create({ publicKey });
            const credForServer = {
                id: credential.id,
                rawId: bufferToBase64url(credential.rawId),
                type: credential.type,
                response: {
                    attestationObject: bufferToBase64url(credential.response.attestationObject),
                    clientDataJSON: bufferToBase64url(credential.response.clientDataJSON),
                },
            };
            if (credential.response.getTransports) {
                credForServer.response.transports = credential.response.getTransports();
            }
            const finishR = await fetch('/api/passkey/register/finish', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ name, credential: credForServer }),
            });
            if (finishR.ok) {
                window.flash.success('Passkey registered successfully.');
                nameInput.value = '';
                this._loadPasskeys();
            } else {
                const data = await finishR.json().catch(() => ({}));
                throw new Error(data.error || 'Registration failed');
            }
        } catch (err) {
            if (err.name === 'NotAllowedError') {
                errorDiv.textContent = 'Registration was cancelled or timed out.';
            } else if (err.name === 'InvalidStateError') {
                errorDiv.textContent = 'This passkey is already registered.';
            } else {
                errorDiv.textContent = err.message || 'An error occurred. Please try again.';
            }
            errorDiv.style.display = 'block';
        } finally {
            btn.disabled = false;
            btn.textContent = 'Register Passkey';
        }
    }
}

customElements.define('rdrs-user-settings-page', RdrsUserSettingsPage);
