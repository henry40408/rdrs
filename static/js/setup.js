// static/js/setup.js — the one-time /setup form.
//
// Extracted from an inline <script> so the page survives a strict
// `script-src 'self'` (see middleware::security_headers). `window.flash` comes
// from components/rdrs-flash.js, which base.html loads ahead of this module.
//
// This page exists only while the instance has no accounts. Every later
// account is created by an admin and given its password through
// /invite/{token}, which is a plain server-rendered form and needs no
// JavaScript at all.

const form = document.getElementById('setup-form');
if (form) {
    form.addEventListener('submit', async (e) => {
        e.preventDefault();
        const errorDiv = document.getElementById('error');
        errorDiv.hidden = true;

        const username = document.getElementById('username').value;
        const password = document.getElementById('password').value;
        const confirmPassword = document.getElementById('confirm-password').value;

        if (password !== confirmPassword) {
            errorDiv.textContent = 'Passwords do not match';
            errorDiv.hidden = false;
            return;
        }

        try {
            const response = await fetch('/api/setup', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ username, password })
            });

            if (response.ok) {
                flash.redirect('/login', 'success', 'Account created. Please sign in.');
            } else {
                const data = await response.json();
                errorDiv.textContent = data.error || 'Could not create the account';
                errorDiv.hidden = false;
            }
        } catch (err) {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.hidden = false;
        }
    });
}
