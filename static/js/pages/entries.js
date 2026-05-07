// <rdrs-entries-page> — CSR shell for the entries-family list pages.
// data-mode in {unread, all, read, starred, summarized, feed, category, search}
// is inferred from location.pathname on connect.

class RdrsEntriesPage extends HTMLElement {
    connectedCallback() {
        // TODO(Task 2): full implementation.
        this.innerHTML = '<p class="muted">Loading...</p>';
    }
}

customElements.define('rdrs-entries-page', RdrsEntriesPage);
