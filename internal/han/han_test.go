package han

import (
	"encoding/json"
	"os"
	"testing"

	"golang.org/x/text/encoding/simplifiedchinese"
	"golang.org/x/text/encoding/traditionalchinese"
)

// The sweep is only valid while it agrees with the encoders over the whole
// range, as in the Rust build.
func TestSweepMatchesTheEncoders(t *testing.T) {
	gbk := simplifiedchinese.GBK.NewEncoder()
	big5 := traditionalchinese.Big5.NewEncoder()
	bits := simplifiedOnly()
	for cp := rune(first); cp <= last; cp++ {
		text := string(cp)
		_, gbkErr := gbk.String(text)
		big5Bytes, big5Err := big5.String(text)
		// Unlike encoding_rs, the x/text Big5 encoder emits HKSCS leads below
		// 0xA1; the sweep deliberately ignores them.
		big5Has := big5Err == nil && big5Bytes[0] >= big5FirstLead
		want := gbkErr == nil && !big5Has
		if bits.has(cp) != want {
			t.Errorf("U+%04X %c: sweep says %v, encoders say %v", cp, cp, bits.has(cp), want)
		}
	}
}

func TestFontGapCharactersAreSimplified(t *testing.T) {
	bits := simplifiedOnly()
	for _, ch := range "杀远虑稳迟杂迈敌艳鉴" {
		if !bits.has(ch) {
			t.Errorf("%c should count as Simplified", ch)
		}
	}
}

func TestLangAttr(t *testing.T) {
	cases := []struct {
		name, text, want string
	}{
		{"simplified", "《控制》新作体验：新怪谈游戏的祖师爷，又杀回来了", "zh-Hans"},
		{"traditional", "《控制》新作體驗：新怪談遊戲的祖師爺，又殺回來了", ""},
		{"variant form 爲", "人死後大腦爲何能保存上萬年", ""},
		{"variant form 羣", "哈薩克斯坦重建老虎種羣", ""},
		{"japanese name", "阿賀沢紅茶《冰之城牆》動畫第二季10月登場", ""},
		{"empty", "", ""},
		{"ascii", "Rust 1.90 released", ""},
		{"kana", "日本語のテキスト", ""},
		{"markup simplified", `<p class="x"><a href="/a">远方</a>的敌人</p>`, "zh-Hans"},
		{"markup traditional", `<p class="simplified"><a href="/traditional">遠方</a></p>`, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := LangAttr(tc.text); got != tc.want {
				t.Errorf("LangAttr(%q) = %q, want %q", tc.text, got, tc.want)
			}
		})
	}
}

// Rust vectors; the Big5 repertoire of x/text may differ from encoding_rs, so
// any mismatch here must be reported rather than silenced.
func TestLangAttrMatchesRustVectors(t *testing.T) {
	raw, err := os.ReadFile("../../testdata/vectors/text.json")
	if err != nil {
		t.Fatal(err)
	}
	var vectors struct {
		Han []struct {
			Input string  `json:"input"`
			Lang  *string `json:"lang"`
		} `json:"han"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	if len(vectors.Han) == 0 {
		t.Fatal("no han vectors")
	}
	for _, v := range vectors.Han {
		want := ""
		if v.Lang != nil {
			want = *v.Lang
		}
		if got := LangAttr(v.Input); got != want {
			t.Errorf("LangAttr(%q) = %q, want %q", v.Input, got, want)
		}
	}
}
