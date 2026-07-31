// static/js/passkey.js — <rdrs-passkeys> custom element.
//
// Self-contained WebAuthn UI: lists registered passkeys, registers
// new ones, supports rename + delete. The SSR /user-settings page
// mounts <rdrs-passkeys></rdrs-passkeys> in the passkey section;
// this element handles all the in-page UX while the underlying
// /api/passkey* and /api/passkeys/* JSON endpoints remain in place
// (WebAuthn requires JS, this is the planned exception).

// `?v=` is substituted at serve time (see handlers/static_assets.rs) so this
// nested import is cache-busted like the top-level <script> tags.
import { escapeHtml } from '/static/js/utils.js?v=__RDRS_ASSET_VERSION__';

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

// Adding or removing a passkey changes which credentials can open the account,
// so the server requires the session to have proved itself within the last few
// minutes (middleware::auth::RecentlyAuthenticated) and answers 403 with this
// exact message otherwise.
const REAUTH_MESSAGE = 'Reauthentication required';

// A <dialog> rather than prompt(): prompt() shows the password in clear text
// and cannot be styled to match the rest of the page.
function promptForPassword() {
    return new Promise((resolve) => {
        const dialog = document.createElement('dialog');
        dialog.className = 'reauth-dialog';
        dialog.innerHTML = `
            <form method="dialog">
                <h2 class="reauth-dialog__title">Confirm it's you</h2>
                <p class="reauth-dialog__body">Changing your passkeys needs your password again.</p>
                <label class="reauth-dialog__label" for="reauth-password">Password</label>
                <input type="password" id="reauth-password" autocomplete="current-password" required>
                <div class="reauth-dialog__actions">
                    <button value="cancel" class="btn-secondary" formnovalidate>Cancel</button>
                    <button value="confirm">Confirm</button>
                </div>
            </form>`;
        document.body.appendChild(dialog);
        const input = dialog.querySelector('#reauth-password');
        dialog.addEventListener('close', () => {
            const value = dialog.returnValue === 'confirm' ? input.value : null;
            dialog.remove();
            resolve(value);
        });
        dialog.showModal();
        input.focus();
    });
}

// Run `send`, and if the server asks for re-authentication, collect the
// password, re-authenticate, and run it again — exactly once. `send` must be
// replayable, which is why the caller passes a thunk rather than a Response:
// the retry re-issues the request from scratch.
async function withReauth(send) {
    let response = await send();
    if (response.status !== 403) return response;

    // Read the body via a clone; the caller still needs the original if this
    // turns out to be an ordinary 403.
    const data = await response.clone().json().catch(() => ({}));
    if (data.error !== REAUTH_MESSAGE) return response;

    const password = await promptForPassword();
    if (password === null) return response;

    const reauth = await fetch('/api/session/reauth', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password }),
    });
    if (!reauth.ok) {
        const err = await reauth.json().catch(() => ({}));
        throw new Error(err.error || 'Re-authentication failed');
    }
    return send();
}

class RdrsPasskeys extends HTMLElement {
    connectedCallback() {
        if (window.PublicKeyCredential === undefined) {
            this.innerHTML = '<p class="error">Your browser does not support passkeys.</p>';
            return;
        }
        this.innerHTML = `
            <div id="passkey-error" class="error" hidden></div>
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
            const r = await withReauth(() => fetch(`/api/passkeys/${id}`, { method: 'DELETE' }));
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
        errorDiv.hidden = true;
        const name = nameInput.value.trim();
        if (!name) {
            errorDiv.textContent = 'Passkey name is required';
            errorDiv.hidden = false;
            return;
        }
        try {
            btn.disabled = true;
            btn.textContent = 'Registering...';
            // Only the start of the ceremony can ask for re-authentication —
            // the server checks there so the password prompt lands before the
            // authenticator prompt, and so a retry still has its challenge.
            const startR = await withReauth(() => fetch('/api/passkey/register/start', {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
            }));
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
            errorDiv.hidden = false;
        } finally {
            btn.disabled = false;
            btn.textContent = 'Register Passkey';
        }
    }
}

customElements.define('rdrs-passkeys', RdrsPasskeys);
