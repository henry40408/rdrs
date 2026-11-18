package opml

import (
	"context"
	"fmt"
	"log/slog"
	"net/url"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
)

// ImportSummary is what an import did, so "imported" can tell 300 new feeds
// from all duplicates.
type ImportSummary struct {
	CategoriesCreated int
	FeedsAdded        int
	// FeedsSkipped are already subscribed in the same category.
	FeedsSkipped int
	// FeedsFailed were refused: an unfetchable URL, or rejected by the database.
	FeedsFailed int
}

// Describe is a one-line summary for a flash message; zero-count clauses are
// omitted.
func (s ImportSummary) Describe() string {
	plural := "s"
	if s.FeedsAdded == 1 {
		plural = ""
	}
	out := fmt.Sprintf("OPML imported: %d feed%s added", s.FeedsAdded, plural)
	if s.FeedsSkipped > 0 {
		out += fmt.Sprintf(", %d already subscribed", s.FeedsSkipped)
	}
	if s.FeedsFailed > 0 {
		out += fmt.Sprintf(", %d failed", s.FeedsFailed)
	}
	return out + "."
}

// ImportParams are the inputs of Import.
type ImportParams struct {
	DB       db.Executor
	UserID   int64
	Outlines []Outline
	Fetcher  *fetch.Fetcher
}

// Import subscribes the user to every feed, creating missing categories.
// Duplicates are skipped and refusals counted, so one bad entry cannot sink the
// run; only a category or lookup failure is fatal.
func Import(ctx context.Context, p ImportParams) (ImportSummary, error) {
	var summary ImportSummary
	for _, outline := range p.Outlines {
		category, err := models.FindCategoryByNameAndUser(ctx, p.DB, outline.CategoryName, p.UserID)
		if err != nil {
			return summary, err
		}
		if category == nil {
			summary.CategoriesCreated++
			if category, err = models.CreateCategory(ctx, p.DB, p.UserID, outline.CategoryName); err != nil {
				return summary, err
			}
		}
		for _, feed := range outline.Feeds {
			if err := importFeed(ctx, p, category.ID, feed, &summary); err != nil {
				return summary, err
			}
		}
	}
	return summary, nil
}

func importFeed(ctx context.Context, p ImportParams, categoryID int64, feed Feed, summary *ImportSummary) error {
	// OPML is as untrusted as a typed URL.
	if parsed, err := url.Parse(feed.XMLURL); err != nil || p.Fetcher.Validate(parsed) != nil {
		summary.FeedsFailed++
		slog.Warn("skipping OPML feed whose URL is not fetchable", "event", "opml.feed_import_rejected", "url", feed.XMLURL)
		return nil
	}
	existing, err := models.FindFeedByURLAndCategory(ctx, p.DB, feed.XMLURL, categoryID)
	if err != nil {
		return err
	}
	if existing != nil {
		summary.FeedsSkipped++
		return nil
	}
	_, err = models.CreateFeed(ctx, p.DB, models.CreateFeedParams{
		CategoryID: categoryID, URL: feed.XMLURL, Title: feed.Title, SiteURL: feed.HTMLURL,
	})
	if err != nil {
		summary.FeedsFailed++
		slog.Warn("skipping OPML feed the database rejected", "event", "opml.feed_import_failed", "url", feed.XMLURL, "error", err)
		return nil
	}
	summary.FeedsAdded++
	return nil
}
