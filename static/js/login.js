// static/js/login.js — the /login form and the passkey sign-in button.
//
// Extracted verbatim from an inline <script> in login.html so the page survives
// a strict `script-src 'self'` (see middleware::security_headers). The
// base64url helpers are duplicated in passkey.js, which drives the *enrolment*
// side on /user-settings; the two pages never load each other's module.

const isWebAuthnSupported = window.PublicKeyCredential !== undefined;

if (isWebAuthnSupported) {
    document.getElementById('passkey-section').hidden = false;
}

function base64urlToBuffer(base64url) {
    const padding = '='.repeat((4 - base64url.length % 4) % 4);
    const base64 = base64url.replace(/-/g, '+').replace(/_/g, '/') + padding;
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
}

function bufferToBase64url(buffer) {
    const bytes = new Uint8Array(buffer);
    let binary = '';
    for (let i = 0; i < bytes.length; i++) {
        binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
}

const loginForm = document.getElementById('login-form');
if (loginForm) loginForm.addEventListener('submit', async (e) => {
    e.preventDefault();
    const errorDiv = document.getElementById('error');
    errorDiv.hidden = true;

    const username = document.getElementById('username').value;
    const password = document.getElementById('password').value;

    try {
        const response = await fetch('/api/session', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ username, password })
        });

        if (response.ok) {
            window.location.href = '/';
        } else {
            const data = await response.json();
            errorDiv.textContent = data.error || 'Login failed';
            errorDiv.hidden = false;
        }
    } catch (err) {
        errorDiv.textContent = 'An error occurred. Please try again.';
        errorDiv.hidden = false;
    }
});

document.getElementById('passkey-login-btn')?.addEventListener('click', async () => {
    const errorDiv = document.getElementById('error');
    const btn = document.getElementById('passkey-login-btn');
    errorDiv.hidden = true;

    try {
        btn.disabled = true;
        btn.textContent = 'Authenticating...';

        const startResponse = await fetch('/api/passkey/auth/start', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' }
        });

        if (!startResponse.ok) {
            const data = await startResponse.json();
            throw new Error(data.error || 'Failed to start authentication');
        }

        const { options } = await startResponse.json();

        const publicKey = {
            ...options.publicKey,
            challenge: base64urlToBuffer(options.publicKey.challenge),
            allowCredentials: options.publicKey.allowCredentials?.map(cred => ({
                ...cred,
                id: base64urlToBuffer(cred.id)
            })) || []
        };

        const credential = await navigator.credentials.get({ publicKey });

        const credentialForServer = {
            id: credential.id,
            rawId: bufferToBase64url(credential.rawId),
            type: credential.type,
            response: {
                authenticatorData: bufferToBase64url(credential.response.authenticatorData),
                clientDataJSON: bufferToBase64url(credential.response.clientDataJSON),
                signature: bufferToBase64url(credential.response.signature),
                userHandle: credential.response.userHandle ? bufferToBase64url(credential.response.userHandle) : null
            }
        };

        const finishResponse = await fetch('/api/passkey/auth/finish', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ credential: credentialForServer })
        });

        if (finishResponse.ok) {
            window.location.href = '/';
        } else {
            const data = await finishResponse.json();
            throw new Error(data.error || 'Authentication failed');
        }
    } catch (err) {
        if (err.name === 'NotAllowedError') {
            errorDiv.textContent = 'Authentication was cancelled or timed out.';
        } else {
            errorDiv.textContent = err.message || 'An error occurred. Please try again.';
        }
        errorDiv.hidden = false;
    } finally {
        btn.disabled = false;
        btn.textContent = 'Login with Passkey';
    }
});
