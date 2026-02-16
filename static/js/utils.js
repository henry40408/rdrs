// Shared utility functions for RDRS components

/**
 * Escape HTML special characters to prevent XSS.
 * @param {string} text
 * @returns {string}
 */
export function escapeHtml(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

/**
 * Decode HTML entities (e.g., &#x27; -> ').
 * @param {string} html
 * @returns {string}
 */
export function decodeHtml(html) {
    if (!html) return '';
    const textarea = document.createElement('textarea');
    textarea.innerHTML = html;
    return textarea.value;
}

/**
 * Format an ISO date string as a locale date (short form).
 * @param {string} isoString
 * @returns {string}
 */
export function formatDate(isoString) {
    const date = new Date(isoString);
    return date.toLocaleDateString();
}

/**
 * Format an ISO date string as a full locale date+time.
 * @param {string} isoString
 * @returns {string}
 */
export function formatDateTime(isoString) {
    const date = new Date(isoString);
    return date.toLocaleString();
}

/**
 * Highlight search terms within text (returns HTML string).
 * @param {string} text - Raw text (will be escaped)
 * @param {string} search - Search term to highlight
 * @returns {string} HTML string with highlights
 */
export function highlightText(text, search) {
    if (!search || !text) return escapeHtml(text);
    const escaped = escapeHtml(text);
    const searchEscaped = escapeHtml(search);
    const regex = new RegExp(`(${searchEscaped.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'gi');
    return escaped.replace(regex, '<span class="search-highlight">$1</span>');
}

/**
 * Extract a content snippet around a search term.
 * @param {string} content - HTML content
 * @param {string} search - Search term
 * @param {number} maxLen - Max snippet length
 * @returns {string|null}
 */
export function getContentSnippet(content, search, maxLen = 80) {
    if (!content || !search) return null;
    const text = content.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim();
    const lowerText = text.toLowerCase();
    const lowerSearch = search.toLowerCase();
    const idx = lowerText.indexOf(lowerSearch);
    if (idx === -1) return null;

    const contextBefore = 30;
    const start = Math.max(0, idx - contextBefore);
    const end = Math.min(text.length, idx + search.length + (maxLen - contextBefore));
    let snippet = text.substring(start, end);
    if (start > 0) snippet = '...' + snippet;
    if (end < text.length) snippet = snippet + '...';
    return snippet;
}
