//! Minimal HTML entity decoder for plain-text fields (titles, authors, OPML
//! labels), undoing the residual layer of double-encoded feeds (`&amp;#x27;`)
//! that Askama would otherwise re-escape and display literally.
//!
//! Unknown or malformed sequences are left verbatim. MUST NOT be applied to
//! HTML fields that go through `sanitize_html`.

/// Decode `&amp; &lt; &gt; &quot; &apos; &#39;` and decimal/hex references;
/// anything else is preserved unchanged.
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
            // Copy the whole UTF-8 char.
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

        // Not an entity: keep the '&' literal.
        out.push('&');
        i += 1;
    }
    out
}

/// Decode the body of one `&…;`, or `None` if unrecognized.
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
    fn decodes_numeric_and_named_references() {
        for (input, expected) in [
            ("Collabora&#x27;s CODE", "Collabora's CODE"),
            ("a&#X27;b", "a'b"),
            ("it&#39;s", "it's"),
            ("Tom &amp; Jerry", "Tom & Jerry"),
            ("&quot;hi&quot; &apos;yo&apos;", "\"hi\" 'yo'"),
            ("&lt;tag&gt;", "<tag>"),
            (
                "A &amp; B &#x27;C&#x27; &lt;d&gt; &#39;e&#39;",
                "A & B 'C' <d> 'e'",
            ),
            ("plain text, no entities", "plain text, no entities"),
            ("日本語 &amp; 中文", "日本語 & 中文"),
            ("ends with &", "ends with &"),
        ] {
            assert_eq!(decode_html_entities(input), expected, "{input}");
        }
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
}
