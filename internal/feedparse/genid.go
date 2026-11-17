package feedparse

import (
	"crypto/rand"
	"fmt"

	"github.com/henry40408/rdrs/internal/siphash"
)

// Keys of the feed-rs id hash. Changing either makes every feed without
// guids look new, so refreshes would insert duplicates.
const (
	idKey0 = 0x5d78_4074_2887_2d60
	idKey1 = 0x90ee_ca4c_90a5_e228
)

// GenerateID is the id of an entry that has none: a hash of its first link
// and title, or a random UUID when it has no link.
func GenerateID(links []Link, title *string) string {
	if len(links) == 0 {
		return uuidV4()
	}
	data := []byte(links[0].Href)
	if title != nil {
		data = append(data, *title...)
	}
	h1, h2 := siphash.Sum24x128(idKey0, idKey1, data)
	return fmt.Sprintf("%x%x", h1, h2)
}

func assignMissingIDs(feed *Feed) {
	if feed.ID == "" {
		feed.ID = GenerateID(feed.Links, feed.Title)
	}
	for i := range feed.Entries {
		if feed.Entries[i].ID == "" {
			feed.Entries[i].ID = GenerateID(feed.Entries[i].Links, feed.Entries[i].Title)
		}
	}
}

func uuidV4() string {
	var b [16]byte
	_, _ = rand.Read(b[:]) // crypto/rand does not fail on supported platforms
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:])
}
