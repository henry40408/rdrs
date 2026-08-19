//! Han-script variant detection, used only to choose a `lang` attribute.
//!
//! The UI font stacks (`static/css/app.css`) name no CJK family — the webfonts
//! are latin subsets and `-apple-system` has no Han coverage — so every Chinese
//! character is placed by the browser's own fallback. With no language hint on
//! the text the cascade follows the *reader's* locale, and a Traditional
//! cascade lands on `PingFang TC`, which is missing 487 of the CJK codepoints
//! `PingFang SC` carries. Common simplified forms are among them (杀 远 迟 稳 杂
//! 迈 敌 艳 鉴 虑), so those characters — and only those — drop one step further
//! to `PingFang SC` and render in a visibly different face from their neighbours.
//!
//! Tagging a Simplified string `lang="zh-Hans"` puts the whole string on the
//! Simplified cascade, which has no such gap. Nothing here needs to be right
//! about *language*; it only needs to stop one line being resolved in two
//! fonts.
//!
//! A character counts as Simplified-only when GBK can encode it and Big5
//! cannot. `encoding_rs` is already linked into the binary (`lol_html`,
//! `quick-xml`, `reqwest`), so this adds no dependency weight and no character
//! table to keep up to date.

use encoding_rs::{BIG5, Encoding, GBK};
use std::sync::LazyLock;

/// Inclusive codepoint range covered by the lookup: CJK Unified Ideographs
/// Extension A through the main block. Neither GBK nor Big5 maps anything
/// outside it, so a character beyond this range can never satisfy the test.
const FIRST: u32 = 0x3400;
const LAST: u32 = 0x9FFF;
const WORDS: usize = ((LAST - FIRST) as usize + 1).div_ceil(64);

/// How many Simplified-only characters a string needs before it is tagged.
///
/// One is too eager. Traditional publications that spell 為 as 爲 and 群 as
/// 羣, and Traditional titles carrying a Japanese name (瀬 咲 沢), each trip a
/// single match without being Simplified — and `PingFang TC` covers all of those
/// anyway, so tagging them would restyle a line that was rendering fine.
/// Requiring two drops the misfire rate on Traditional titles from 1.19% to
/// 0.12% while still catching the strings that actually hit the gap.
const THRESHOLD: usize = 2;

/// Bit set over `FIRST..=LAST`; a set bit means "GBK covers it, Big5 does not".
///
/// Built by decoding every two-byte sequence each encoding accepts rather than
/// by asking the encoder about all 27,648 codepoints one at a time: the
/// encoders search their index linearly, which costs 60 ms in a release build
/// and 4.7 s in a debug one, where the sweep costs 4 ms and 100 ms. The two
/// agree exactly across this range — `sweep_matches_the_encoders` pins that.
static SIMPLIFIED_ONLY: LazyLock<[u64; WORDS]> = LazyLock::new(|| {
    let mut bits = [0u64; WORDS];
    // GBK first, then Big5 clears what both share, leaving GBK-only.
    sweep(GBK, GBK_FIRST_LEAD, |cp| set(&mut bits, cp, true));
    sweep(BIG5, BIG5_FIRST_LEAD, |cp| set(&mut bits, cp, false));
    bits
});

/// Lead byte each encoding's two-byte sequences start at.
///
/// Big5 starts at 0xA1, not 0x81, on purpose. The WHATWG Big5 decoder accepts
/// leads from 0x81 — the HKSCS extension — but its *encoder* refuses every
/// pointer below `(0xA1 - 0x81) * 157`, so those characters decode and never
/// encode. Starting the sweep at 0xA1 reproduces the encoder's repertoire,
/// which is the one this module means by "Big5 has it".
const GBK_FIRST_LEAD: u8 = 0x81;
const BIG5_FIRST_LEAD: u8 = 0xA1;

/// Call `visit` with the codepoint of every two-byte sequence `encoding`
/// decodes to exactly one character.
fn sweep(encoding: &'static Encoding, first_lead: u8, mut visit: impl FnMut(u32)) {
    for lead in first_lead..=0xFE {
        for trail in 0x40..=0xFE {
            let bytes = [lead, trail];
            let (text, malformed) = encoding.decode_without_bom_handling(&bytes);
            if malformed {
                continue;
            }
            // A handful of Big5 sequences decode to a pair of codepoints;
            // those are not single characters and cannot be what we tag on.
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
///
/// Safe to hand raw HTML: tags and attribute names are ASCII and cannot match.
/// The scan stops at the threshold, so a Simplified body is decided within its
/// first few characters rather than in a full pass.
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

    /// The sweep is an optimisation over asking the encoders directly, and it
    /// leans on the Big5 encoder's pointer floor lining up with lead byte
    /// 0xA1. Should `encoding_rs` ever move either repertoire, the two would
    /// drift apart silently — so check them against each other over the whole
    /// range the lookup answers for.
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

    /// The characters that actually trip the `PingFang TC` gap, as measured
    /// across the entry corpus. Every one of them must be recognised, or the
    /// tag never lands on the strings that need it.
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

    /// Traditional copy that spells 為 as 爲 and 群 as 羣 trips one match per
    /// occurrence. `PingFang TC` has both, so a single hit must not tag the line.
    #[test]
    fn one_variant_form_is_not_enough() {
        assert_eq!(lang_attr("人死後大腦爲何能保存上萬年"), None);
        assert_eq!(lang_attr("哈薩克斯坦重建老虎種羣"), None);
    }

    /// A Japanese name inside a Traditional title is the other single-match
    /// case; `PingFang TC` covers these too.
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

    /// Markup must not change the verdict — the reading pane hands this
    /// function stored HTML, not plain text.
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
