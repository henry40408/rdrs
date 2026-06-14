// static/js/passkey.js — <rdrs-passkeys> custom element.
//
// Self-contained WebAuthn UI: lists registered passkeys, registers
// new ones, supports rename + delete. The SSR /user-settings page
// mounts <rdrs-passkeys></rdrs-passkeys> in the passkey section;
// this element handles all the in-page UX while the underlying
// /api/passkey* and /api/passkeys/* JSON endpoints remain in place
// (WebAuthn requires JS, this is the planned exception).

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

class RdrsPasskeys extends HTMLElement {
    connectedCallback() {
        if (window.PublicKeyCredential === undefined) {
            this.innerHTML = '<p class="error">Your browser does not support passkeys.</p>';
            return;
        }
        this.innerHTML = `
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
        `;
        this.querySelector('#register-passkey-form').addEventListener('submit', (e) => this._onRegister(e));
        this._loadList();
    }

    async _loadList() {
        const list = this.querySelector('#passkeys-list');
        try {
            const r = await fetch('/api/passkeys', { credentials: 'same-origin' });
            if (!r.ok) throw new Error();
            const data = await r.json();
            if (data.passkeys.length === 0) {
                list.innerHTML = '<p class="muted">No passkeys registered yet.</p>';
                return;
            }
            list.innerHTML = '<table class="mobile-cards"><thead><tr><th>Name</th><th>Created</th><th>Last Used</th><th>Actions</th></tr></thead><tbody>'
                + data.passkeys.map(p => `
                <tr id="passkey-row-${p.id}">
                    <td data-label="Name"><span id="passkey-name-${p.id}">${escapeHtml(p.name)}</span></td>
                    <td data-label="Created">${escapeHtml(p.created_at)}</td>
                    <td data-label="Last Used">${escapeHtml(p.last_used_at || 'Never')}</td>
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
                    if (el.dataset.passkeyAction === 'rename') this._rename(id);
                    else if (el.dataset.passkeyAction === 'delete') this._delete(id);
                });
            });
        } catch {
            list.innerHTML = '<p class="error">Failed to load passkeys.</p>';
        }
    }

    async _rename(id) {
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
                this._loadList();
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to rename passkey');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    async _delete(id) {
        if (!confirm('Are you sure you want to delete this passkey?')) return;
        try {
            const r = await fetch(`/api/passkeys/${id}`, { method: 'DELETE' });
            if (r.ok) {
                window.flash.success('Passkey deleted successfully.');
                this._loadList();
            } else {
                const data = await r.json().catch(() => ({}));
                window.flash.error(data.error || 'Failed to delete passkey');
            }
        } catch {
            window.flash.error('An error occurred. Please try again.');
        }
    }

    async _onRegister(e) {
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
                this._loadList();
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

customElements.define('rdrs-passkeys', RdrsPasskeys);
