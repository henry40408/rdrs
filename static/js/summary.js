// Summarize / Dismiss / Copy behavior for the Kagi summary box. The two DOM
// predicates are exported because the swap engine and the keyboard use them too.

// Shared by the Summarize button and 'a': dismiss a showing summary via its
// Dismiss control. False when none is showing.
export function dismissVisibleSummary() {
    const dismiss = document.querySelector('#reading-pane [data-summary-dismiss]');
    if (!dismiss) return false;
    dismiss.click();
    return true;
}

// True mid-generation, when only Cancel should act.
export function summaryInFlight() {
    return !!document.querySelector('#reading-pane [data-summary-pending]');
}

// Keep the toggle's label/icon/aria-label in sync; skipped mid-request so the
// "Summarizing…" label survives.
function syncSummarizeToggleLabel() {
    const form = document.querySelector('#reading-pane [data-summary-toggle]');
    if (!form || form.getAttribute('aria-busy') === 'true') return;
    const btn = form.querySelector('button');
    if (!btn) return;
    const showing = !!document.querySelector('#reading-pane [data-summary-dismiss]');
    // Matches the server render; handler gates cover the gap.
    btn.disabled = summaryInFlight();
    const labelEl = btn.querySelector('.action-label');
    if (labelEl) labelEl.textContent = showing ? 'Dismiss' : 'Summary';
    btn.setAttribute('aria-label', showing ? 'Dismiss summary' : 'Summarize');
    const summarizeIcon = btn.querySelector('.action-icon-summarize');
    const dismissIcon = btn.querySelector('.action-icon-dismiss');
    if (summarizeIcon) summarizeIcon.hidden = showing;
    if (dismissIcon) dismissIcon.hidden = !showing;
}

// Copy to clipboard; Dismiss DELETEs the summary and strips it and the row badge.
export function installSummaryActions({ currentPaneEntryId }) {
    document.addEventListener('rdrs:swap-complete', syncSummarizeToggleLabel);
    document.addEventListener('click', async (e) => {
        const copyBtn = e.target.closest('[data-summary-copy]');
        if (copyBtn) {
            // Scoped to the box, so what is copied matches what is visible.
            const box = copyBtn.closest('.summary-box');
            if (!box) return;
            const summaryEl = box.querySelector('.rp-summary-content');
            if (!summaryEl) return;
            const title = (box.querySelector('[data-summary-title]')?.textContent || '').trim();
            const link = (box.querySelector('[data-summary-link]')?.getAttribute('href') || '').trim();
            const summary = summaryEl.textContent.trim();
            const parts = [];
            if (title) parts.push(title);
            if (link) parts.push(link);
            parts.push(summary);
            const text = parts.join('\n\n');
            try {
                await navigator.clipboard.writeText(text);
                // Not textContent, which would drop the icon span.
                const label = copyBtn.querySelector('.action-label') || copyBtn;
                const original = label.textContent;
                label.textContent = 'Copied!';
                setTimeout(() => { label.textContent = original; }, 2000);
            } catch {
                window.flash?.error('Failed to copy to clipboard');
            }
            return;
        }
        const dismissBtn = e.target.closest('[data-summary-dismiss]');
        if (!dismissBtn) return;
        const entryId = dismissBtn.getAttribute('data-entry-id');
        if (!entryId) return;
        dismissBtn.disabled = true;
        try {
            const r = await fetch(`/api/entries/${entryId}/summary`, {
                method: 'DELETE',
                credentials: 'same-origin',
            });
            if (!r.ok) throw new Error('delete failed');
            // Skip if the reader switched entries meanwhile (as in performSwap()).
            if (String(currentPaneEntryId()) === String(entryId)) {
                // Keep the wrapper: it is the target for a later summarize.
                const container = document.querySelector('[data-summary-container]');
                if (container) container.replaceChildren();
                syncSummarizeToggleLabel();
            }
            const row = document.querySelector(
                `[data-entry-row][data-entry-id="${entryId}"]`
            );
            row?.querySelector(
                '.summary-badge, .summary-badge-pending, .summary-badge-processing, .summary-badge-failed'
            )?.remove();
        } catch {
            window.flash?.error('Failed to dismiss summary');
            dismissBtn.disabled = false;
        }
    });
}
