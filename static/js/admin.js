// static/js/admin.js — the copy button on the one-time account link.
//
// The link is shown exactly once (only its HMAC is stored), so the job here is
// to make copying it reliable rather than pretty. Two things matter:
//
//   - `navigator.clipboard` needs a secure context. A self-hosted rdrs reached
//     over plain HTTP on a LAN has no clipboard API at all, and a button that
//     silently does nothing there is worse than no button. The fallback selects
//     the text so ⌘C/Ctrl-C works, and says so.
//   - The link lives in a readonly <input> rather than a <code> block for the
//     same reason: selectable, and selectable programmatically.

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
        // Permission denied, or a browser that rejects the write outside a
        // gesture it recognises. The text is already selected, so say what to
        // do rather than reporting a failure the user cannot act on.
        flashLabel(button, 'Press ⌘C');
    }
});
