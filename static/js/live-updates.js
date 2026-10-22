// Live updates over SSE: `sidebar` refetches /api/sidebar, `summary` updates the
// badge and pane. Each reconnect resyncs the sidebar.
const SUMMARY_ICON_FILLED =
    '<svg class="ico is-filled" viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></svg>';
const SUMMARY_ICON_OUTLINE =
    '<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><g transform="translate(1.2 1.2) scale(0.9)"><path d="M12 3L14 10L21 12L14 14L12 21L10 14L3 12L10 10Z"/></g></svg>';
// status -> [badge class, title, filled?]; null clears the badge.
const SUMMARY_BADGE = {
    completed:  ['summary-badge', 'Has Summary', true],
    pending:    ['summary-badge-pending', 'Pending', false],
    processing: ['summary-badge-processing', 'Processing', false],
    failed:     ['summary-badge-failed', 'Failed', true],
};
const BADGE_SELECTOR =
    '.summary-badge, .summary-badge-pending, .summary-badge-processing, .summary-badge-failed';

function renderSummaryBadge(row, status) {
    const existing = row.querySelector(BADGE_SELECTOR);
    if (!status || !SUMMARY_BADGE[status]) { existing?.remove(); return; }
    const [cls, title, filled] = SUMMARY_BADGE[status];
    const svg = filled ? SUMMARY_ICON_FILLED : SUMMARY_ICON_OUTLINE;
    if (existing) {
        existing.className = cls;
        existing.title = title;
        existing.innerHTML = svg;
        return;
    }
    // Before <time>, matching the SSR badge order.
    const span = document.createElement('span');
    span.className = cls;
    span.title = title;
    span.setAttribute('aria-hidden', 'true');
    span.innerHTML = svg;
    const statusCluster = row.querySelector('.entry-status');
    const time = statusCluster?.querySelector('.entry-time');
    if (statusCluster && time) statusCluster.insertBefore(span, time);
    else statusCluster?.appendChild(span);
}

// Announce that sidebar state may have changed; <rdrs-sidebar> refetches.
export function refreshSidebar() {
    document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
}

function onSummaryEvent(data, { currentPaneEntryId, performSwap }) {
    const { entry_id, status } = data;
    const row = document.querySelector(`[data-entry-row][data-entry-id="${entry_id}"]`);
    if (row) renderSummaryBadge(row, status);
    if (String(currentPaneEntryId()) === String(entry_id)) {
        performSwap(`/entries/${entry_id}/summary/fragment`, { method: 'GET' }, '#rp-summary-container');
    }
}

export function installSse({ currentPaneEntryId, performSwap }) {
    // Only on the logged-in surface (the sidebar element is the marker).
    if (!document.querySelector('rdrs-sidebar')) return;
    let es;
    try {
        es = new EventSource('/events', { withCredentials: true });
    } catch {
        return; // EventSource unavailable — no live updates, page still works.
    }
    // Skip the first `open`: the sidebar already fetched. Later ones are reconnects.
    let sseHasConnected = false;
    es.addEventListener('open', () => {
        if (sseHasConnected) refreshSidebar();
        sseHasConnected = true;
    });
    es.addEventListener('sidebar', () => refreshSidebar());
    es.addEventListener('summary', (e) => {
        try { onSummaryEvent(JSON.parse(e.data), { currentPaneEntryId, performSwap }); } catch {}
    });
    // EventSource auto-reconnects on transient errors; nothing to do here.
}
