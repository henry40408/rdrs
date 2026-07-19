//! Plain-text extraction from HTML. Pure string function — no request state.
//! Strips tags (including `<script>`/`<style>` bodies and comments) and
//! collapses whitespace to single spaces.

fn strip_impl(raw: &str, tag_gap: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut last_space = true;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end_tag) = skip_until {
            if let Some(pos) = raw[i..].to_ascii_lowercase().find(end_tag) {
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
                let lower = raw[i..].to_ascii_lowercase();
                if lower.starts_with("<script") {
                    skip_until = Some("</script>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<style") {
                    skip_until = Some("</style>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<!--") {
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

/// Like [`strip_to_plain_text`] but inserts **no** separator at tag
/// boundaries, so a term split across inline tags (`超<b>少女</b>`) stays
/// contiguous. Used to build the searchable `entry.content_text`.
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
        // No separator at tag boundaries, so a term split by inline markup
        // stays contiguous and matchable by LIKE.
        assert_eq!(
            strip_to_search_text("超<b>少女</b>與機器人"),
            "超少女與機器人"
        );
    }
}
