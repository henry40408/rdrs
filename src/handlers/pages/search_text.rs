//! Plain-text extraction, snippet building, and match highlighting for the
//! SSR search results page. Pure string functions — no request state.

use crate::utils::text::strip_to_plain_text;

/// Build a query-aware snippet: returns a `max_chars`-wide window centered on
/// the first case-insensitive match of any of `terms` in the plain-text content,
/// with `…` prefix/suffix where the window doesn't reach the original boundaries.
/// Falls back to the leading `max_chars` characters if no term matches
/// (or if `terms` is empty).
pub fn build_snippet(html: Option<&str>, terms: &[&str], max_chars: usize) -> String {
    let raw = match html {
        Some(s) if !s.is_empty() => s,
        _ => return String::new(),
    };
    let plain = strip_to_plain_text(raw);
    let total_chars = plain.chars().count();
    if total_chars <= max_chars {
        return plain;
    }

    // Try to center on the earliest match of any term (ASCII-case-insensitive).
    let plain_lower = plain.to_ascii_lowercase();
    let first_match = terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter_map(|t| plain_lower.find(&t.to_ascii_lowercase()))
        .min();
    if let Some(byte_pos) = first_match {
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

    // Fallback: leading window.
    let truncated: String = plain.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

/// Wrap case-insensitive (ASCII-only — matches the `SQLite` LIKE COLLATE NOCASE
/// behavior of the search query) matches of any of `terms` in `<mark>` tags.
/// Returns HTML with the non-match parts and the matched text both escaped, plus
/// the `<mark>...</mark>` wrappers around hits. Overlapping matches from
/// different terms are merged into a single wrapper. Use with `|safe` in
/// templates.
pub fn highlight_html(text: &str, terms: &[&str]) -> String {
    // `to_ascii_lowercase` preserves byte length, so offsets into `t_lower`
    // index `text` identically.
    let t_lower = text.to_ascii_lowercase();

    // Collect every match range (byte offsets) across all non-empty terms.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let needle = term.to_ascii_lowercase();
        if needle.is_empty() {
            continue;
        }
        let n = needle.len();
        let mut start = 0;
        while let Some(rel) = t_lower[start..].find(&needle) {
            let abs = start + rel;
            ranges.push((abs, abs + n));
            start = abs + n;
        }
    }
    if ranges.is_empty() {
        return html_escape_minimal(text);
    }

    // Sort by start and merge overlapping/adjacent ranges so nested or crossing
    // matches (e.g. "learn" inside "learning") produce one contiguous wrapper.
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }

    let mut out = String::with_capacity(text.len() + 16 * merged.len());
    let mut last = 0;
    for (s, e) in merged {
        out.push_str(&html_escape_minimal(&text[last..s]));
        out.push_str("<mark>");
        out.push_str(&html_escape_minimal(&text[s..e]));
        out.push_str("</mark>");
        last = e;
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
