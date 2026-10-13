//! Plain-text extraction from HTML: strips tags (including `<script>`/`<style>`
//! bodies and comments) and collapses whitespace.

/// ASCII case-insensitive prefix test on bytes; no allocation.
fn starts_with_ci(hay: &[u8], needle: &str) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

/// ASCII case-insensitive substring search on bytes; no allocation.
/// `needle` must be non-empty and start with an ASCII byte.
fn find_ci(hay: &[u8], needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    let first = n[0];
    let last_start = hay.len().checked_sub(n.len())?;
    (0..=last_start).find(|&p| {
        hay[p].eq_ignore_ascii_case(&first) && hay[p..p + n.len()].eq_ignore_ascii_case(n)
    })
}

fn strip_impl(raw: &str, tag_gap: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut last_space = true;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end_tag) = skip_until {
            if let Some(pos) = find_ci(&bytes[i..], end_tag) {
                i += pos + end_tag.len();
                skip_until = None;
                in_tag = false;
                if tag_gap && !last_space {
                    out.push(' ');
                    last_space = true;
                }
                continue;
            }
            break;
        }
        let ch = bytes[i] as char;
        match ch {
            '<' => {
                let rest = &bytes[i..];
                if starts_with_ci(rest, "<script") {
                    skip_until = Some("</script>");
                    i += 1;
                    continue;
                }
                if starts_with_ci(rest, "<style") {
                    skip_until = Some("</style>");
                    i += 1;
                    continue;
                }
                if starts_with_ci(rest, "<!--") {
                    if let Some(pos) = raw[i + 4..].find("-->") {
                        i += 4 + pos + 3;
                        if tag_gap && !last_space {
                            out.push(' ');
                            last_space = true;
                        }
                        continue;
                    }
                    break;
                }
                in_tag = true;
                i += 1;
            }
            '>' if in_tag => {
                in_tag = false;
                if tag_gap && !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ if in_tag => {
                i += 1;
            }
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ => {
                let ch_len = raw[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&raw[i..i + ch_len]);
                last_space = false;
                i += ch_len;
            }
        }
    }
    out.trim().to_string()
}

/// Strip HTML to plain text, inserting a space at tag boundaries (readable
/// for display snippets).
pub fn strip_to_plain_text(raw: &str) -> String {
    strip_impl(raw, true)
}

/// Like [`strip_to_plain_text`] with no separator at tag boundaries, so
/// `超<b>少女</b>` stays contiguous. Builds the searchable `entry.content_text`.
pub fn strip_to_search_text(raw: &str) -> String {
    strip_impl(raw, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_keeps_text_across_tags() {
        // A term split across inline tags must be contiguous in plain text.
        assert_eq!(
            strip_to_plain_text("超<b>少女</b>與機器人"),
            "超 少女 與機器人"
        );
    }

    #[test]
    fn drops_script_and_attribute_text() {
        assert_eq!(
            strip_to_plain_text(
                r#"<a href="https://x/超少女">hi</a><script>var superheroine=1</script>"#
            ),
            "hi",
        );
    }

    #[test]
    fn search_text_joins_across_tags() {
        assert_eq!(
            strip_to_search_text("超<b>少女</b>與機器人"),
            "超少女與機器人"
        );
    }

    #[test]
    fn golden_case_insensitive_blocks_and_comments() {
        let html =
            "a<SCRIPT type=x>var a='</b>';</ScRiPt>b<Style>p{}</STYLE>c<!-- <b>x</b> -->d<B>e</B>f";
        assert_eq!(strip_to_plain_text(html), "a b c d e f");
        assert_eq!(strip_to_search_text(html), "abcdef");
    }

    #[test]
    fn golden_unterminated_blocks_drop_rest() {
        assert_eq!(strip_to_plain_text("a<script>b<b>c"), "a");
        assert_eq!(strip_to_plain_text("a<style>b"), "a");
        assert_eq!(strip_to_plain_text("a<!-- b"), "a");
        assert_eq!(strip_to_plain_text("a<scrip>b</scrip>"), "a b");
    }

    #[test]
    fn many_tags_linear_time() {
        let html = "<b>x</b>".repeat(80_000);
        let start = std::time::Instant::now();
        let out = strip_to_search_text(&html);
        assert_eq!(out.len(), 80_000);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(500),
            "strip took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn many_script_blocks_linear_time() {
        let html = "<script>x</script>y".repeat(40_000);
        let start = std::time::Instant::now();
        let out = strip_to_search_text(&html);
        assert_eq!(out.len(), 40_000);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(500),
            "strip took {:?}",
            start.elapsed()
        );
    }
}
