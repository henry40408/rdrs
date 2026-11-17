package match

// Match is one pattern found in a password. I and J are inclusive rune
// (not byte) offsets into the password.
type Match struct {
	Pattern string
	I       int
	J       int
	Token   string

	// Dictionary
	Reversed            bool
	UppercaseVariations float64
	L33tVariations      float64
	MatchedWord         string
	Rank                int
	DictionaryName      string
	L33t                bool
	Sub                 map[rune]rune

	// Spatial and sequence
	Graph         string
	SequenceName  string
	SequenceSpace int
	Ascending     bool
	Turns         int
	ShiftedCount  int

	// Repeat
	BaseToken   string
	BaseGuesses float64
	BaseMatches []*Match
	RepeatCount int

	// Regexp
	RegexName string

	// Date
	Year      int
	Month     int
	Day       int
	Separator string
	Guesses   float64
}

// Matcher finds the matches of one pattern in a password given as runes.
type Matcher interface {
	Matches(password []rune) []*Match
}
