// Package feedparse reads RSS, Atom, RSS 1.0 and JSON Feed documents into the
// fields the sync needs. It follows the semantics of the feed-rs crate the
// Rust build used, because the field choices decide which entries a refresh
// treats as new.
package feedparse

import "time"

// Type is the kind of feed document.
type Type string

const (
	TypeAtom Type = "Atom"
	TypeJSON Type = "JSON"
	TypeRSS0 Type = "RSS0"
	TypeRSS1 Type = "RSS1"
	TypeRSS2 Type = "RSS2"
)

// Target says what a link points at when the feed says it is not the entry.
type Target int

const (
	TargetNone Target = iota
	TargetComments
	TargetCommentsFeed
)

// Link is an href with the little else the sync cares about.
type Link struct {
	Href   string
	Target Target
}

// Person is an author; Name is nil when the feed gave only an address.
type Person struct {
	Name  *string
	Email *string
}

// Entry is one item of a feed. Optional fields are nil when absent, which is
// not the same as empty.
type Entry struct {
	ID        string
	Title     *string
	Links     []Link
	Content   *string
	Summary   *string
	Authors   []Person
	Published *time.Time
	Updated   *time.Time
}

// Feed is a parsed document.
type Feed struct {
	Type        Type
	ID          string
	Title       *string
	Description *string
	Updated     *time.Time
	Published   *time.Time
	Links       []Link
	Icon        *string
	Logo        *string
	Entries     []Entry
}
