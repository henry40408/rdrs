//! Minimal HTML entity decoder for plain-text fields (entry titles, author
//! names, OPML labels). Some feeds double-encode entities (e.g. emit
//! `&amp;#x27;`); feed-rs / quick-xml only unescape one layer, leaving a
//! literal `&#x27;` in the stored value. When Askama then auto-escapes the
//! field for rendering, the `&` is escaped again and the reader sees the
//! literal `&#x27;` instead of `'`. Decoding the residual entity before
//! handing the string to the template fixes the display.
//!
//! Scope is deliberately narrow: named entities common in feed text, plus
//! decimal (`&#NN;`) and hexadecimal (`&#xNN;` / `&#XNN;`) numeric character
//! references. Unknown or malformed sequences are left verbatim — never a
//! panic. This MUST NOT be applied to HTML fields (content / summary) that go
//! through `sanitize_html`, only to plain-text fields.

/// Decode the named/numeric HTML entities we care about in plain text.
///
/// Recognizes `&amp; &lt; &gt; &quot; &apos; &#39;`, decimal references
/// `&#NN;`, and hex references `&#xNN;` / `&#XNN;`. Anything else (unknown
/// name, missing terminating `;`, non-digit body, out-of-range code point)
/// is preserved unchanged.
pub fn decode_html_entities(s: &str) -> String {
    // Fast path: no entity markers at all.
    if !s.contains('&') {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            // Copy the current UTF-8 char wholesale (handles multibyte).
            let ch_len = utf8_len(bytes[i]);
            out.push_str(&s[i..i + ch_len]);
            i += ch_len;
            continue;
        }

        // Find the terminating ';' within a small window.
        if let Some(semi) = s[i + 1..]
            .char_indices()
            .take_while(|&(off, _)| off < 32)
            .find(|&(_, c)| c == ';')
            .map(|(off, _)| i + 1 + off)
        {
            let body = &s[i + 1..semi];
            if let Some(decoded) = decode_one(body) {
                out.push(decoded);
                i = semi + 1;
                continue;
            }
        }

        // Not a recognized entity — keep the '&' literal and move on.
        out.push('&');
        i += 1;
    }
    out
}

/// Decode the inside of a single `&…;` (the body excludes `&` and `;`).
/// Returns `None` when the body is not a recognized entity.
fn decode_one(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = body.strip_prefix('#')?;
            let code = if let Some(hex) = num.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                num.parse::<u32>().ok()?
            };
            char::from_u32(code)
        }
    }
}

/// Byte length of the UTF-8 char starting at the given lead byte.
fn utf8_len(lead: u8) -> usize {
    match lead {
        b if b < 0x80 => 1,
        b if b >> 5 == 0b110 => 2,
        b if b >> 4 == 0b1110 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reference_lowercase() {
        assert_eq!(
            decode_html_entities("Collabora&#x27;s CODE"),
            "Collabora's CODE"
        );
    }

    #[test]
    fn hex_reference_uppercase_marker() {
        assert_eq!(decode_html_entities("a&#X27;b"), "a'b");
    }

    #[test]
    fn decimal_reference() {
        assert_eq!(decode_html_entities("it&#39;s"), "it's");
    }

    #[test]
    fn named_amp() {
        assert_eq!(decode_html_entities("Tom &amp; Jerry"), "Tom & Jerry");
    }

    #[test]
    fn named_quot_and_apos() {
        assert_eq!(
            decode_html_entities("&quot;hi&quot; &apos;yo&apos;"),
            "\"hi\" 'yo'"
        );
    }

    #[test]
    fn named_lt_gt() {
        assert_eq!(decode_html_entities("&lt;tag&gt;"), "<tag>");
    }

    #[test]
    fn mixed() {
        assert_eq!(
            decode_html_entities("A &amp; B &#x27;C&#x27; &lt;d&gt; &#39;e&#39;"),
            "A & B 'C' <d> 'e'"
        );
    }

    #[test]
    fn no_entities() {
        assert_eq!(
            decode_html_entities("plain text, no entities"),
            "plain text, no entities"
        );
    }

    #[test]
    fn invalid_sequences_preserved() {
        // Unknown name, missing semicolon, non-numeric body, bad code point.
        assert_eq!(decode_html_entities("R&D"), "R&D");
        assert_eq!(decode_html_entities("a&unknown;b"), "a&unknown;b");
        assert_eq!(decode_html_entities("100&#nope;"), "100&#nope;");
        assert_eq!(decode_html_entities("a & b"), "a & b");
        // Out-of-range Unicode scalar -> char::from_u32 returns None.
        assert_eq!(decode_html_entities("&#xFFFFFFFF;"), "&#xFFFFFFFF;");
        // Surrogate code point is not a valid char.
        assert_eq!(decode_html_entities("&#xD800;"), "&#xD800;");
    }

    #[test]
    fn multibyte_passthrough() {
        assert_eq!(decode_html_entities("日本語 &amp; 中文"), "日本語 & 中文");
    }

    #[test]
    fn trailing_ampersand() {
        assert_eq!(decode_html_entities("ends with &"), "ends with &");
    }
}
