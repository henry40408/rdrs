package feedparse

import "time"

// Link is the entry's address: the first link in document order.
func (e *Entry) Link() *string {
	if len(e.Links) == 0 {
		return nil
	}
	return &e.Links[0].Href
}

// Body is the full content, or the summary when the entry has none.
func (e *Entry) Body() *string {
	if e.Content != nil {
		return e.Content
	}
	return e.Summary
}

// Author is the name of the first author, when it has one.
func (e *Entry) Author() *string {
	if len(e.Authors) == 0 {
		return nil
	}
	return e.Authors[0].Name
}

// PublishedAt is published, else updated, else the feed's own timestamp, in
// UTC and truncated to the second; nil when the feed gives no date at all.
func (e *Entry) PublishedAt(feedTimestamp *time.Time) *time.Time {
	at := e.Published
	if at == nil {
		at = e.Updated
	}
	if at == nil {
		at = feedTimestamp
	}
	if at == nil {
		return nil
	}
	utc := at.UTC()
	return &utc
}

// Timestamp is the feed's own "last updated" time: updated, else published.
func (f *Feed) Timestamp() *time.Time {
	if f.Updated != nil {
		return f.Updated
	}
	return f.Published
}
