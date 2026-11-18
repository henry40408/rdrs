package feedparse

import (
	"regexp"
	"strings"
)

// emailPattern is the feed-rs pattern, delimiters and all, so a name is split
// off an address exactly where the Rust parser split it.
var emailPattern = regexp.MustCompile(`\b(<\[\()?(mailto:)?([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})(>]\))?\b`)

// parsePerson splits "address (Name)" style text. The longer of what comes
// before and after the address is the name; with no address the whole text is.
func parsePerson(raw string) Person {
	m := emailPattern.FindStringSubmatchIndex(raw)
	if m == nil {
		return Person{Name: ptr(strings.TrimSpace(raw))}
	}
	start, end := m[0], m[1]
	person := Person{Email: ptr(strings.TrimSpace(raw[m[6]:m[7]]))}
	var name string
	switch {
	case start > len(raw)-end:
		name = raw[:start-1]
	case end < len(raw):
		name = raw[end+1:]
	}
	if name = strings.TrimSpace(name); name != "" {
		person.Name = &name
	}
	return person
}
