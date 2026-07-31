// static/js/register.js — the /register form.
//
// Extracted verbatim from an inline <script> in register.html so the page
// survives a strict `script-src 'self'` (see middleware::security_headers).
// `window.flash` comes from components/rdrs-flash.js, which base.html loads
// ahead of this module.

const form = document.getElementById('register-form');
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
            const response = await fetch('/api/register', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ username, password })
            });

            if (response.ok) {
                flash.redirect('/login', 'success', 'Registration successful! Please login.');
            } else {
                const data = await response.json();
                errorDiv.textContent = data.error || 'Registration failed';
                errorDiv.hidden = false;
            }
        } catch (err) {
            errorDiv.textContent = 'An error occurred. Please try again.';
            errorDiv.hidden = false;
        }
    });
}
