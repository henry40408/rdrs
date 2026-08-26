// static/js/admin.js — the copy button on the one-time account link.
//
// The link is shown exactly once (only its HMAC is stored), so copying has to be
// reliable rather than pretty. `navigator.clipboard` needs a secure context, and
// a self-hosted rdrs reached over plain HTTP on a LAN has no clipboard API at
// all, so the fallback selects the text and says to press ⌘C. That is also why
// the link lives in a readonly <input> rather than a <code> block.

function flashLabel(button, text) {
    const label = button.querySelector('.action-label') || button;
    const original = label.textContent;
    label.textContent = text;
    setTimeout(() => { label.textContent = original; }, 2000);
}

document.addEventListener('click', async (e) => {
    const button = e.target.closest('[data-copy-target]');
    if (!button) return;

    const input = document.querySelector(button.dataset.copyTarget);
    if (!input) return;

    input.focus();
    input.select();

    if (!navigator.clipboard) {
        flashLabel(button, 'Press ⌘C');
        return;
    }

    try {
        await navigator.clipboard.writeText(input.value);
        flashLabel(button, 'Copied!');
    } catch {
        // The text is already selected, so say what to do rather than report a
        // failure the reader cannot act on.
        flashLabel(button, 'Press ⌘C');
    }
});
