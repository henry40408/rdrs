// Package greader holds the pure parts of the Google Reader compatible API:
// stream and item identifiers, the JSON response shapes and the POST token.
// The HTTP handlers live in internal/server.
package greader

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/henry40408/rdrs/internal/models"
)

// StreamKind says what a stream identifier names.
type StreamKind int

// The stream kinds.
const (
	// ReadingList is user/-/state/com.google/reading-list: every item.
	ReadingList StreamKind = iota
	Read
	Starred
	KeptUnread
	// Label is user/-/label/<name>; the stream's Name is the label.
	Label
	// Feed is feed/<url>; the stream's Name is the URL.
	Feed
)

// Stream is a parsed Google Reader stream identifier.
type Stream struct {
	Kind StreamKind
	// Name is the label or the feed URL; empty for the state streams.
	Name string
}

// State stream identifiers, as clients send and receive them.
const (
	ReadingListID = "user/-/state/com.google/reading-list"
	ReadID        = "user/-/state/com.google/read"
	StarredID     = "user/-/state/com.google/starred"
	KeptUnreadID  = "user/-/state/com.google/kept-unread"
)

func invalid(format string, args ...any) error {
	return &models.ValidationError{Message: fmt.Sprintf(format, args...)}
}

// ParseStream parses a stream identifier. Failures are
// *models.ValidationError, which the handlers answer with 400.
func ParseStream(s string) (Stream, error) {
	if url, ok := strings.CutPrefix(s, "feed/"); ok {
		if url == "" {
			return Stream{}, invalid("Empty feed URL in stream ID")
		}
		return Stream{Kind: Feed, Name: url}, nil
	}
	normalized, ok := withoutUserPart(s)
	if !ok {
		return Stream{}, invalid("Invalid stream ID: %s", s)
	}
	switch normalized {
	case "state/com.google/reading-list":
		return Stream{Kind: ReadingList}, nil
	case "state/com.google/read":
		return Stream{Kind: Read}, nil
	case "state/com.google/starred":
		return Stream{Kind: Starred}, nil
	case "state/com.google/kept-unread":
		return Stream{Kind: KeptUnread}, nil
	}
	name, isLabel := strings.CutPrefix(normalized, "label/")
	if !isLabel {
		return Stream{}, invalid("Unknown stream ID: %s", s)
	}
	if name == "" {
		return Stream{}, invalid("Empty label name")
	}
	return Stream{Kind: Label, Name: name}, nil
}

// withoutUserPart drops "user/-/" or "user/<id>/": clients send either.
func withoutUserPart(s string) (string, bool) {
	afterUser, ok := strings.CutPrefix(s, "user/")
	if !ok {
		return "", false
	}
	if rest, ok := strings.CutPrefix(s, "user/-/"); ok {
		return rest, true
	}
	_, rest, found := strings.Cut(afterUser, "/")
	return rest, found
}

// String is the identifier a client sees for the stream.
func (s Stream) String() string {
	switch s.Kind {
	case ReadingList:
		return ReadingListID
	case Read:
		return ReadID
	case Starred:
		return StarredID
	case KeptUnread:
		return KeptUnreadID
	case Label:
		return "user/-/label/" + s.Name
	}
	return "feed/" + s.Name
}

// LabelName is the label of a "user/-/label/<name>" or
// "user/<id>/label/<name>" string; false when s is neither.
func LabelName(s string) (string, bool) {
	afterUser, ok := strings.CutPrefix(s, "user/")
	if !ok {
		return "", false
	}
	if rest, ok := strings.CutPrefix(s, "user/-/label/"); ok {
		return rest, true
	}
	_, rest, found := strings.Cut(afterUser, "/label/")
	return rest, found
}

const itemIDPrefix = "tag:google.com,2005:reader/item/"

// ItemID is the long item identifier of an entry.
func ItemID(entryID int64) string {
	// uint64 prints a negative id as two's complement, like Rust's {:016x}.
	return fmt.Sprintf("%s%016x", itemIDPrefix, uint64(entryID))
}

// EntryID parses an item identifier: the long form, a decimal number, or the
// bare hex some clients send.
func EntryID(s string) (int64, error) {
	if hex, ok := strings.CutPrefix(s, itemIDPrefix); ok {
		return parseHexID(s, hex)
	}
	if id, err := strconv.ParseInt(s, 10, 64); err == nil {
		return id, nil
	}
	return parseHexID(s, s)
}

func parseHexID(original, hex string) (int64, error) {
	hex = strings.TrimLeft(hex, "0")
	if hex == "" {
		return 0, nil
	}
	id, err := strconv.ParseInt(hex, 16, 64)
	if err != nil {
		return 0, invalid("Invalid item ID: %s", original)
	}
	return id, nil
}
