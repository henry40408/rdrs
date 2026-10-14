//! Plain-text extraction from HTML: strips tags (including `<script>`/`<style>`
//! bodies and comments) and collapses whitespace.

/// ASCII case-insensitive prefix test on bytes; no allocation.
fn starts_with_ci(hay: &[u8], needle: &str) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

/// ASCII case-insensitive substring search on bytes; no allocation.
/// `needle` must be non-empty (checked by `debug_assert!`; an empty needle
/// would panic on indexing) and start with an ASCII byte.
fn find_ci(hay: &[u8], needle: &str) -> Option<usize> {
    debug_assert!(!needle.is_empty(), "find_ci: needle must be non-empty");
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

    /// Best-of-5 wall time of `strip_to_search_text` on `unit` repeated `reps`
    /// times; asserts the output length to keep the call from being elided.
    fn best_time(unit: &str, reps: usize, expected_len: usize) -> std::time::Duration {
        let html = unit.repeat(reps);
        (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                let out = strip_to_search_text(&html);
                let elapsed = start.elapsed();
                assert_eq!(out.len(), expected_len);
                elapsed
            })
            .min()
            .unwrap()
    }

    /// Scaling check instead of an absolute deadline: an 8x larger input must
    /// take well under the 64x an O(n^2) implementation would need (linear
    /// is ~8x; the 24x bound leaves headroom for CI jitter).
    fn assert_linear(unit: &str, out_per_unit: usize) {
        let small = 10_000;
        let t1 = best_time(unit, small, small * out_per_unit)
            // Floor avoids a near-zero denominator inflating the ratio.
            .max(std::time::Duration::from_micros(200));
        let t8 = best_time(unit, small * 8, small * 8 * out_per_unit);
        let ratio = t8.as_secs_f64() / t1.as_secs_f64();
        assert!(
            ratio < 24.0,
            "8x input took {ratio:.1}x longer ({t1:?} -> {t8:?}); expected ~8x"
        );
    }

    #[test]
    fn many_tags_linear_time() {
        assert_linear("<b>x</b>", 1);
    }

    #[test]
    fn many_script_blocks_linear_time() {
        assert_linear("<script>x</script>y", 1);
    }
}
