// Bulk mark-as-read: the "Mark as Read..." age dropdown, the "mark loaded entries
// above" button, and the `A` shortcut's worker. `performSwap` is passed in so
// this module never imports app.js.

// "Mark as Read..." posts to the GReader bulk-mark endpoint (optional `ts=`) and
// swaps the list in place; a form POST would land on a JSON response.
const AGE_LABELS = {
    '1': 'older than 1 day',
    '7': 'older than 1 week',
    '30': 'older than 1 month',
    '365': 'older than 1 year',
    'all': 'all',
};
const READING_LIST_STREAM = 'user/-/state/com.google/reading-list';

// Row count from `X-RDRS-Affected`, or null so callers use their own estimate.
function affectedCount(resp) {
    const raw = resp.headers.get('X-RDRS-Affected');
    if (raw === null) return null;
    const n = Number.parseInt(raw, 10);
    return Number.isNaN(n) ? null : n;
}

// Rebound after swaps (the header may be replaced); a guard prevents duplicates.
export function installMarkAsReadDropdown({ performSwap }) {
    const select = document.getElementById('mark-read-age');
    if (!select || select.dataset.markReadBound) return;
    select.dataset.markReadBound = '1';
    select.addEventListener('change', async () => {
        const age = select.value;
        select.selectedIndex = 0;
        if (!age) return;
        const ageLabel = AGE_LABELS[age] || age;
        if (!confirm(`Mark ${ageLabel} entries as read?`)) return;
        // GReader stream ID for the current page, e.g. `feed/<url>`.
        const scope = select.dataset.markReadScope || READING_LIST_STREAM;
        const body = new URLSearchParams();
        body.set('s', scope);
        if (age !== 'all') {
            const days = parseInt(age, 10);
            const tsUsec = (Math.floor(Date.now() / 1000) - days * 86400) * 1000000;
            body.set('ts', tsUsec.toString());
        }
        select.disabled = true;
        select.setAttribute('aria-busy', 'true');
        try {
            const resp = await fetch('/reader/api/0/mark-all-as-read', {
                method: 'POST',
                body,
                credentials: 'same-origin',
            });
            if (!resp.ok) throw new Error('Failed to mark as read');
            const n = affectedCount(resp);
            const scopeSuffix = age === 'all' ? '' : ` ${ageLabel}`;
            const message = n === null
                ? `Marked${scopeSuffix || ' all'} entries as read.`
                : `Marked ${n} ${n === 1 ? 'entry' : 'entries'}${scopeSuffix} as read.`;
            await finishBulkMarkRead(message, { performSwap });
        } catch (err) {
            const message = err.message || 'Failed to mark as read';
            if (window.flash) {
                window.flash.error(message);
            } else {
                alert(message);
            }
        } finally {
            select.disabled = false;
            select.removeAttribute('aria-busy');
        }
    });
}

/// Re-render the current list in place from `?fragment=1` (page 1). Resolves
/// `false` if there is nothing to swap, so callers can reload instead.
async function refreshEntriesList({ performSwap }) {
    if (!document.querySelector('[data-entries-list]')) return false;
    const url = new URL(window.location.href);
    url.searchParams.set('fragment', '1');
    // Drop `after` (append fragment) and `entry` (pane is left alone).
    url.searchParams.delete('after');
    url.searchParams.delete('entry');
    const applied = await performSwap(url.toString(), { method: 'GET' }, '[data-entries-list]',
        { fallbackUrl: window.location.href }); // `?fragment=1` is not a page.
    if (applied) scrollEntriesListToTop();
    return applied;
}

/// After a bulk mark-as-read: re-render the list and show `message`, or pass it
/// to the next document.
async function finishBulkMarkRead(message, { performSwap }) {
    const refreshed = await refreshEntriesList({ performSwap });
    if (!refreshed) {
        // No swap possible: pass the message via the cookie.
        window.flash?.set('success', message);
        window.location.reload();
        return;
    }
    // Shown rather than `set()`: the page it belongs to is still up.
    window.flash?.success(message);
    document.dispatchEvent(new CustomEvent('rdrs:sidebar-stale'));
}

/// Scroll the list back to the top after a bulk mark-as-read (both the desktop
/// list scroller and the mobile document).
function scrollEntriesListToTop() {
    document.querySelector('[data-entries-list]')?.scrollTo({ top: 0 });
    window.scrollTo({ top: 0 });
}

// Mark every rendered entry read via GReader edit-tag. Separate from the button
// so `A` still works when scoped search hides it; `btn` only carries busy state.
export async function markLoadedEntriesAsRead(btn, { performSwap }) {
    const rows = Array.from(document.querySelectorAll('[data-entry-row]'));
    const ids = rows.map(r => r.dataset.entryId).filter(Boolean);
    if (ids.length === 0) {
        const msg = 'No entries to mark.';
        if (window.flash) { window.flash.info(msg); } else { alert(msg); }
        return;
    }
    if (!confirm(`Mark ${ids.length} loaded entries as read?`)) return;
    const body = new URLSearchParams();
    for (const id of ids) body.append('i', id);
    body.set('a', 'user/-/state/com.google/read');
    if (btn) {
        btn.disabled = true;
        btn.setAttribute('aria-busy', 'true');
    }
    try {
        const resp = await fetch('/reader/api/0/edit-tag', {
            method: 'POST',
            body,
            credentials: 'same-origin',
        });
        if (!resp.ok) throw new Error('Failed to mark entries as read');
        // Excludes already-read rows, so usually smaller than posted.
        const n = affectedCount(resp) ?? ids.length;
        const message = `Marked ${n} ${n === 1 ? 'entry' : 'entries'} as read.`;
        await finishBulkMarkRead(message, { performSwap });
    } catch (err) {
        const message = err.message || 'Failed to mark entries as read';
        if (window.flash) { window.flash.error(message); } else { alert(message); }
    } finally {
        if (btn) {
            btn.disabled = false;
            btn.removeAttribute('aria-busy');
        }
    }
}

export function installMarkAboveButton({ performSwap }) {
    const btn = document.getElementById('mark-above-read');
    if (!btn || btn.dataset.markAboveBound) return;
    // Inside the swapped container; the guard prevents duplicate listeners.
    btn.dataset.markAboveBound = '1';
    btn.addEventListener('click', () => markLoadedEntriesAsRead(btn, { performSwap }));
}
