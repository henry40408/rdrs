package feedsync

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/datetime"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/feedparse"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Counts is how a refresh changed the stored entries.
type Counts struct {
	New       int64
	Updated   int64
	Unchanged int64
	Skipped   int64
}

// validators are the cache validators of a response.
type validators struct {
	ETag         *string
	LastModified *string
}

type batch struct {
	feedID        int64
	feedTimestamp *time.Time
	validators    validators
}

// latest is the newest of the non-nil times: the feed's own timestamp, its
// newest entry, or the HTTP Last-Modified, which guards against frozen
// in-feed dates.
func latest(times ...*time.Time) *time.Time {
	var best *time.Time
	for _, t := range times {
		if t != nil && (best == nil || t.After(*best)) {
			best = t
		}
	}
	return best
}

func (c *Counts) record(outcome entry.Outcome) {
	switch outcome {
	case entry.Inserted:
		c.New++
	case entry.Updated:
		c.Updated++
	case entry.Unchanged:
		c.Unchanged++
	case entry.SkippedTombstoned:
		c.Skipped++
	}
}

func upsertOf(feedID int64, item *feedparse.Entry, publishedAt *time.Time) *entry.Upsert {
	return &entry.Upsert{
		FeedID: feedID, GUID: item.ID, Title: item.Title, Link: item.Link(), Content: item.Body(),
		Summary: item.Summary, Author: item.Author(), PublishedAt: publishedAt,
	}
}

// apply upserts every entry and records the fetch result in one transaction:
// a single commit, and readers never see a half-applied sync.
func apply(ctx context.Context, database *db.DB, parsed *feedparse.Feed, b batch) (Counts, error) {
	var counts Counts
	tx, err := database.Begin(ctx)
	if err != nil {
		return counts, err
	}
	defer func() { _ = tx.Rollback() }()

	var newest *time.Time
	for i := range parsed.Entries {
		item := &parsed.Entries[i]
		publishedAt := item.PublishedAt(b.feedTimestamp)
		newest = latest(newest, publishedAt)
		result, err := entry.UpsertEntry(ctx, tx, upsertOf(b.feedID, item, publishedAt))
		if err != nil {
			return counts, err
		}
		counts.record(result.Outcome)
	}

	err = models.UpdateFetchResult(ctx, tx, models.FetchResult{
		ID: b.feedID, FetchedAt: time.Now(), ETag: b.validators.ETag, LastModified: b.validators.LastModified,
		FeedUpdatedAt: latest(b.feedTimestamp, newest, parseHTTPDate(b.validators.LastModified)),
	})
	if err != nil {
		return counts, err
	}
	return counts, tx.Commit()
}

func parseHTTPDate(text *string) *time.Time {
	if text == nil {
		return nil
	}
	parsed, ok := datetime.ParseTimestamp(*text)
	if !ok {
		return nil
	}
	return &parsed
}
