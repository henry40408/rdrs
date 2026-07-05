//! Plain-text extraction, snippet building, and match highlighting for the
//! SSR search results page. Pure string functions — no request state.

use crate::utils::text::strip_to_plain_text;

/// Build a query-aware snippet: returns a `max_chars`-wide window centered on
/// the first case-insensitive match of `query` in the plain-text content, with
/// `…` prefix/suffix where the window doesn't reach the original boundaries.
/// Falls back to the leading `max_chars` characters if no match is found
/// (or if `query` is empty).
pub fn build_snippet(html: Option<&str>, query: &str, max_chars: usize) -> String {
    let raw = match html {
        Some(s) if !s.is_empty() => s,
        _ => return String::new(),
    };
    let plain = strip_to_plain_text(raw);
    let total_chars = plain.chars().count();
    if total_chars <= max_chars {
        return plain;
    }

    // Try to center on the first match (ASCII-case-insensitive).
    let q = query.trim();
    if !q.is_empty() {
        let plain_lower = plain.to_ascii_lowercase();
        let q_lower = q.to_ascii_lowercase();
        if let Some(byte_pos) = plain_lower.find(&q_lower) {
            // Convert byte position → char index.
            let match_char_idx = plain[..byte_pos].chars().count();
            let context_before = max_chars / 3;
            let start_char = match_char_idx.saturating_sub(context_before);
            let end_char = (start_char + max_chars).min(total_chars);
            // Recompute start to fill the window if we hit the tail.
            let start_char = end_char.saturating_sub(max_chars);

            let window: String = plain
                .chars()
                .skip(start_char)
                .take(end_char - start_char)
                .collect();
            let prefix = if start_char > 0 { "…" } else { "" };
            let suffix = if end_char < total_chars { "…" } else { "" };
            return format!("{}{}{}", prefix, window.trim(), suffix);
        }
    }

    // Fallback: leading window.
    let truncated: String = plain.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

/// Wrap case-insensitive (ASCII-only — matches the `SQLite` LIKE COLLATE NOCASE
/// behavior of the search query) matches of `query` in `<mark>` tags. Returns
/// HTML with the non-match parts and the matched text both escaped, plus the
/// `<mark>...</mark>` wrappers around hits. Use with `|safe` in templates.
pub fn highlight_html(text: &str, query: &str) -> String {
    if query.is_empty() {
        return html_escape_minimal(text);
    }
    let q_lower = query.to_ascii_lowercase();
    let q_bytes = q_lower.len();
    if q_bytes == 0 {
        return html_escape_minimal(text);
    }
    let t_lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len() + 16);
    let mut last = 0;
    let mut start = 0;
    while start <= t_lower.len() {
        match t_lower[start..].find(&q_lower) {
            Some(rel) => {
                let abs = start + rel;
                out.push_str(&html_escape_minimal(&text[last..abs]));
                out.push_str("<mark>");
                out.push_str(&html_escape_minimal(&text[abs..abs + q_bytes]));
                out.push_str("</mark>");
                last = abs + q_bytes;
                start = last;
            }
            None => break,
        }
    }
    out.push_str(&html_escape_minimal(&text[last..]));
    out
}

fn html_escape_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
