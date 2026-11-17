package matching

import (
	"slices"

	"github.com/henry40408/rdrs/internal/zxcvbn/match"
	"github.com/henry40408/rdrs/internal/zxcvbn/scoring"
)

type repeatMatch struct {
	omni *omnimatcher
}

// repeatRun is a block repeated back to back: password[start:start+size*count].
type repeatRun struct {
	start, size, count int
}

func (r repeatRun) end() int {
	return r.start + r.size*r.count
}

// Matches mirrors the regexes upstream and Rust use, (.+)\1+ (greedy) and
// (.+?)\1+ (lazy), scanned left to right, but on runes rather than bytes.
func (rm repeatMatch) Matches(password []rune) []*match.Match {
	var matches []*match.Match
	for lastIndex := 0; lastIndex < len(password); {
		run, ok := nextRepeat(password, lastIndex)
		if !ok {
			break
		}
		matches = append(matches, rm.repeatMatchFor(password, run))
		lastIndex = run.end()
	}
	return matches
}

// nextRepeat picks between the greedy and the lazy run as zxcvbn does: greedy
// wins for "aabaab" (block "aab"), lazy for "aaaaa" (block "a").
func nextRepeat(password []rune, from int) (repeatRun, bool) {
	start, ok := firstRepeatStart(password, from)
	if !ok {
		return repeatRun{}, false
	}
	greedy := longestBlockRun(password, start)
	lazy := shortestBlockRun(password, start)
	if greedy.end() <= lazy.end() {
		return lazy, true
	}
	// The greedy block may itself be a repeat: "aabaab" in "aabaabaabaab".
	token := password[greedy.start:greedy.end()]
	size := shortestPeriod(token)
	return repeatRun{start: greedy.start, size: size, count: len(token) / size}, true
}

func (rm repeatMatch) repeatMatchFor(password []rune, run repeatRun) *match.Match {
	base := password[run.start : run.start+run.size]
	baseAnalysis := scoring.MostGuessableMatchSequence(base, rm.omni.matches(base), false)
	return &match.Match{
		Pattern:     "repeat",
		I:           run.start,
		J:           run.end() - 1,
		Token:       string(password[run.start:run.end()]),
		BaseToken:   string(base),
		BaseGuesses: baseAnalysis.Guesses,
		BaseMatches: baseAnalysis.Sequence,
		RepeatCount: run.count,
	}
}

func firstRepeatStart(password []rune, from int) (int, bool) {
	for start := from; start < len(password); start++ {
		if shortestBlockRun(password, start).count > 0 {
			return start, true
		}
	}
	return 0, false
}

func longestBlockRun(password []rune, start int) repeatRun {
	for size := (len(password) - start) / 2; size >= 1; size-- {
		if run := blockRun(password, start, size); run.count > 0 {
			return run
		}
	}
	return repeatRun{}
}

func shortestBlockRun(password []rune, start int) repeatRun {
	for size := 1; size <= (len(password)-start)/2; size++ {
		if run := blockRun(password, start, size); run.count > 0 {
			return run
		}
	}
	return repeatRun{}
}

// blockRun counts how often the size-rune block at start occurs back to back.
// count is 0 unless it occurs at least twice. The block may not contain a
// newline, because the regex '.' does not match one.
func blockRun(password []rune, start, size int) repeatRun {
	block := password[start : start+size]
	if slices.Contains(block, '\n') {
		return repeatRun{}
	}
	count := 1
	for next := start + size; next+size <= len(password); next += size {
		if !slices.Equal(password[next:next+size], block) {
			break
		}
		count++
	}
	if count < 2 {
		return repeatRun{}
	}
	return repeatRun{start: start, size: size, count: count}
}

// shortestPeriod is the smallest block size that, repeated, makes up all of
// token, as ^(.+?)\1+$ finds.
func shortestPeriod(token []rune) int {
	for size := 1; size <= len(token)/2; size++ {
		if len(token)%size == 0 && isRepetition(token, size) {
			return size
		}
	}
	return len(token)
}

func isRepetition(token []rune, size int) bool {
	for i := size; i < len(token); i++ {
		if token[i] != token[i-size] {
			return false
		}
	}
	return true
}
