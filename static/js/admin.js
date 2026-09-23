// Copy button for the one-time account link. `navigator.clipboard` is missing
// over plain HTTP, so the fallback selects the readonly <input> and asks for ⌘C.

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
        // Already selected: tell the reader what to do rather than report failure.
        flashLabel(button, 'Press ⌘C');
    }
});
