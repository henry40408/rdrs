package zxcvbn

import (
	"bufio"
	"math/rand/v2"
	"os"
	"strconv"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/zxcvbn/scoring"
)

// corpusYear is when testdata/rust_scores.tsv was generated. Year and date
// guesses depend on the distance to the current year, so the comparison
// pins it.
const corpusYear = 2026

const minAcceptedScore = 3

type rustCase struct {
	line     int
	score    int
	username string
	password string
}

// loadRustCorpus reads "score\tusername\tpassword" lines scored by Rust
// zxcvbn 3.1.1.
func loadRustCorpus(t *testing.T) []rustCase {
	t.Helper()
	f, err := os.Open("testdata/rust_scores.tsv")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = f.Close() }()
	var cases []rustCase
	scanner := bufio.NewScanner(f)
	for n := 1; scanner.Scan(); n++ {
		fields := strings.SplitN(scanner.Text(), "\t", 3)
		if len(fields) != 3 {
			t.Fatalf("line %d: want 3 fields, got %d", n, len(fields))
		}
		score, err := strconv.Atoi(fields[0])
		if err != nil {
			t.Fatalf("line %d: %v", n, err)
		}
		cases = append(cases, rustCase{line: n, score: score, username: fields[1], password: fields[2]})
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	return cases
}

func userInputs(username string) []string {
	if username == "" {
		return nil
	}
	return []string{username}
}

func pinReferenceYear(t *testing.T) {
	t.Helper()
	saved := scoring.ReferenceYear
	scoring.ReferenceYear = corpusYear
	t.Cleanup(func() { scoring.ReferenceYear = saved })
}

// Each score is Rust zxcvbn 3.1.1's. Upstream trustelem/zxcvbn, indexing
// bytes, scored the weak ones 3-4 and the random ones 0.
func TestScoresNonASCIILikeRust(t *testing.T) {
	for _, tc := range []struct {
		password string
		want     int
	}{
		{"密碼密碼密碼密碼1", 1},
		{"éééééééééééé1", 1},
		{"あいうえおかきくけこさし", 1},
		{"абвгдежзийкл", 0},
		{"парольпароль", 2},
		{strings.Repeat("é", 127) + "x", 1},
		{"😀😀😀😀😀😀😀😀😀😀😀😀", 0},
		{"鋒蔣窄濤蠟慘囑滬駐誼壹溝", 4},
		{"дтэщшвигпшоп", 4},
		{"ψζλκθφωνξμβη", 4},
		{"zé密рψ😀kô藍дλぬ", 4},
	} {
		if got := PasswordStrength(tc.password, nil).Score; got != tc.want {
			t.Errorf("%q: score %d, want %d", tc.password, got, tc.want)
		}
	}
}

// Twelve random characters from a large alphabet are about 10^12 brute-force
// guesses, whatever the script.
func TestRandomNonASCIIIsStrong(t *testing.T) {
	alphabets := map[string][2]rune{
		"CJK":      {0x4E00, 0x9FFF},
		"Cyrillic": {0x0410, 0x044F},
		"Greek":    {0x03B1, 0x03C9},
		"Hangul":   {0xAC00, 0xD7A3},
		"emoji":    {0x1F300, 0x1F3FF},
	}
	rng := rand.New(rand.NewPCG(1, 2))
	for name, span := range alphabets {
		for range 200 {
			password := randomText(rng, span, 12)
			if got := PasswordStrength(password, nil).Score; got < minAcceptedScore {
				t.Errorf("%s %q: score %d", name, password, got)
			}
		}
	}
}

func randomText(rng *rand.Rand, span [2]rune, n int) string {
	runes := make([]rune, n)
	for i := range runes {
		runes[i] = span[0] + rune(rng.IntN(int(span[1]-span[0]+1)))
	}
	return string(runes)
}

func TestInvalidUTF8ScoresZero(t *testing.T) {
	if got := PasswordStrength(strings.Repeat("\xff", 20), nil).Score; got != 0 {
		t.Fatalf("score %d", got)
	}
}

// The policy accepts a score of 3 or more, so that verdict must match Rust on
// every case. Exact scores are reported, not required.
func TestVerdictsMatchRustZxcvbn(t *testing.T) {
	pinReferenceYear(t)
	cases := loadRustCorpus(t)
	scoreDiffs := 0
	for _, tc := range cases {
		got := PasswordStrength(tc.password, userInputs(tc.username)).Score
		if got != tc.score {
			scoreDiffs++
			t.Logf("line %d %q (user %q): score %d, Rust %d", tc.line, tc.password, tc.username, got, tc.score)
		}
		if (got >= minAcceptedScore) != (tc.score >= minAcceptedScore) {
			t.Errorf("line %d %q (user %q): score %d, Rust %d: verdict differs",
				tc.line, tc.password, tc.username, got, tc.score)
		}
	}
	t.Logf("%d cases, %d exact score differences", len(cases), scoreDiffs)
}
