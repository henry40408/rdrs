//! Han-script variant detection, used only to choose a `lang` attribute.
//!
//! With no CJK family in our font stacks, a Traditional-locale fallback lands
//! on `PingFang TC`, which lacks many simplified forms, so those characters
//! render in a different face. Tagging Simplified text `lang="zh-Hans"` avoids
//! that. Simplified-only means GBK encodes it and Big5 does not (`encoding_rs`
//! is already linked).

use encoding_rs::{BIG5, Encoding, GBK};
use std::sync::LazyLock;

/// Codepoint range covered (CJK Ext A through the main block); neither GBK
/// nor Big5 maps anything outside it.
const FIRST: u32 = 0x3400;
const LAST: u32 = 0x9FFF;
const WORDS: usize = ((LAST - FIRST) as usize + 1).div_ceil(64);

/// Simplified-only characters needed before tagging. One misfires on
/// Traditional variants (爲, 羣) and Japanese names that `PingFang TC` covers.
const THRESHOLD: usize = 2;

/// Bit set over `FIRST..=LAST`: set means GBK-only. Built by sweeping decoders,
/// since the encoders search linearly (far slower, esp. in debug);
/// `sweep_matches_the_encoders` pins that both agree.
static SIMPLIFIED_ONLY: LazyLock<[u64; WORDS]> = LazyLock::new(|| {
    let mut bits = [0u64; WORDS];
    // GBK first, then Big5 clears what both share, leaving GBK-only.
    sweep(GBK, GBK_FIRST_LEAD, |cp| set(&mut bits, cp, true));
    sweep(BIG5, BIG5_FIRST_LEAD, |cp| set(&mut bits, cp, false));
    bits
});

/// Lead byte each encoding's two-byte sequences start at. Big5 starts at 0xA1:
/// the decoder accepts HKSCS leads from 0x81 but the encoder refuses them, and
/// "Big5 has it" means the encoder's repertoire.
const GBK_FIRST_LEAD: u8 = 0x81;
const BIG5_FIRST_LEAD: u8 = 0xA1;

/// Call `visit` for every two-byte sequence `encoding` decodes to one char.
fn sweep(encoding: &'static Encoding, first_lead: u8, mut visit: impl FnMut(u32)) {
    for lead in first_lead..=0xFE {
        for trail in 0x40..=0xFE {
            let bytes = [lead, trail];
            let (text, malformed) = encoding.decode_without_bom_handling(&bytes);
            if malformed {
                continue;
            }
            // Some Big5 sequences decode to two codepoints; skip them.
            let mut chars = text.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                visit(ch as u32);
            }
        }
    }
}

fn set(bits: &mut [u64; WORDS], cp: u32, present: bool) {
    if !(FIRST..=LAST).contains(&cp) {
        return;
    }
    let index = (cp - FIRST) as usize;
    if present {
        bits[index / 64] |= 1 << (index % 64);
    } else {
        bits[index / 64] &= !(1 << (index % 64));
    }
}

fn is_simplified_only(ch: char) -> bool {
    let cp = ch as u32;
    if !(FIRST..=LAST).contains(&cp) {
        return false;
    }
    let index = (cp - FIRST) as usize;
    SIMPLIFIED_ONLY[index / 64] & (1 << (index % 64)) != 0
}

/// `Some("zh-Hans")` when `text` reads as Simplified Chinese, else `None`.
/// Safe on raw HTML (ASCII cannot match); stops at the threshold.
pub fn lang_attr(text: &str) -> Option<&'static str> {
    let mut seen = 0usize;
    for ch in text.chars() {
        if is_simplified_only(ch) {
            seen += 1;
            if seen >= THRESHOLD {
                return Some("zh-Hans");
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the sweep against `encoding_rs` repertoire drift over the whole
    /// range.
    #[test]
    fn sweep_matches_the_encoders() {
        fn encodable(encoding: &'static Encoding, text: &str) -> bool {
            let (_bytes, _actual, had_unmappable) = encoding.encode(text);
            !had_unmappable
        }

        let mut buf = [0u8; 4];
        for cp in FIRST..=LAST {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let encoded = ch.encode_utf8(&mut buf);
            let expected = encodable(GBK, encoded) && !encodable(BIG5, encoded);
            assert_eq!(
                is_simplified_only(ch),
                expected,
                "U+{cp:04X} {ch}: sweep and encoder disagree"
            );
        }
    }

    /// Characters that actually hit the `PingFang TC` gap in the corpus.
    #[test]
    fn recognises_the_characters_that_hit_the_font_gap() {
        for ch in "杀远虑稳迟杂迈敌艳鉴".chars() {
            assert!(is_simplified_only(ch), "{ch} should count as Simplified");
        }
    }

    #[test]
    fn tags_simplified_text() {
        assert_eq!(
            lang_attr("《控制》新作体验：新怪谈游戏的祖师爷，又杀回来了"),
            Some("zh-Hans")
        );
    }

    #[test]
    fn leaves_traditional_text_alone() {
        assert_eq!(
            lang_attr("《控制》新作體驗：新怪談遊戲的祖師爺，又殺回來了"),
            None
        );
    }

    /// Traditional variants 爲/羣: one hit must not tag the line.
    #[test]
    fn one_variant_form_is_not_enough() {
        assert_eq!(lang_attr("人死後大腦爲何能保存上萬年"), None);
        assert_eq!(lang_attr("哈薩克斯坦重建老虎種羣"), None);
    }

    /// A Japanese name in a Traditional title: also a single-match case.
    #[test]
    fn a_japanese_name_is_not_enough() {
        assert_eq!(lang_attr("阿賀沢紅茶《冰之城牆》動畫第二季10月登場"), None);
    }

    #[test]
    fn ignores_text_without_han() {
        assert_eq!(lang_attr(""), None);
        assert_eq!(lang_attr("Rust 1.90 released"), None);
        assert_eq!(lang_attr("日本語のテキスト"), None);
    }

    /// Markup must not change the verdict; callers pass stored HTML.
    #[test]
    fn scans_through_markup() {
        assert_eq!(
            lang_attr(r#"<p class="x"><a href="/a">远方</a>的敌人</p>"#),
            Some("zh-Hans")
        );
        assert_eq!(
            lang_attr(r#"<p class="simplified"><a href="/traditional">遠方</a></p>"#),
            None
        );
    }
}
